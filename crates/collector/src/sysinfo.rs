//! Facts about the machine that don't change while Negotium runs, read once at startup.
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::SystemInformation::{FIRMWARE_TABLE_PROVIDER, GetSystemFirmwareTable};
use windows::core::w;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemInfo {
    /// "AMD Ryzen 9 7950X 16-Core Processor".
    pub cpu_name: Option<String>,
    /// Configured memory speed in MT/s (SMBIOS), the fastest installed module.
    pub memory_speed_mts: Option<u32>,
}

pub fn read_system_info() -> SystemInfo {
    SystemInfo { cpu_name: cpu_name(), memory_speed_mts: memory_speed_mts() }
}

pub fn cpu_name() -> Option<String> {
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf` has `size` writable bytes for a REG_SZ value; `size` is updated to the bytes written.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size as *mut u32),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let units = &buf[..(size as usize / 2).min(buf.len())];
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    let name = String::from_utf16_lossy(&units[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

pub fn memory_speed_mts() -> Option<u32> {
    let provider = FIRMWARE_TABLE_PROVIDER(u32::from_be_bytes(*b"RSMB"));
    // SAFETY: size query with no buffer.
    let size = unsafe { GetSystemFirmwareTable(provider, 0, None) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` has `size` writable bytes.
    let got = unsafe { GetSystemFirmwareTable(provider, 0, Some(buf.as_mut_slice())) };
    if got == 0 || got > size {
        return None;
    }
    parse_smbios_memory_speed(&buf[..got as usize])
}

/// Walks a RawSMBIOSData blob (4 version bytes, a u32 table length, the table) for type-17 Memory
/// Device structures and returns the highest configured speed (offset 0x20), falling back to the
/// rated speed (0x15), over installed modules (size at 0x0C nonzero). Never panics on short input.
pub fn parse_smbios_memory_speed(raw: &[u8]) -> Option<u32> {
    let len = u32::from_le_bytes(raw.get(4..8)?.try_into().ok()?) as usize;
    let table = raw.get(8..8usize.checked_add(len)?).or_else(|| raw.get(8..))?;
    let mut best: Option<u32> = None;
    let mut i = 0usize;
    while i + 4 <= table.len() {
        let (kind, flen) = (table[i], table[i + 1] as usize);
        if flen < 4 || i + flen > table.len() {
            break;
        }
        let f = &table[i..i + flen];
        if kind == 17 {
            let word = |o: usize| f.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
            let usable = |s: &u16| *s != 0 && *s != 0xFFFF;
            let installed = word(0x0C).is_some_and(|s| s != 0);
            let speed = word(0x20).filter(usable).or_else(|| word(0x15).filter(usable));
            if installed && let Some(s) = speed {
                best = Some(best.map_or(s as u32, |b| b.max(s as u32)));
            }
        }
        if kind == 127 {
            break;
        }
        // Skip the formatted area, then the string set, which ends with two NULs.
        let mut j = i + flen;
        while j + 1 < table.len() && !(table[j] == 0 && table[j + 1] == 0) {
            j += 1;
        }
        i = j + 2;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One SMBIOS type-17 (Memory Device) structure: size in MB, speed, configured speed.
    fn device(size_mb: u16, speed: u16, configured: u16) -> Vec<u8> {
        let mut f = vec![0u8; 0x22];
        f[0] = 17;
        f[1] = 0x22;
        f[0x0C..0x0E].copy_from_slice(&size_mb.to_le_bytes());
        f[0x15..0x17].copy_from_slice(&speed.to_le_bytes());
        f[0x20..0x22].copy_from_slice(&configured.to_le_bytes());
        f.extend_from_slice(b"DIMM A\0\0");
        f
    }

    /// RawSMBIOSData: four version bytes, the table length, then the table.
    fn raw(structures: &[Vec<u8>]) -> Vec<u8> {
        let table = structures.concat();
        let mut out = vec![0, 3, 4, 0];
        out.extend_from_slice(&(table.len() as u32).to_le_bytes());
        out.extend(table);
        out
    }

    const END: [u8; 6] = [127, 4, 0, 0, 0, 0];

    #[test]
    fn memory_speed_prefers_configured_speed_over_rated_speed() {
        let r = raw(&[device(16384, 6400, 6000), device(16384, 6400, 6000), END.to_vec()]);
        assert_eq!(parse_smbios_memory_speed(&r), Some(6000));
    }

    #[test]
    fn memory_speed_falls_back_to_rated_speed_and_skips_empty_slots() {
        let r = raw(&[device(0, 4800, 4800), device(8192, 3200, 0), END.to_vec()]);
        assert_eq!(parse_smbios_memory_speed(&r), Some(3200));
    }

    #[test]
    fn memory_speed_is_none_without_devices_and_never_panics_on_truncation() {
        assert_eq!(parse_smbios_memory_speed(&raw(&[END.to_vec()])), None);
        assert_eq!(parse_smbios_memory_speed(&[]), None);
        let full = raw(&[device(8192, 3200, 3200)]);
        for cut in 0..full.len() {
            let _ = parse_smbios_memory_speed(&full[..cut]);
        }
    }
}
