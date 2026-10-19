//! Optional CPU temperature sensor reader: probes PawnIO (`\\.\PawnIO` / `PawnIOLib.dll`) and Windows ACPI
//! Thermal Zone performance counters (`\Thermal Zone Information(*)\High Precision Temperature`
//! and `\Thermal Zone Information(*)\Temperature`). Silent when unavailable (`None`), never
//! adding to `source_errors`.
use windows::Win32::Foundation::{CloseHandle, FreeLibrary, GENERIC_READ, HANDLE, HMODULE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{HRESULT, PCSTR, PCWSTR, s, w};

static AMD_FAMILY17_BIN: &[u8] = include_bytes!("blobs/AMDFamily17.bin");
static INTEL_MSR_BIN: &[u8] = include_bytes!("blobs/IntelMSR.bin");

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CpuVendor {
    Amd,
    Intel,
    Unknown,
}

pub fn detect_cpu_vendor() -> CpuVendor {
    #[cfg(target_arch = "x86_64")]
    {
        let cpuid = std::arch::x86_64::__cpuid(0);
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&cpuid.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&cpuid.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&cpuid.ecx.to_le_bytes());
        if &vendor == b"AuthenticAMD" {
            CpuVendor::Amd
        } else if &vendor == b"GenuineIntel" {
            CpuVendor::Intel
        } else {
            CpuVendor::Unknown
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    CpuVendor::Unknown
}

/// Checks whether the PawnIO kernel driver or library is installed on the system.
pub fn pawnio_installed() -> bool {
    if std::path::Path::new(r"C:\Program Files\PawnIO\PawnIOLib.dll").exists() {
        return true;
    }
    // SAFETY: probes the PawnIO device object with read sharing; closed immediately if opened.
    let res = unsafe {
        CreateFileW(
            w!(r"\\.\PawnIO"),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match res {
        Ok(h) if !h.is_invalid() => {
            // SAFETY: valid handle from CreateFileW.
            unsafe {
                let _ = CloseHandle(h);
            }
            true
        }
        Err(e) => {
            // ERROR_ACCESS_DENIED (0x5) means the driver is installed and active, but requires elevation.
            e.code().0 == windows::Win32::Foundation::WIN32_ERROR(windows::Win32::Foundation::ERROR_ACCESS_DENIED.0).to_hresult().0
        }
        _ => false,
    }
}

/// Checks whether PawnIO is accessible without permission issues.
pub fn pawnio_accessible() -> bool {
    let res = unsafe {
        CreateFileW(
            w!(r"\\.\PawnIO"),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match res {
        Ok(h) if !h.is_invalid() => {
            unsafe {
                let _ = CloseHandle(h);
            }
            true
        }
        _ => false,
    }
}

/// Converts a raw ACPI thermal zone reading (either tenths of Kelvin from `High Precision
/// Temperature` or whole Kelvin from `Temperature`) into °C, filtering out dummy/unpopulated
/// zones outside `5.0..=125.0 °C`.
pub fn kelvin_raw_to_celsius(raw: f64, high_precision: bool) -> Option<f32> {
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    let kelvin = if high_precision || raw > 1000.0 { raw / 10.0 } else { raw };
    let c = (kelvin - 273.15) as f32;
    (5.0..=125.0).contains(&c).then_some(c)
}

struct PawnSession {
    dll: HMODULE,
    handle: HANDLE,
    pawnio_execute: unsafe extern "system" fn(HANDLE, PCSTR, *const u64, usize, *mut u64, usize, *mut usize) -> HRESULT,
    pawnio_close: unsafe extern "system" fn(HANDLE) -> HRESULT,
    vendor: CpuVendor,
}

// SAFETY: PawnIO handles and DLL pointers are only called by `&mut self` on the collector thread.
unsafe impl Send for PawnSession {}

impl PawnSession {
    pub fn open(vendor: CpuVendor) -> Option<Self> {
        if vendor == CpuVendor::Unknown {
            return None;
        }

        // Try standard PATH / local directory, then standard installation location.
        let dll = unsafe { LoadLibraryW(w!("PawnIOLib.dll")) }
            .or_else(|_| unsafe { LoadLibraryW(w!(r"C:\Program Files\PawnIO\PawnIOLib.dll")) })
            .ok()
            .filter(|h| !h.is_invalid())?;

        let get_proc = |name: PCSTR| -> Option<unsafe extern "system" fn() -> isize> {
            // SAFETY: retrieves procedure address by ASCII name.
            unsafe { GetProcAddress(dll, name) }
        };

        let open_fn = get_proc(s!("pawnio_open"));
        let load_fn = get_proc(s!("pawnio_load"));
        let exec_fn = get_proc(s!("pawnio_execute"));
        let close_fn = get_proc(s!("pawnio_close"));

        let (Some(open_fn), Some(load_fn), Some(exec_fn), Some(close_fn)) = (open_fn, load_fn, exec_fn, close_fn) else {
            let _ = unsafe { FreeLibrary(dll) };
            return None;
        };

        let pawnio_open: unsafe extern "system" fn(*mut HANDLE) -> HRESULT = unsafe { std::mem::transmute(open_fn) };
        let pawnio_load: unsafe extern "system" fn(HANDLE, *const u8, usize) -> HRESULT = unsafe { std::mem::transmute(load_fn) };
        let pawnio_execute: unsafe extern "system" fn(HANDLE, PCSTR, *const u64, usize, *mut u64, usize, *mut usize) -> HRESULT = unsafe { std::mem::transmute(exec_fn) };
        let pawnio_close: unsafe extern "system" fn(HANDLE) -> HRESULT = unsafe { std::mem::transmute(close_fn) };

        let mut handle = HANDLE::default();
        // SAFETY: pawnio_open opens a driver session handle.
        let hr = unsafe { pawnio_open(&mut handle) };
        if hr.is_err() || handle.is_invalid() {
            let _ = unsafe { FreeLibrary(dll) };
            return None;
        }

        let blob = match vendor {
            CpuVendor::Amd => AMD_FAMILY17_BIN,
            CpuVendor::Intel => INTEL_MSR_BIN,
            CpuVendor::Unknown => unreachable!(),
        };

        // SAFETY: pawnio_load registers the hardware-access module.
        let hr = unsafe { pawnio_load(handle, blob.as_ptr(), blob.len()) };
        if hr.is_err() {
            let _ = unsafe { pawnio_close(handle) };
            let _ = unsafe { FreeLibrary(dll) };
            return None;
        }

        Some(Self {
            dll,
            handle,
            pawnio_execute,
            pawnio_close,
            vendor,
        })
    }

    pub fn read_temp(&self) -> Option<f32> {
        match self.vendor {
            CpuVendor::Amd => {
                // AMD Zen SMN register THM_TCON_CUR_TMP = 0x00059800.
                let input = [0x00059800u64];
                let mut out = [0u64; 1];
                let mut ret_size = 0usize;
                // SAFETY: calls ioctl_read_smn from AMDFamily17 module.
                let hr = unsafe {
                    (self.pawnio_execute)(
                        self.handle,
                        s!("ioctl_read_smn"),
                        input.as_ptr(),
                        1,
                        out.as_mut_ptr(),
                        1,
                        &mut ret_size,
                    )
                };
                if hr.is_ok() && ret_size >= 1 {
                    let raw = out[0] as u32;
                    let cur_temp = ((raw >> 21) & 0x7FF) as f32 * 0.125f32;
                    if (5.0..=125.0).contains(&cur_temp) {
                        return Some(cur_temp);
                    }
                }
                None
            }
            CpuVendor::Intel => {
                // Read TjMax from MSR_IA32_TEMPERATURE_TARGET (0x1A2).
                let mut tj_max = 100.0f32;
                let mut out = [0u64; 1];
                let mut ret_size = 0usize;
                // SAFETY: calls ioctl_read_msr from IntelMSR module.
                let hr = unsafe {
                    (self.pawnio_execute)(
                        self.handle,
                        s!("ioctl_read_msr"),
                        [0x1A2u64].as_ptr(),
                        1,
                        out.as_mut_ptr(),
                        1,
                        &mut ret_size,
                    )
                };
                if hr.is_ok() && ret_size >= 1 {
                    let target = ((out[0] >> 16) & 0xFF) as f32;
                    if (70.0..=115.0).contains(&target) {
                        tj_max = target;
                    }
                }

                // Read package temperature margin from MSR_IA32_PACKAGE_THERM_STATUS (0x1B1)
                // or fallback to core thermal status MSR_IA32_THERM_STATUS (0x19C).
                let mut hr = unsafe {
                    (self.pawnio_execute)(
                        self.handle,
                        s!("ioctl_read_msr"),
                        [0x1B1u64].as_ptr(),
                        1,
                        out.as_mut_ptr(),
                        1,
                        &mut ret_size,
                    )
                };
                if hr.is_err() {
                    hr = unsafe {
                        (self.pawnio_execute)(
                            self.handle,
                            s!("ioctl_read_msr"),
                            [0x19Cu64].as_ptr(),
                            1,
                            out.as_mut_ptr(),
                            1,
                            &mut ret_size,
                        )
                    };
                }
                if hr.is_ok() && ret_size >= 1 {
                    let delta = ((out[0] >> 16) & 0x7F) as f32;
                    let temp = tj_max - delta;
                    if (5.0..=125.0).contains(&temp) {
                        return Some(temp);
                    }
                }
                None
            }
            CpuVendor::Unknown => None,
        }
    }
}

impl Drop for PawnSession {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: closes PawnIO session handle.
            unsafe {
                let _ = (self.pawnio_close)(self.handle);
            }
        }
        if !self.dll.is_invalid() {
            // SAFETY: unloads PawnIOLib.dll.
            unsafe {
                let _ = FreeLibrary(self.dll);
            }
        }
    }
}

pub struct CpuTemp {
    query: Option<PDH_HQUERY>,
    hi_prec: Option<PDH_HCOUNTER>,
    kelvin: Option<PDH_HCOUNTER>,
    pawnio: Option<PawnSession>,
    vendor: CpuVendor,
}

// SAFETY: PDH query/counter handles and driver sessions are only accessed by `&mut self` on the
// collector thread.
unsafe impl Send for CpuTemp {}

impl Default for CpuTemp {
    fn default() -> Self {
        Self::open()
    }
}

impl CpuTemp {
    pub fn open() -> Self {
        let vendor = detect_cpu_vendor();
        let pawnio = PawnSession::open(vendor);

        let mut query = PDH_HQUERY::default();
        // SAFETY: opens a dedicated PDH query for thermal zone sensors.
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return Self { query: None, hi_prec: None, kelvin: None, pawnio, vendor };
        }
        let add = |path: PCWSTR| -> Option<PDH_HCOUNTER> {
            let mut c = PDH_HCOUNTER::default();
            // SAFETY: adds English counter to `query`.
            (unsafe { PdhAddEnglishCounterW(query, path, 0, &mut c) } == 0).then_some(c)
        };
        let hi_prec = add(w!("\\Thermal Zone Information(*)\\High Precision Temperature"));
        let kelvin = add(w!("\\Thermal Zone Information(*)\\Temperature"));
        Self { query: Some(query), hi_prec, kelvin, pawnio, vendor }
    }

    pub fn has_pawnio(&self) -> bool {
        self.pawnio.is_some()
    }

    pub fn vendor(&self) -> CpuVendor {
        self.vendor
    }

    pub fn read(&mut self) -> Option<f32> {
        if let Some(session) = &self.pawnio
            && let Some(temp) = session.read_temp()
        {
            return Some(temp);
        }

        let query = self.query?;
        // SAFETY: collects thermal zone query data.
        if unsafe { PdhCollectQueryData(query) } != 0 {
            return None;
        }
        if let Some(c) = self.hi_prec
            && let Some(temp) = read_max_zone(c, true)
        {
            return Some(temp);
        }
        if let Some(c) = self.kelvin
            && let Some(temp) = read_max_zone(c, false)
        {
            return Some(temp);
        }
        None
    }
}

fn read_max_zone(counter: PDH_HCOUNTER, high_precision: bool) -> Option<f32> {
    let mut bytes = 0u32;
    let mut count = 0u32;
    // SAFETY: size query with null buffer.
    let status = unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None) };
    if status != PDH_MORE_DATA || bytes == 0 || count == 0 {
        return None;
    }
    let cap = (bytes as usize).div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()) + 4;
    let mut buf = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); cap];
    bytes = (buf.len() * size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()) as u32;
    // SAFETY: `buf` is sized for `bytes`.
    if unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, Some(buf.as_mut_ptr())) } != 0 {
        return None;
    }
    let mut best: Option<f32> = None;
    for item in &buf[..count as usize] {
        if item.FmtValue.CStatus <= PDH_CSTATUS_NEW_DATA {
            // SAFETY: union holds doubleValue when formatted with PDH_FMT_DOUBLE.
            let raw = unsafe { item.FmtValue.Anonymous.doubleValue };
            if let Some(c) = kelvin_raw_to_celsius(raw, high_precision) {
                best = Some(best.map_or(c, |b| b.max(c)));
            }
        }
    }
    best
}

impl Drop for CpuTemp {
    fn drop(&mut self) {
        if let Some(q) = self.query.take() {
            // SAFETY: closes PDH query opened in `CpuTemp::open`.
            unsafe {
                let _ = PdhCloseQuery(q);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_kelvin_and_tenths_of_kelvin_to_celsius_within_bounds() {
        // 338.15 K = 65.0 °C
        let c = kelvin_raw_to_celsius(3381.5, true).unwrap();
        assert!((c - 65.0).abs() < 0.05);
        let c2 = kelvin_raw_to_celsius(338.15, false).unwrap();
        assert!((c2 - 65.0).abs() < 0.05);
        // Unpopulated 0 K or 273.15 K (0 °C) or absurd 500 °C are rejected.
        assert_eq!(kelvin_raw_to_celsius(0.0, true), None);
        assert_eq!(kelvin_raw_to_celsius(2731.5, true), None);
        assert_eq!(kelvin_raw_to_celsius(8000.0, true), None);
    }

    #[test]
    fn detects_x86_vendor() {
        let v = detect_cpu_vendor();
        assert!(matches!(v, CpuVendor::Amd | CpuVendor::Intel), "detected vendor: {v:?}");
    }

    #[test]
    fn pawnio_installed_and_open_reports_safely() {
        let installed = pawnio_installed();
        let mut temp = CpuTemp::open();
        let reading = temp.read();
        println!("pawnio_installed: {installed}, has_pawnio: {}, reading: {reading:?}", temp.has_pawnio());
    }
}
