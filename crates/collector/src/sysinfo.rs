//! Facts about the machine that don't change while Negotium runs, read once at startup.
use std::collections::HashMap;
use std::mem::size_of;
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, GENERIC_READ};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::SystemInformation::{
    FIRMWARE_TABLE_PROVIDER, GetLogicalProcessorInformationEx, GetSystemFirmwareTable, GetSystemWindowsDirectoryW,
    LOGICAL_PROCESSOR_RELATIONSHIP, RelationCache, RelationProcessorCore, RelationProcessorPackage,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::Win32::System::Threading::{IsProcessorFeaturePresent, PF_VIRT_FIRMWARE_ENABLED};
use windows::core::{HSTRING, PCWSTR, w};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskInfo {
    /// PhysicalDisk number (e.g. 0 for `\\.\PhysicalDrive0`).
    pub number: u32,
    /// Product model name ("Samsung SSD 990 PRO 2TB").
    pub model: Option<String>,
    /// Short media & bus label ("SSD (NVMe)", "SSD (SATA)", "HDD (SATA)", "SSD", "HDD", "USB").
    pub kind: Option<String>,
    /// Total physical disk capacity in bytes.
    pub capacity_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryHardware {
    /// Configured memory speed in MT/s (SMBIOS), the fastest installed module.
    pub speed_mts: Option<u32>,
    /// (installed modules, total slots) from SMBIOS Type 17.
    pub slots_used: Option<(u32, u32)>,
    /// Form factor of installed modules ("DIMM", "SODIMM", "Row of chips").
    pub form_factor: Option<String>,
    /// Sum of installed module sizes in bytes (used to compute hardware reserved RAM).
    pub installed_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuTopology {
    /// Physical processor packages (sockets).
    pub sockets: Option<u32>,
    /// Physical processor cores.
    pub cores: Option<u32>,
    /// Total L1 cache in bytes across cores.
    pub l1_bytes: Option<u64>,
    /// Total L2 cache in bytes.
    pub l2_bytes: Option<u64>,
    /// Total L3 cache in bytes.
    pub l3_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemInfo {
    /// "AMD Ryzen 9 7950X 16-Core Processor".
    pub cpu_name: Option<String>,
    /// Base frequency in MHz from the processor registry key (`~MHz`).
    pub cpu_base_mhz: Option<u32>,
    /// Sockets, physical cores, and L1/L2/L3 cache totals.
    pub cpu_topology: CpuTopology,
    /// Firmware virtualization enabled.
    pub cpu_virtualization: Option<bool>,
    /// Configured memory speed in MT/s (SMBIOS), the fastest installed module.
    pub memory_speed_mts: Option<u32>,
    /// Additional SMBIOS memory facts (slots used, form factor, installed physical bytes).
    pub memory_hw: MemoryHardware,
    /// Physical disks discovered via `\\.\PhysicalDriveN`.
    pub disks: Vec<DiskInfo>,
    /// Drive letter of the Windows installation (e.g. `"C:"`).
    pub system_drive: String,
    /// Hardware GPUs in DXGI order.
    pub gpus: Vec<crate::gpu::GpuInfo>,
    /// Why GPU enumeration was incomplete (reported as source errors on every tick).
    pub gpu_errors: Vec<String>,
}

/// The machine's Windows directory, from Windows itself (`GetSystemWindowsDirectoryW`) rather than
/// `%SystemRoot%`: the user can set that variable (HKCU\Environment) to anything, and an elevated
/// copy must never start, or classify by, a folder the user picked. `C:\Windows` if Windows
/// doesn't say.
pub fn windows_dir() -> String {
    let mut buf = [0u16; 260];
    // SAFETY: writes at most `buf.len()` UTF-16 units into `buf` and returns how many (without the
    // NUL), or the size it needs (larger than `buf`) or 0 on failure.
    let n = unsafe { GetSystemWindowsDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return r"C:\Windows".to_string();
    }
    String::from_utf16(&buf[..n]).unwrap_or_else(|_| r"C:\Windows".to_string())
}

pub fn read_system_info() -> SystemInfo {
    let (gpus, gpu_errors) = crate::gpu::enumerate_gpus();
    let memory_hw = read_smbios_memory();
    let win_dir = windows_dir();
    let system_drive = win_dir.get(..2).unwrap_or("C:").to_uppercase();
    // SAFETY: plain query with no pointers.
    let virt = unsafe { IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED) }.as_bool();
    SystemInfo {
        cpu_name: cpu_name(),
        cpu_base_mhz: cpu_base_mhz(),
        cpu_topology: cpu_topology(),
        cpu_virtualization: Some(virt),
        memory_speed_mts: memory_hw.speed_mts,
        memory_hw,
        disks: enumerate_disks(),
        system_drive,
        gpus,
        gpu_errors,
    }
}

fn reg_sz(subkey: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: `buf` has `size` writable bytes for a REG_SZ value; `size` is updated to the bytes written.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey,
            value,
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
    let text = String::from_utf16_lossy(&units[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn cpu_name() -> Option<String> {
    reg_sz(w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"), w!("ProcessorNameString"))
}

pub fn cpu_base_mhz() -> Option<u32> {
    let mut mhz = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `mhz` is a valid u32 buffer for a REG_DWORD value.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"),
            w!("~MHz"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut mhz as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS && mhz > 0).then_some(mhz)
}

fn raw_proc_info(rel: LOGICAL_PROCESSOR_RELATIONSHIP) -> Option<Vec<u8>> {
    let mut len = 0u32;
    // SAFETY: size query with null buffer.
    let _ = unsafe { GetLogicalProcessorInformationEx(rel, None, &mut len) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` has `len` writable bytes.
    unsafe { GetLogicalProcessorInformationEx(rel, Some(buf.as_mut_ptr().cast()), &mut len) }.ok()?;
    buf.truncate(len as usize);
    Some(buf)
}

pub fn cpu_topology() -> CpuTopology {
    let count_records = |rel: LOGICAL_PROCESSOR_RELATIONSHIP| -> Option<u32> {
        let buf = raw_proc_info(rel)?;
        let mut count = 0u32;
        let mut offset = 0usize;
        while offset + 8 <= buf.len() {
            let size = u32::from_ne_bytes(buf[offset + 4..offset + 8].try_into().ok()?) as usize;
            if size < 8 || offset + size > buf.len() {
                break;
            }
            count += 1;
            offset += size;
        }
        (count > 0).then_some(count)
    };
    let mut l1 = 0u64;
    let mut l2 = 0u64;
    let mut l3 = 0u64;
    if let Some(buf) = raw_proc_info(RelationCache) {
        let mut offset = 0usize;
        while offset + size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() <= buf.len() {
            // SAFETY: `buf[offset..]` holds a valid `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` header.
            let info = unsafe { &*(buf.as_ptr().add(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX) };
            let size = info.Size as usize;
            if size < 8 || offset + size > buf.len() {
                break;
            }
            if info.Relationship == RelationCache {
                // SAFETY: `Relationship == RelationCache` selects the `Cache` union member.
                let cache = unsafe { &info.Anonymous.Cache };
                match cache.Level {
                    1 => l1 += cache.CacheSize as u64,
                    2 => l2 += cache.CacheSize as u64,
                    3 => l3 += cache.CacheSize as u64,
                    _ => {}
                }
            }
            offset += size;
        }
    }
    CpuTopology {
        sockets: count_records(RelationProcessorPackage),
        cores: count_records(RelationProcessorCore),
        l1_bytes: (l1 > 0).then_some(l1),
        l2_bytes: (l2 > 0).then_some(l2),
        l3_bytes: (l3 > 0).then_some(l3),
    }
}

/// Reads display adapter class registry keys (`{4d36e968-e325-11ce-bfc1-08002be10318}\0000..0031`)
/// for `(DriverVersion, DriverDate)` keyed by lowercase `DriverDesc`.
pub fn gpu_driver_info() -> HashMap<String, (Option<String>, Option<String>)> {
    let mut out = HashMap::new();
    for i in 0..32u32 {
        let key = HSTRING::from(format!(
            r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{i:04}"
        ));
        let Some(desc) = reg_sz(PCWSTR(key.as_ptr()), w!("DriverDesc")) else { continue };
        let ver = reg_sz(PCWSTR(key.as_ptr()), w!("DriverVersion"));
        let date = reg_sz(PCWSTR(key.as_ptr()), w!("DriverDate"));
        out.entry(desc.to_lowercase()).or_insert((ver, date));
    }
    out
}

pub fn memory_speed_mts() -> Option<u32> {
    read_smbios_memory().speed_mts
}

pub fn read_smbios_memory() -> MemoryHardware {
    let provider = FIRMWARE_TABLE_PROVIDER(u32::from_be_bytes(*b"RSMB"));
    // SAFETY: size query with no buffer.
    let size = unsafe { GetSystemFirmwareTable(provider, 0, None) };
    if size == 0 {
        return MemoryHardware::default();
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` has `size` writable bytes.
    let got = unsafe { GetSystemFirmwareTable(provider, 0, Some(buf.as_mut_slice())) };
    if got == 0 || got > size {
        return MemoryHardware::default();
    }
    parse_smbios_memory(&buf[..got as usize])
}

fn smbios_form_factor(code: u8) -> Option<&'static str> {
    match code {
        0x08 => Some("DIMM"),
        0x09 => Some("TSOP"),
        0x0B => Some("Row of chips"),
        0x0C => Some("RIMM"),
        0x0D => Some("SODIMM"),
        0x0E => Some("SRIMM"),
        0x0F => Some("FB-DIMM"),
        _ => None,
    }
}

/// Walks a RawSMBIOSData blob for type-17 Memory Device structures.
pub fn parse_smbios_memory(raw: &[u8]) -> MemoryHardware {
    let Some(len_bytes) = raw.get(4..8).and_then(|b| b.try_into().ok()) else {
        return MemoryHardware::default();
    };
    let len = u32::from_le_bytes(len_bytes) as usize;
    let Some(table) = raw.get(8..8usize.checked_add(len).unwrap_or(usize::MAX)).or_else(|| raw.get(8..)) else {
        return MemoryHardware::default();
    };
    let mut best_speed: Option<u32> = None;
    let mut total_slots = 0u32;
    let mut used_slots = 0u32;
    let mut form_factor: Option<String> = None;
    let mut installed_bytes = 0u64;
    let mut i = 0usize;
    while i + 4 <= table.len() {
        let (kind, flen) = (table[i], table[i + 1] as usize);
        if flen < 4 || i + flen > table.len() {
            break;
        }
        let f = &table[i..i + flen];
        if kind == 17 {
            total_slots += 1;
            let word = |o: usize| f.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
            let dword = |o: usize| f.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let usable = |s: &u16| *s != 0 && *s != 0xFFFF;
            let raw_size = word(0x0C).unwrap_or(0);
            if raw_size != 0 && raw_size != 0xFFFF {
                used_slots += 1;
                let mb = if raw_size == 0x7FFF {
                    (dword(0x1C).unwrap_or(0) & 0x7FFF_FFFF) as u64
                } else if raw_size & 0x8000 != 0 {
                    (raw_size & 0x7FFF) as u64 / 1024
                } else {
                    raw_size as u64
                };
                installed_bytes = installed_bytes.saturating_add(mb * 1_048_576);
                let speed = word(0x20).filter(usable).or_else(|| word(0x15).filter(usable));
                if let Some(s) = speed {
                    best_speed = Some(best_speed.map_or(s as u32, |b| b.max(s as u32)));
                }
                if form_factor.is_none()
                    && let Some(&code) = f.get(0x0E)
                    && let Some(ff) = smbios_form_factor(code)
                {
                    form_factor = Some(ff.to_string());
                }
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
    MemoryHardware {
        speed_mts: best_speed,
        slots_used: (total_slots > 0 && used_slots > 0).then_some((used_slots, total_slots)),
        form_factor,
        installed_bytes: (installed_bytes > 0).then_some(installed_bytes),
    }
}

/// Walks a RawSMBIOSData blob (4 version bytes, a u32 table length, the table) for type-17 Memory
/// Device structures and returns the highest configured speed (offset 0x20), falling back to the
/// rated speed (0x15), over installed modules (size at 0x0C nonzero). Never panics on short input.
pub fn parse_smbios_memory_speed(raw: &[u8]) -> Option<u32> {
    parse_smbios_memory(raw).speed_mts
}

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;

#[repr(C)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
    additional: [u8; 4],
}

fn c_string_at(buf: &[u8], offset: usize) -> Option<String> {
    if offset == 0 || offset >= buf.len() {
        return None;
    }
    let slice = &buf[offset..];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    let s = String::from_utf8_lossy(&slice[..end]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn bus_type_name(bus: u8) -> Option<&'static str> {
    match bus {
        0x07 => Some("USB"),
        0x08 => Some("RAID"),
        0x0B => Some("SATA"),
        0x0A => Some("SAS"),
        0x11 => Some("NVMe"),
        0x10 => Some("Storage Spaces"),
        _ => None,
    }
}

fn query_physical_drive(number: u32) -> Option<DiskInfo> {
    let path = HSTRING::from(format!(r"\\.\PhysicalDrive{number}"));
    // SAFETY: opens device with 0 desired access (metadata query only; works without elevation).
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .ok()?;

    // 1. StorageDeviceProperty (PropertyId = 0) -> model name and BusType.
    let q_dev = StoragePropertyQuery { property_id: 0, query_type: 0, additional: [0; 4] };
    let mut dev_buf = [0u8; 512];
    let mut returned = 0u32;
    // SAFETY: valid device handle and buffers for IOCTL_STORAGE_QUERY_PROPERTY.
    let dev_ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&q_dev as *const _ as *const _),
            size_of::<StoragePropertyQuery>() as u32,
            Some(dev_buf.as_mut_ptr().cast()),
            dev_buf.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok()
        && returned >= 32;

    let (model, bus) = if dev_ok {
        let vendor_off = u32::from_le_bytes(dev_buf[12..16].try_into().unwrap_or([0; 4])) as usize;
        let prod_off = u32::from_le_bytes(dev_buf[16..20].try_into().unwrap_or([0; 4])) as usize;
        let bus_type = dev_buf[28];
        let vendor = c_string_at(&dev_buf[..returned as usize], vendor_off);
        let prod = c_string_at(&dev_buf[..returned as usize], prod_off);
        let model = match (vendor, prod) {
            (Some(v), Some(p)) if !p.to_lowercase().starts_with(&v.to_lowercase()) => Some(format!("{v} {p}")),
            (_, Some(p)) => Some(p),
            (Some(v), None) => Some(v),
            (None, None) => None,
        };
        (model, Some(bus_type))
    } else {
        (None, None)
    };

    // 2. StorageDeviceSeekPenaltyProperty (PropertyId = 7) -> SSD vs HDD.
    let q_seek = StoragePropertyQuery { property_id: 7, query_type: 0, additional: [0; 4] };
    let mut seek_buf = [0u8; 16];
    let mut seek_ret = 0u32;
    // SAFETY: valid device handle and buffers for StorageDeviceSeekPenaltyProperty.
    let incurs_seek = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&q_seek as *const _ as *const _),
            size_of::<StoragePropertyQuery>() as u32,
            Some(seek_buf.as_mut_ptr().cast()),
            seek_buf.len() as u32,
            Some(&mut seek_ret),
            None,
        )
    }
    .is_ok()
        .then(|| seek_ret >= 9)
        .filter(|&ok| ok)
        .map(|_| seek_buf[8] != 0);

    let media = match (incurs_seek, bus) {
        (Some(false), _) | (None, Some(0x11)) => Some("SSD"),
        (Some(true), _) => Some("HDD"),
        _ => None,
    };
    let kind = match (media, bus.and_then(bus_type_name)) {
        (Some(m), Some(b)) => Some(format!("{m} ({b})")),
        (Some(m), None) => Some(m.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
    };

    // 3. Capacity via IOCTL_DISK_GET_LENGTH_INFO (requires GENERIC_READ on some systems; try 0 first, then GENERIC_READ).
    let mut cap_buf = [0u8; 8];
    let mut cap_ret = 0u32;
    // SAFETY: valid device handle and 8-byte GET_LENGTH_INFORMATION output buffer.
    let mut capacity_bytes = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_LENGTH_INFO,
            None,
            0,
            Some(cap_buf.as_mut_ptr().cast()),
            8,
            Some(&mut cap_ret),
            None,
        )
    }
    .is_ok()
    .then(|| u64::from_le_bytes(cap_buf))
    .filter(|&c| c > 0);

    // SAFETY: closes handle opened above.
    unsafe {
        let _ = CloseHandle(handle);
    }

    if capacity_bytes.is_none()
        && let Ok(r_handle) = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
    {
        // SAFETY: valid read handle and 8-byte output buffer.
        if unsafe {
            DeviceIoControl(
                r_handle,
                IOCTL_DISK_GET_LENGTH_INFO,
                None,
                0,
                Some(cap_buf.as_mut_ptr().cast()),
                8,
                Some(&mut cap_ret),
                None,
            )
        }
        .is_ok()
        {
            let c = u64::from_le_bytes(cap_buf);
            if c > 0 {
                capacity_bytes = Some(c);
            }
        }
        // SAFETY: closes read handle opened above.
        unsafe {
            let _ = CloseHandle(r_handle);
        }
    }

    Some(DiskInfo { number, model, kind, capacity_bytes })
}

pub fn enumerate_disks() -> Vec<DiskInfo> {
    (0..16).filter_map(query_physical_drive).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_dir_is_the_real_windows_directory() {
        let dir = std::path::PathBuf::from(windows_dir());
        assert!(dir.is_absolute() && dir.join(r"System32\kernel32.dll").is_file(), "{}", dir.display());
    }

    /// One SMBIOS type-17 (Memory Device) structure: size in MB, speed, configured speed.
    fn device(size_mb: u16, speed: u16, configured: u16) -> Vec<u8> {
        let mut f = vec![0u8; 0x22];
        f[0] = 17;
        f[1] = 0x22;
        f[0x0C..0x0E].copy_from_slice(&size_mb.to_le_bytes());
        f[0x0E] = 0x08; // DIMM
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
        let hw = parse_smbios_memory(&r);
        assert_eq!(hw.speed_mts, Some(6000));
        assert_eq!(hw.slots_used, Some((2, 2)));
        assert_eq!(hw.form_factor.as_deref(), Some("DIMM"));
        assert_eq!(hw.installed_bytes, Some(32 * 1024 * 1_048_576));
    }

    #[test]
    fn memory_speed_falls_back_to_rated_speed_and_skips_empty_slots() {
        let r = raw(&[device(0, 4800, 4800), device(8192, 3200, 0), END.to_vec()]);
        let hw = parse_smbios_memory(&r);
        assert_eq!(hw.speed_mts, Some(3200));
        assert_eq!(hw.slots_used, Some((1, 2)));
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

    #[test]
    fn cpu_topology_and_disks_are_discovered_on_live_machine() {
        let info = read_system_info();
        assert!(info.cpu_topology.cores.unwrap_or(0) >= 1);
        assert!(info.cpu_topology.sockets.unwrap_or(0) >= 1);
        assert!(!info.disks.is_empty(), "at least one physical drive");
        assert!(info.disks.iter().any(|d| d.kind.is_some()), "{:?}", info.disks);
    }
}
