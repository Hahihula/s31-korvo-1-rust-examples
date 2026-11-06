//! The four function buttons share one ADC pin (GPIO42 = ADC1_CH0).
//!
//! A 10 kΩ pull-up to 3.3 V and one resistor per button to GND form a ladder.
//! Nominal voltages from the schematic:
//!
//! | Button | Resistor | Voltage |
//! |--------|----------|---------|
//! | VOL+   | 1.3 kΩ   | 0.38 V  |
//! | VOL-   | 3.3 kΩ   | 0.82 V  |
//! | PLAY   | 6.8 kΩ   | 1.34 V  |
//! | SET    | 13 kΩ    | 1.87 V  |
//! | none   | -        | 3.30 V  |
//!
//! The ESP32-S31 SAR ADC is differential and its scale runs backwards: a
//! grounded input reads about [`ZERO_DIFF_CODE`] (2198), and higher voltages
//! give *lower* codes, about 0.91 mV per code, down to 0 at roughly 2 V. The
//! idle level (3.3 V) is above that range and reads 0, while all four buttons
//! are below 2 V. (Conversion taken from
//! <https://github.com/playfulFence/esp-touch-paint>; esp-hal has no S31 ADC
//! calibration yet.)
//!
//! The BOOT button (GPIO61) is a plain digital input and is handled by
//! [`BootButton`]. It is also a strapping pin: holding it during reset enters
//! download mode.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    Blocking,
    analog::adc::{Adc, AdcConfig, AdcPin, Attenuation, Error as AdcError, FULL_SCALE},
    gpio::{Input, InputConfig, Pull},
    peripherals::{ADC1, GPIO42, GPIO61},
};

#[cfg(doc)]
use esp_hal::analog::adc::ZERO_DIFF_CODE;

/// Input voltage at raw code 0; the scale spans twice this over [`FULL_SCALE`].
const RANGE_MV: u32 = 2000;
/// Poll period of the ladder.
const POLL: Duration = Duration::from_millis(10);
/// Number of equal consecutive polls before a state change is accepted.
const DEBOUNCE_POLLS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    VolUp,
    VolDown,
    Play,
    Set,
}

impl Button {
    pub const ALL: [Button; 4] = [Button::VolUp, Button::VolDown, Button::Play, Button::Set];

    pub const fn name(self) -> &'static str {
        match self {
            Button::VolUp => "VOL+",
            Button::VolDown => "VOL-",
            Button::Play => "PLAY",
            Button::Set => "SET",
        }
    }

    pub const fn index(self) -> usize {
        self as usize
    }

    /// Maps a ladder voltage to a button, splitting halfway between nominal
    /// levels. Idle (3.3 V) clips to about 2000 mV, just above SET (1.87 V).
    pub const fn from_millivolts(mv: u32) -> Option<Button> {
        match mv {
            0..600 => Some(Button::VolUp),
            600..1080 => Some(Button::VolDown),
            1080..1605 => Some(Button::Play),
            1605..1940 => Some(Button::Set),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Event {
    Pressed { button: Button, raw: u16, mv: u32 },
    Released { button: Button, held: Duration },
}

/// Estimated pin voltage in millivolts for a raw S31 ADC code (see the module
/// docs); clips at about 2000 mV.
pub const fn millivolts(raw: u16) -> u32 {
    RANGE_MV.saturating_sub(raw as u32 * 2 * RANGE_MV / FULL_SCALE as u32)
}

pub struct Buttons<'d> {
    adc: Adc<'d, ADC1<'d>, Blocking>,
    pin: AdcPin<GPIO42<'d>, ADC1<'d>>,
    state: Option<Button>,
    pressed_at: Instant,
    candidate: Option<Button>,
    candidate_polls: u8,
}

impl<'d> Buttons<'d> {
    pub fn new(adc1: ADC1<'d>, gpio42: GPIO42<'d>) -> Result<Self, AdcError> {
        let mut config = AdcConfig::new();
        // The S31 SAR has a single input range; the attenuation is ignored.
        let pin = config.enable_pin(gpio42, Attenuation::_11dB);
        let adc = Adc::new(adc1, config)?;
        Ok(Self {
            adc,
            pin,
            state: None,
            pressed_at: Instant::now(),
            candidate: None,
            candidate_polls: 0,
        })
    }

    /// Average of four conversions. A failed conversion counts as idle (0).
    pub fn read_raw(&mut self) -> u16 {
        let sum: u32 = (0..4)
            .map(|_| u32::from(self.adc.read_blocking(&mut self.pin).unwrap_or(0)))
            .sum();
        (sum / 4) as u16
    }

    /// One averaged reading as `(raw, millivolts)`.
    pub fn sample(&mut self) -> (u16, u32) {
        let raw = self.read_raw();
        (raw, millivolts(raw))
    }

    /// Waits for the next debounced press or release.
    pub async fn next_event(&mut self) -> Event {
        loop {
            Timer::after(POLL).await;

            let (raw, mv) = self.sample();
            let reading = Button::from_millivolts(mv);

            if reading != self.candidate {
                self.candidate = reading;
                self.candidate_polls = 1;
                continue;
            }
            if self.candidate_polls < DEBOUNCE_POLLS {
                self.candidate_polls += 1;
            }
            if self.candidate_polls < DEBOUNCE_POLLS || reading == self.state {
                continue;
            }

            // Moving straight from one button to another reports the release first.
            if let Some(button) = self.state.take() {
                return Event::Released {
                    button,
                    held: self.pressed_at.elapsed(),
                };
            }

            if let Some(button) = reading {
                self.state = Some(button);
                self.pressed_at = Instant::now();
                return Event::Pressed { button, raw, mv };
            }
        }
    }
}

/// Contact bounce settling time of the BOOT button.
const BOOT_DEBOUNCE: Duration = Duration::from_millis(20);

/// The BOOT button on GPIO61 (active low, pressed = GND).
pub struct BootButton<'d> {
    input: Input<'d>,
}

impl<'d> BootButton<'d> {
    pub fn new(gpio61: GPIO61<'d>) -> Self {
        let input = Input::new(gpio61, InputConfig::default().with_pull(Pull::Up));
        Self { input }
    }

    /// Waits for a complete debounced press and returns how long it was held.
    pub async fn next_press(&mut self) -> Duration {
        let pressed_at = loop {
            self.input.wait_for_low().await;
            Timer::after(BOOT_DEBOUNCE).await;
            if self.input.is_low() {
                break Instant::now();
            }
        };
        loop {
            self.input.wait_for_high().await;
            Timer::after(BOOT_DEBOUNCE).await;
            if self.input.is_high() {
                return pressed_at.elapsed();
            }
        }
    }
}
