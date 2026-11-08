//! Calendar dates from Unix time. No hardware here, so it is unit-tested on
//! a PC.

/// Calendar date and time in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    /// Year, e.g. 2026.
    pub year: i32,
    /// Month, 1..=12.
    pub month: u8,
    /// Day of the month, 1..=31.
    pub day: u8,
    /// Hour, 0..=23.
    pub hour: u8,
    /// Minute, 0..=59.
    pub minute: u8,
    /// Second, 0..=59.
    pub second: u8,
}

impl DateTime {
    /// Converts Unix seconds to a UTC date (Howard Hinnant's `civil_from_days`).
    #[must_use]
    pub fn from_unix(unix: u64) -> Self {
        let days = (unix / 86_400) as i64;
        let secs = unix % 86_400;

        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;

        Self {
            year,
            month,
            day,
            hour: (secs / 3600) as u8,
            minute: (secs / 60 % 60) as u8,
            second: (secs % 60) as u8,
        }
    }
}

impl core::fmt::Display for DateTime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd_hms(unix: u64) -> (i32, u8, u8, u8, u8, u8) {
        let d = DateTime::from_unix(unix);
        (d.year, d.month, d.day, d.hour, d.minute, d.second)
    }

    #[test]
    fn known_dates() {
        assert_eq!(ymd_hms(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(ymd_hms(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(ymd_hms(1_735_689_599), (2024, 12, 31, 23, 59, 59));
        assert_eq!(ymd_hms(4_102_444_800), (2100, 1, 1, 0, 0, 0));
        assert_eq!(ymd_hms(4_107_542_400), (2100, 3, 1, 0, 0, 0));
    }

    #[test]
    fn display() {
        extern crate std;
        use std::string::ToString;
        assert_eq!(
            DateTime::from_unix(1_735_689_599).to_string(),
            "2024-12-31 23:59:59 UTC"
        );
    }
}
