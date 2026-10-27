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
//! The ESP32-S31 ADC has no calibration in esp-hal yet: a grounded input reads
//! about [`ZERO_DIFF_CODE`]. The idle level (3.3 V) is sampled at start-up and
//! readings are scaled linearly between those two points.
//!
//! The BOOT button (GPIO61) is a plain digital input and is handled by
//! [`BootButton`]. It is also a strapping pin: holding it during reset enters
//! download mode.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    Blocking,
    analog::adc::{
        Adc, AdcConfig, AdcPin, Attenuation, Error as AdcError, FULL_SCALE, ZERO_DIFF_CODE,
    },
    gpio::{Input, InputConfig, Pull},
    peripherals::{ADC1, GPIO42, GPIO61},
};

/// Supply voltage of the ladder pull-up.
const IDLE_MV: u32 = 3300;
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

    /// Maps a ladder voltage to a button, splitting halfway between nominal levels.
    pub const fn from_millivolts(mv: u32) -> Option<Button> {
        match mv {
            0..600 => Some(Button::VolUp),
            600..1080 => Some(Button::VolDown),
            1080..1600 => Some(Button::Play),
            1600..2580 => Some(Button::Set),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Event {
    Pressed { button: Button, raw: u16, mv: u32 },
    Released { button: Button, held: Duration },
}

pub struct Buttons<'d> {
    adc: Adc<'d, ADC1<'d>, Blocking>,
    pin: AdcPin<GPIO42<'d>, ADC1<'d>>,
    idle_raw: u16,
    state: Option<Button>,
    pressed_at: Instant,
    candidate: Option<Button>,
    candidate_polls: u8,
}

impl<'d> Buttons<'d> {
    /// Sets up the ADC and samples the idle level. Keep the buttons released
    /// during this call.
    pub fn new(adc1: ADC1<'d>, gpio42: GPIO42<'d>) -> Result<Self, AdcError> {
        let mut config = AdcConfig::new();
        // The S31 SAR has a single input range; the attenuation is ignored.
        let pin = config.enable_pin(gpio42, Attenuation::_11dB);
        let adc = Adc::new(adc1, config)?;

        let mut this = Self {
            adc,
            pin,
            idle_raw: FULL_SCALE,
            state: None,
            pressed_at: Instant::now(),
            candidate: None,
            candidate_polls: 0,
        };

        let idle = (0..16).map(|_| this.read_raw()).max().unwrap_or(FULL_SCALE);
        // A pressed button at boot (or a saturated ADC) gives a bogus idle level.
        this.idle_raw = if idle > ZERO_DIFF_CODE + 1000 {
            idle
        } else {
            FULL_SCALE
        };
        Ok(this)
    }

    pub fn idle_raw(&self) -> u16 {
        self.idle_raw
    }

    /// Average of four conversions. A failed conversion counts as the idle level.
    pub fn read_raw(&mut self) -> u16 {
        let idle = self.idle_raw;
        let sum: u32 = (0..4)
            .map(|_| u32::from(self.adc.read_blocking(&mut self.pin).unwrap_or(idle)))
            .sum();
        (sum / 4) as u16
    }

    /// Estimated pin voltage in millivolts.
    pub fn millivolts(&self, raw: u16) -> u32 {
        let span = u32::from(self.idle_raw.saturating_sub(ZERO_DIFF_CODE)).max(1);
        let above_zero = u32::from(raw.saturating_sub(ZERO_DIFF_CODE));
        above_zero * IDLE_MV / span
    }

    /// Waits for the next debounced press or release.
    pub async fn next_event(&mut self) -> Event {
        loop {
            Timer::after(POLL).await;

            let raw = self.read_raw();
            let mv = self.millivolts(raw);
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
