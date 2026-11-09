//! Gain steps of the ES8389 microphone amplifier (PGA), in dB.

use es8389::MicGain;

/// Steps in ascending order, with their gain in dB.
const STEPS: [(MicGain, f32); 13] = [
    (MicGain::Db0, 0.0),
    (MicGain::Db3_5, 3.5),
    (MicGain::Db6_5, 6.5),
    (MicGain::Db9_5, 9.5),
    (MicGain::Db12_5, 12.5),
    (MicGain::Db15_5, 15.5),
    (MicGain::Db18_5, 18.5),
    (MicGain::Db21_5, 21.5),
    (MicGain::Db24_5, 24.5),
    (MicGain::Db27_5, 27.5),
    (MicGain::Db30_5, 30.5),
    (MicGain::Db33_5, 33.5),
    (MicGain::Db36_5, 36.5),
];

/// Microphone PGA gain: 0 dB, then 3.5 dB to 36.5 dB in 3 dB steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gain(usize);

impl Gain {
    /// The lowest gain, 0 dB.
    pub const MIN: Gain = Gain(0);
    /// The highest gain, 36.5 dB.
    pub const MAX: Gain = Gain(STEPS.len() - 1);

    /// The nearest gain at or below `db`, within 0 to 36.5 dB (NaN and
    /// negative values give 0 dB).
    #[must_use]
    pub fn from_db(db: f32) -> Self {
        let mut best = 0;
        for (i, &(_, step_db)) in STEPS.iter().enumerate() {
            if db >= step_db {
                best = i;
            }
        }
        Gain(best)
    }

    /// The next higher step (or the same at the maximum).
    #[must_use]
    pub fn up(self) -> Self {
        Gain((self.0 + 1).min(Self::MAX.0))
    }

    /// The next lower step (or the same at the minimum).
    #[must_use]
    pub fn down(self) -> Self {
        Gain(self.0.saturating_sub(1))
    }

    /// The gain in dB.
    #[must_use]
    pub fn db(self) -> f32 {
        STEPS[self.0].1
    }

    /// The step for [`es8389::Es8389::set_mic_gain`].
    #[must_use]
    pub fn mic_gain(self) -> MicGain {
        STEPS[self.0].0
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
    fn up_down_clamp() {
        assert_eq!(Gain::MIN.up().db(), 3.5);
        assert_eq!(Gain::MIN.down(), Gain::MIN);
        assert_eq!(Gain::MAX.up(), Gain::MAX);
        assert_eq!(Gain::MAX.down().db(), 33.5);
    }

    #[test]
    fn maps_to_mic_gain() {
        assert_eq!(Gain::MIN.mic_gain(), MicGain::Db0);
        assert_eq!(Gain::from_db(33.5).mic_gain(), MicGain::Db33_5);
        assert_eq!(Gain::MAX.mic_gain(), MicGain::Db36_5);
    }
}
