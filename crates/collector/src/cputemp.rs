//! Optional CPU temperature sensor reader: probes PawnIO (`\\.\PawnIO`) and Windows ACPI
//! Thermal Zone performance counters (`\Thermal Zone Information(*)\High Precision Temperature`
//! and `\Thermal Zone Information(*)\Temperature`). Silent when unavailable (`None`), never
//! adding to `source_errors`.
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

/// Checks whether the PawnIO kernel driver (`\\.\PawnIO`) is installed and accessible.
pub fn pawnio_installed() -> bool {
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

pub struct CpuTemp {
    query: Option<PDH_HQUERY>,
    hi_prec: Option<PDH_HCOUNTER>,
    kelvin: Option<PDH_HCOUNTER>,
    pawnio: Option<HANDLE>,
}

// SAFETY: PDH query/counter handles and device handles are only accessed by `&mut self` on the
// collector thread.
unsafe impl Send for CpuTemp {}

impl Default for CpuTemp {
    fn default() -> Self {
        Self::open()
    }
}

impl CpuTemp {
    pub fn open() -> Self {
        // SAFETY: optional probe of \\.\PawnIO.
        let pawnio = unsafe {
            CreateFileW(
                w!(r"\\.\PawnIO"),
                GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .ok()
        .filter(|h| !h.is_invalid());

        let mut query = PDH_HQUERY::default();
        // SAFETY: opens a dedicated PDH query for thermal zone sensors.
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return Self { query: None, hi_prec: None, kelvin: None, pawnio };
        }
        let add = |path: PCWSTR| -> Option<PDH_HCOUNTER> {
            let mut c = PDH_HCOUNTER::default();
            // SAFETY: adds English counter to `query`.
            (unsafe { PdhAddEnglishCounterW(query, path, 0, &mut c) } == 0).then_some(c)
        };
        let hi_prec = add(w!("\\Thermal Zone Information(*)\\High Precision Temperature"));
        let kelvin = add(w!("\\Thermal Zone Information(*)\\Temperature"));
        Self { query: Some(query), hi_prec, kelvin, pawnio }
    }

    pub fn has_pawnio(&self) -> bool {
        self.pawnio.is_some()
    }

    pub fn read(&mut self) -> Option<f32> {
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
        if let Some(h) = self.pawnio.take() {
            // SAFETY: closes PawnIO handle if opened.
            unsafe {
                let _ = CloseHandle(h);
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
}
