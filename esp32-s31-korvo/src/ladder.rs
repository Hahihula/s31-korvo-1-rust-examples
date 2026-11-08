//! Decoding the four-button resistor ladder: ADC code to voltage, voltage to
//! button, and debouncing. No hardware here, so it is unit-tested on a PC;
//! [`crate::buttons::Buttons`] feeds it from the ADC.
//!
//! A 10 kΩ pull-up to 3.3 V and one resistor per button to GND form the
//! ladder. Nominal voltages from the schematic:
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
//! grounded input reads about 2198 (esp-hal's `ZERO_DIFF_CODE`), and higher
//! voltages give *lower* codes, about 0.91 mV per code, down to 0 at roughly
//! 2 V. The idle level (3.3 V) is above that range and reads 0, while all four
//! buttons are below 2 V. (Conversion taken from
//! <https://github.com/playfulFence/esp-touch-paint>; esp-hal has no S31 ADC
//! calibration yet.)

use embassy_time::{Duration, Instant};

/// Raw code of the S31 ADC at full scale (esp-hal's `FULL_SCALE`).
pub const FULL_SCALE: u16 = 4393;
/// Input voltage at raw code 0; the scale spans twice this over [`FULL_SCALE`].
const RANGE_MV: u32 = 2000;
/// Number of equal consecutive readings before a state change is accepted.
pub const DEBOUNCE_READINGS: u8 = 3;

/// One of the four ladder buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Button {
    /// VOL+, nearest the USB ports.
    VolUp = 0,
    /// VOL-.
    VolDown = 1,
    /// PLAY.
    Play = 2,
    /// SET.
    Set = 3,
}

impl Button {
    /// All buttons, in [`Button::index`] order.
    pub const ALL: [Button; 4] = [Button::VolUp, Button::VolDown, Button::Play, Button::Set];

    /// The label printed on the board.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Button::VolUp => "VOL+",
            Button::VolDown => "VOL-",
            Button::Play => "PLAY",
            Button::Set => "SET",
        }
    }

    /// Position in [`Button::ALL`] (0..4), e.g. for per-button arrays.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Maps a ladder voltage to a button, splitting halfway between nominal
    /// levels. Idle (3.3 V) clips to about 2000 mV, just above SET (1.87 V).
    #[must_use]
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

/// Estimated pin voltage in millivolts for a raw S31 ADC code (see the module
/// docs); clips at about 2000 mV.
#[must_use]
pub const fn millivolts(raw: u16) -> u32 {
    RANGE_MV.saturating_sub(raw as u32 * 2 * RANGE_MV / FULL_SCALE as u32)
}

/// A debounced change of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A button was pressed.
    Pressed(Button),
    /// A button was released after being held this long.
    Released(Button, Duration),
}

/// Debounces ladder readings: a new state counts after
/// [`DEBOUNCE_READINGS`] equal readings in a row.
#[derive(Debug, Clone, Copy)]
pub struct Debouncer {
    state: Option<Button>,
    pressed_at: Instant,
    candidate: Option<Button>,
    candidate_count: u8,
}

impl Debouncer {
    /// Starts with no button pressed.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: None,
            pressed_at: Instant::from_ticks(0),
            candidate: None,
            candidate_count: 0,
        }
    }

    /// The debounced state.
    #[must_use]
    pub fn pressed(&self) -> Option<Button> {
        self.state
    }

    /// Feeds one reading taken at `now`. Moving straight from one button to
    /// another reports the release first and the press with a later reading.
    pub fn update(&mut self, now: Instant, reading: Option<Button>) -> Option<Change> {
        if reading != self.candidate {
            self.candidate = reading;
            self.candidate_count = 1;
            return None;
        }
        self.candidate_count = (self.candidate_count + 1).min(DEBOUNCE_READINGS);
        if self.candidate_count < DEBOUNCE_READINGS || reading == self.state {
            return None;
        }
        if let Some(button) = self.state.take() {
            return Some(Change::Released(button, now - self.pressed_at));
        }
        let button = reading?;
        self.state = Some(button);
        self.pressed_at = now;
        Some(Change::Pressed(button))
    }
}

impl Default for Debouncer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_is_in_index_order() {
        for (i, b) in Button::ALL.iter().enumerate() {
            assert_eq!(b.index(), i);
        }
    }

    #[test]
    fn nominal_voltages_map_to_buttons() {
        assert_eq!(Button::from_millivolts(380), Some(Button::VolUp));
        assert_eq!(Button::from_millivolts(820), Some(Button::VolDown));
        assert_eq!(Button::from_millivolts(1340), Some(Button::Play));
        assert_eq!(Button::from_millivolts(1870), Some(Button::Set));
        assert_eq!(Button::from_millivolts(2000), None);
    }

    #[test]
    fn adc_scale_runs_backwards() {
        assert_eq!(millivolts(0), 2000); // idle clips here
        assert!(millivolts(2198).abs_diff(0) < 5); // grounded input
        assert!(millivolts(1000) > millivolts(1500));
    }

    fn at(ms: u64) -> Instant {
        Instant::from_millis(ms)
    }

    #[test]
    fn press_and_release_after_three_readings() {
        let mut d = Debouncer::new();
        let set = Some(Button::Set);
        assert_eq!(d.update(at(0), set), None);
        assert_eq!(d.update(at(10), set), None);
        assert_eq!(d.update(at(20), set), Some(Change::Pressed(Button::Set)));
        assert_eq!(d.update(at(30), set), None);
        assert_eq!(d.update(at(500), None), None);
        assert_eq!(d.update(at(510), None), None);
        assert_eq!(
            d.update(at(520), None),
            Some(Change::Released(Button::Set, Duration::from_millis(500)))
        );
    }

    #[test]
    fn bounces_are_ignored() {
        let mut d = Debouncer::new();
        for (t, r) in [
            (0, Some(Button::Play)),
            (10, None),
            (20, Some(Button::Play)),
            (30, None),
        ] {
            assert_eq!(d.update(at(t), r), None);
        }
        assert_eq!(d.pressed(), None);
    }

    #[test]
    fn sliding_to_another_button_releases_first() {
        let mut d = Debouncer::new();
        for t in 0..3 {
            d.update(at(t * 10), Some(Button::VolUp));
        }
        let mut changes = Vec::new();
        for t in 3..9 {
            if let Some(c) = d.update(at(t * 10), Some(Button::VolDown)) {
                changes.push(c);
            }
        }
        assert!(matches!(changes[0], Change::Released(Button::VolUp, _)));
        assert_eq!(changes[1], Change::Pressed(Button::VolDown));
    }

    extern crate std;
    use std::vec::Vec;
}
