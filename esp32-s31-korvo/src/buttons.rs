//! The buttons: the four on the ADC ladder (GPIO42 = ADC1_CH0, decoded by
//! [`crate::ladder`]) and BOOT (GPIO61).
//!
//! The BOOT button is a plain digital input and is handled by
//! [`BootButton`]. It is also a strapping pin: holding it during reset enters
//! download mode.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    Blocking,
    analog::adc::{Adc, AdcConfig, AdcPin, Attenuation, Error as AdcError},
    gpio::{Input, InputConfig, Pull},
    peripherals::{ADC1, GPIO42, GPIO61},
};

pub use crate::ladder::{Button, millivolts};
use crate::ladder::{Change, Debouncer};

/// Poll period of the ladder.
const POLL: Duration = Duration::from_millis(10);

/// A debounced press or release on the ladder.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    /// A button was pressed; `raw` and `mv` are the reading that confirmed it.
    Pressed {
        /// Which button.
        button: Button,
        /// Raw ADC code.
        raw: u16,
        /// Estimated pin voltage.
        mv: u32,
    },
    /// A button was released.
    Released {
        /// Which button.
        button: Button,
        /// How long it was held.
        held: Duration,
    },
}

/// The four ladder buttons, polled every 10 ms.
pub struct Buttons<'d> {
    adc: Adc<'d, ADC1<'d>, Blocking>,
    pin: AdcPin<GPIO42<'d>, ADC1<'d>>,
    debouncer: Debouncer,
}

impl<'d> Buttons<'d> {
    /// Sets up ADC1 on GPIO42.
    ///
    /// # Errors
    ///
    /// If the ADC can't be configured.
    pub fn new(adc1: ADC1<'d>, gpio42: GPIO42<'d>) -> Result<Self, AdcError> {
        let mut config = AdcConfig::new();
        // The S31 SAR has a single input range; the attenuation is ignored.
        let pin = config.enable_pin(gpio42, Attenuation::_11dB);
        let adc = Adc::new(adc1, config)?;
        Ok(Self {
            adc,
            pin,
            debouncer: Debouncer::new(),
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
            match self
                .debouncer
                .update(Instant::now(), Button::from_millivolts(mv))
            {
                Some(Change::Pressed(button)) => return Event::Pressed { button, raw, mv },
                Some(Change::Released(button, held)) => return Event::Released { button, held },
                None => {}
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
    /// Configures GPIO61 as an input with pull-up.
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
