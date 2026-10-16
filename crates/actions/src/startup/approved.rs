//! The StartupApproved values Task Manager reads and writes (ruling P3b-2). Each is REG_BINARY,
//! 12 bytes: a little-endian DWORD of flags whose low bit is set while the entry is disabled
//! (Windows writes 0x02 / 0x03; some installers 0x06 / 0x07), then a little-endian FILETIME (UTC)
//! of when it was disabled, zero while enabled. A missing value, or one too short to hold the
//! flags, means enabled.
pub const APPROVED_LEN: usize = 12;
const DEFAULT_FLAGS: u32 = 0x2;
const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;

pub fn is_enabled(bytes: &[u8]) -> bool {
    bytes.len() < 4 || bytes[0] & 1 == 0
}

/// The value to write: `previous`'s other flag bits kept (0x02 when there are none), the low bit
/// set when disabling together with `now_filetime`, and zeros when enabling.
pub fn encode_approved(previous: Option<&[u8]>, enabled: bool, now_filetime: u64) -> [u8; APPROVED_LEN] {
    let old = previous
        .filter(|b| b.len() >= 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(DEFAULT_FLAGS);
    let base = match old & !1 {
        0 => DEFAULT_FLAGS,
        other => other,
    };
    let mut out = [0u8; APPROVED_LEN];
    out[..4].copy_from_slice(&(if enabled { base } else { base | 1 }).to_le_bytes());
    if !enabled {
        out[4..].copy_from_slice(&now_filetime.to_le_bytes());
    }
    out
}

/// The current time as a FILETIME: 100 ns intervals since 1601-01-01 UTC.
pub fn filetime_now() -> u64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    UNIX_EPOCH_AS_FILETIME + (since.as_nanos() / 100) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_low_flag_bit_means_disabled_and_a_missing_or_short_value_means_enabled() {
        assert!(is_enabled(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_enabled(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]));
        assert!(is_enabled(&[6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_enabled(&[7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(is_enabled(&[]));
        assert!(is_enabled(&[3, 0]), "too short to carry flags");
    }

    #[test]
    fn disabling_writes_the_flag_and_the_time_enabling_clears_both() {
        assert_eq!(
            encode_approved(None, false, 0x01D9_ABCD_1234_5678),
            [3, 0, 0, 0, 0x78, 0x56, 0x34, 0x12, 0xCD, 0xAB, 0xD9, 0x01]
        );
        assert_eq!(encode_approved(Some(&[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]), true, 99), [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn other_flag_bits_are_kept() {
        assert_eq!(encode_approved(Some(&[7, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1]), true, 5)[..4], [6, 0, 0, 0]);
        assert_eq!(encode_approved(Some(&[6, 0, 0, 0]), false, 5), [7, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(encode_approved(Some(&[1, 0, 0, 0]), true, 5)[..4], [2, 0, 0, 0], "no other bits: the default");
        assert_eq!(encode_approved(Some(&[3, 0]), false, 5)[..4], [3, 0, 0, 0], "too short: the default");
    }

    #[test]
    fn now_is_a_plausible_filetime() {
        assert!(filetime_now() > 0x01D0_0000_0000_0000, "after 2014");
    }
}
