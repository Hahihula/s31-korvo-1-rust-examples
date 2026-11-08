//! Gain steps of the ES8389 microphone amplifier (PGA).

/// Microphone PGA gain, as in Espressif's `es8389_reg.h`: 0 dB, then 3.5 dB
/// to 36.5 dB in 3 dB steps (register codes 0 to 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gain(u8);

impl Gain {
    /// The lowest gain, 0 dB.
    pub const MIN: Gain = Gain(0);
    /// The highest gain, 36.5 dB.
    pub const MAX: Gain = Gain(12);

    /// The nearest gain at or below `db`, within 0 to 36.5 dB (NaN and
    /// negative values give 0 dB).
    #[must_use]
    pub fn from_db(db: f32) -> Self {
        if db >= 3.5 {
            // `as u8` saturates; the cap keeps it at 36.5 dB.
            Gain((((db - 3.5) / 3.0) as u8 + 1).min(Self::MAX.0))
        } else {
            Gain::MIN
        }
    }

    /// The gain in dB.
    #[must_use]
    pub fn db(self) -> f32 {
        if self.0 == 0 {
            0.0
        } else {
            3.5 + 3.0 * f32::from(self.0 - 1)
        }
    }

    /// The register code (0..=12).
    #[must_use]
    pub fn code(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert_eq!(Gain::from_db(0.0).db(), 0.0);
        assert_eq!(Gain::from_db(3.4).db(), 0.0);
        assert_eq!(Gain::from_db(3.5).db(), 3.5);
        assert_eq!(Gain::from_db(24.5).db(), 24.5);
        assert_eq!(Gain::from_db(33.5).db(), 33.5);
        assert_eq!(Gain::from_db(35.0).db(), 33.5);
    }

    #[test]
    fn capped_at_36_5_db() {
        assert_eq!(Gain::from_db(36.5), Gain::MAX);
        assert_eq!(Gain::MAX.db(), 36.5);
        assert_eq!(Gain::from_db(100.0), Gain::MAX);
    }

    #[test]
    fn out_of_range_is_zero() {
        assert_eq!(Gain::from_db(-3.0), Gain::MIN);
        assert_eq!(Gain::from_db(f32::NAN), Gain::MIN);
    }

    #[test]
    fn codes_round_trip() {
        for code in 0..=12 {
            assert_eq!(Gain::from_db(Gain(code).db()).code(), code);
        }
    }
}
