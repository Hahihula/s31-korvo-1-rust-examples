//! SNTP packets: building a client request and checking a server's reply.
//!
//! No networking here, so it is unit-tested on a PC; [`crate::net::ntp_time`]
//! sends and receives the packets.

/// Seconds between the NTP epoch (1900) and the Unix epoch (1970).
pub const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
/// Size of an SNTP packet without extensions.
pub const PACKET_LEN: usize = 48;

/// Why a reply was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReplyError {
    /// Shorter than [`PACKET_LEN`].
    TooShort,
    /// Not a server reply (mode 4) of version 3 or 4.
    NotAServerReply,
    /// The server is not synchronised (leap indicator 3, or stratum 0 /
    /// "kiss-o'-death", or above 15).
    Unsynchronised,
    /// The originate timestamp doesn't echo our request: a stray or forged
    /// packet.
    NotOurRequest,
    /// The transmit timestamp is before 1970.
    BadTime,
}

impl core::fmt::Display for ReplyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ReplyError::TooShort => "reply too short",
            ReplyError::NotAServerReply => "not an SNTP server reply",
            ReplyError::Unsynchronised => "server not synchronised",
            ReplyError::NotOurRequest => "reply doesn't answer our request",
            ReplyError::BadTime => "transmit time before 1970",
        })
    }
}

impl core::error::Error for ReplyError {}

/// A client request (version 4, mode 3) carrying `nonce` as its transmit
/// timestamp. The server echoes it as the originate timestamp, so the reply
/// can be matched to this request ([`parse_reply`]). Use a random nonce.
#[must_use]
pub fn request(nonce: [u8; 8]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = 0x23; // LI 0, VN 4, mode 3
    p[40..48].copy_from_slice(&nonce);
    p
}

/// Checks a server reply against the request sent with `nonce` and returns
/// the server's transmit time in Unix seconds.
///
/// # Errors
///
/// See [`ReplyError`].
pub fn parse_reply(reply: &[u8], nonce: [u8; 8]) -> Result<u64, ReplyError> {
    if reply.len() < PACKET_LEN {
        return Err(ReplyError::TooShort);
    }
    let (li, version, mode) = (reply[0] >> 6, (reply[0] >> 3) & 0x07, reply[0] & 0x07);
    if mode != 4 || !(3..=4).contains(&version) {
        return Err(ReplyError::NotAServerReply);
    }
    let stratum = reply[1];
    if li == 3 || !(1..=15).contains(&stratum) {
        return Err(ReplyError::Unsynchronised);
    }
    if reply[24..32] != nonce {
        return Err(ReplyError::NotOurRequest);
    }
    let seconds = u32::from_be_bytes([reply[40], reply[41], reply[42], reply[43]]);
    u64::from(seconds)
        .checked_sub(NTP_UNIX_OFFSET)
        .filter(|&s| s > 0)
        .ok_or(ReplyError::BadTime)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    /// 2026-01-01 00:00:00 UTC.
    const UNIX: u64 = 1_767_225_600;

    fn reply() -> [u8; PACKET_LEN] {
        let mut r = [0u8; PACKET_LEN];
        r[0] = 0x24; // LI 0, VN 4, mode 4
        r[1] = 2; // stratum
        r[24..32].copy_from_slice(&NONCE);
        let t = (UNIX + NTP_UNIX_OFFSET) as u32;
        r[40..44].copy_from_slice(&t.to_be_bytes());
        r
    }

    #[test]
    fn request_layout() {
        let q = request(NONCE);
        assert_eq!(q[0], 0x23);
        assert_eq!(&q[40..48], &NONCE);
        assert!(q[1..40].iter().all(|&b| b == 0));
    }

    #[test]
    fn good_reply() {
        assert_eq!(parse_reply(&reply(), NONCE), Ok(UNIX));
    }

    #[test]
    fn rejects() {
        assert_eq!(
            parse_reply(&reply()[..47], NONCE),
            Err(ReplyError::TooShort)
        );
        let mut r = reply();
        r[0] = 0x23; // a client packet
        assert_eq!(parse_reply(&r, NONCE), Err(ReplyError::NotAServerReply));
        let mut r = reply();
        r[0] = 0xE4; // LI 3: unsynchronised
        assert_eq!(parse_reply(&r, NONCE), Err(ReplyError::Unsynchronised));
        let mut r = reply();
        r[1] = 0; // kiss-o'-death
        assert_eq!(parse_reply(&r, NONCE), Err(ReplyError::Unsynchronised));
        assert_eq!(
            parse_reply(&reply(), [9; 8]),
            Err(ReplyError::NotOurRequest)
        );
        let mut r = reply();
        r[40..44].fill(0);
        assert_eq!(parse_reply(&r, NONCE), Err(ReplyError::BadTime));
    }

    #[test]
    fn version_3_servers_are_accepted() {
        let mut r = reply();
        r[0] = 0x1C; // LI 0, VN 3, mode 4
        assert_eq!(parse_reply(&r, NONCE), Ok(UNIX));
    }
}
