//! GPU adapters from DXGI (name, memory sizes, LUID) and their temperature from the kernel
//! adapter performance data (D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA), as Task Manager.
//! Every failure comes back as a message for `source_errors`; none of them stops the other data.
use std::cell::Cell;
use std::ffi::c_void;
use std::mem::size_of;
use std::time::{Duration, Instant};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTERADDRESS, D3DKMT_ADAPTERTYPE, D3DKMT_CLOSEADAPTER,
    D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid,
    D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERADDRESS, KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_ADAPTERTYPE,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuInfo {
    /// Same key as `GpuSample::key`.
    pub key: String,
    /// (HighPart, LowPart) of the adapter LUID.
    pub luid: (i32, u32),
    pub name: String,
    pub dedicated_total: u64,
    pub shared_total: u64,
    /// PCI bus/device/function ("PCI bus 1, device 0, function 0") when reported by the kernel.
    pub location: Option<String>,
    /// Driver version string ("32.0.16.1714") when available.
    pub driver_version: Option<String>,
    /// Driver date string ("2026-...") when available.
    pub driver_date: Option<String>,
}

pub fn adapter_key(high: i32, low: u32) -> String {
    format!("luid_0x{:08x}_0x{low:08x}_phys_0", high as u32)
}

/// Queries D3DKMT for whether an adapter LUID is a real hardware render adapter (not software or
/// an Indirect Display Driver like a virtual display adapter) and its PCI bus/device/function.
fn query_kmt_adapter(luid: (i32, u32)) -> (bool, Option<String>) {
    let mut open = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: LUID { LowPart: luid.1, HighPart: luid.0 },
        hAdapter: 0,
    };
    // SAFETY: `open` is a valid in/out struct; handle is closed before returning.
    if unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.is_err() {
        return (true, None);
    }
    let h = open.hAdapter;
    let mut kind = D3DKMT_ADAPTERTYPE::default();
    let mut q_type = D3DKMT_QUERYADAPTERINFO {
        hAdapter: h,
        Type: KMTQAITYPE_ADAPTERTYPE,
        pPrivateDriverData: &mut kind as *mut _ as *mut c_void,
        PrivateDriverDataSize: size_of::<D3DKMT_ADAPTERTYPE>() as u32,
    };
    // SAFETY: valid adapter handle and output buffer for KMTQAITYPE_ADAPTERTYPE.
    let hardware = if unsafe { D3DKMTQueryAdapterInfo(&mut q_type) }.is_ok() {
        let bits = unsafe { kind.Anonymous.Value };
        // Bit 0: RenderSupported, Bit 2: SoftwareDevice, Bit 6: IndirectDisplayDevice.
        let render = bits & 0x01 != 0;
        let software = bits & 0x04 != 0;
        let indirect = bits & 0x40 != 0;
        render && !software && !indirect
    } else {
        true
    };
    let mut addr = D3DKMT_ADAPTERADDRESS::default();
    let mut q_addr = D3DKMT_QUERYADAPTERINFO {
        hAdapter: h,
        Type: KMTQAITYPE_ADAPTERADDRESS,
        pPrivateDriverData: &mut addr as *mut _ as *mut c_void,
        PrivateDriverDataSize: size_of::<D3DKMT_ADAPTERADDRESS>() as u32,
    };
    // SAFETY: valid adapter handle and output buffer for KMTQAITYPE_ADAPTERADDRESS.
    let location = unsafe { D3DKMTQueryAdapterInfo(&mut q_addr) }.is_ok().then(|| {
        format!("PCI bus {}, device {}, function {}", addr.BusNumber, addr.DeviceNumber, addr.FunctionNumber)
    });
    let close = D3DKMT_CLOSEADAPTER { hAdapter: h };
    // SAFETY: closes the handle opened above.
    unsafe {
        let _ = D3DKMTCloseAdapter(&close);
    }
    (hardware, location)
}

/// Hardware adapters in DXGI order, one per LUID (software and indirect display adapters are skipped).
pub fn list_gpus() -> Vec<GpuInfo> {
    enumerate_gpus().0
}

/// `list_gpus` plus a message for every DXGI call that failed. An adapter that cannot be described
/// is skipped (and reported); the remaining adapters are still listed.
pub fn enumerate_gpus() -> (Vec<GpuInfo>, Vec<String>) {
    let mut out: Vec<GpuInfo> = Vec::new();
    let mut errors = Vec::new();
    let reg_drivers = crate::sysinfo::gpu_driver_info();
    // SAFETY: plain factory creation; COM initialisation is not required for DXGI.
    let factory = match unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        Ok(f) => f,
        Err(e) => {
            errors.push(format!("gpu: CreateDXGIFactory1 failed {:#010x}", e.code().0));
            return (out, errors);
        }
    };
    for i in 0u32.. {
        // SAFETY: enumeration on a valid factory; fails with DXGI_ERROR_NOT_FOUND past the end.
        let adapter = match unsafe { factory.EnumAdapters1(i) } {
            Ok(a) => a,
            Err(e) => {
                if e.code() != DXGI_ERROR_NOT_FOUND {
                    errors.push(format!("gpu: EnumAdapters1({i}) failed {:#010x}", e.code().0));
                }
                break;
            }
        };
        // SAFETY: plain query on a valid adapter.
        let desc = match unsafe { adapter.GetDesc1() } {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("gpu: GetDesc1 for adapter {i} failed {:#010x}", e.code().0));
                continue;
            }
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let luid = (desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart);
        let (hardware, location) = query_kmt_adapter(luid);
        if !hardware {
            continue;
        }
        let key = adapter_key(luid.0, luid.1);
        if out.iter().any(|g| g.key == key) {
            continue;
        }
        let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..end]).trim().to_string();
        // If an Indirect Display Driver or linked clone reports the exact same PCI location &
        // hardware IDs as an already-listed GPU, skip the duplicate.
        if location.is_some() && out.iter().any(|g| g.name == name && g.location == location) {
            continue;
        }
        let (driver_version, driver_date) = reg_drivers
            .get(&name.to_lowercase())
            .cloned()
            .unwrap_or((None, None));
        out.push(GpuInfo {
            key,
            luid,
            name,
            dedicated_total: desc.DedicatedVideoMemory as u64,
            shared_total: desc.SharedSystemMemory as u64,
            location,
            driver_version,
            driver_date,
        });
    }
    (out, errors)
}

/// Adapter temperature is reported in tenths of a degree; 0 means the driver has no sensor.
pub fn deci_celsius(raw: u32) -> Option<f32> {
    (raw > 0 && raw < 2000).then(|| raw as f32 / 10.0)
}

/// How long a failing temperature query is left alone before it is tried again.
pub const QUERY_RETRY: Duration = Duration::from_secs(30);

/// How often, at most, DXGI adapters are enumerated again when PDH reports an adapter they don't
/// list (a driver update or a re-enabled adapter gets a new LUID).
pub const RESCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Whether to enumerate adapters again: PDH reports a key that neither `known` lists nor the last
/// enumeration already failed to match (`unmatched`, e.g. the Basic Render Driver), and the last
/// enumeration was at least RESCAN_INTERVAL ago.
pub(crate) fn needs_rescan<'a>(
    pdh_keys: impl IntoIterator<Item = &'a str>, known: &[GpuInfo], unmatched: &[String], last: Instant, now: Instant,
) -> bool {
    now.saturating_duration_since(last) >= RESCAN_INTERVAL
        && pdh_keys.into_iter().any(|k| !known.iter().any(|g| g.key == k) && !unmatched.iter().any(|u| u == k))
}

/// Kernel adapter handles opened once; queried every tick; closed on drop.
pub struct GpuTemps {
    /// (key, kernel handle, when a failing query may be tried again — `None` while it works).
    handles: Vec<(String, u32, Cell<Option<Instant>>)>,
    /// Adapters that could not be opened, reported on every read.
    open_errors: Vec<String>,
}

impl GpuTemps {
    pub fn open(gpus: &[GpuInfo]) -> Self {
        let mut handles = Vec::new();
        let mut open_errors = Vec::new();
        for g in gpus {
            let mut open = D3DKMT_OPENADAPTERFROMLUID {
                AdapterLuid: LUID { LowPart: g.luid.1, HighPart: g.luid.0 },
                hAdapter: 0,
            };
            // SAFETY: `open` is a valid in/out struct; the handle is closed in Drop.
            let status = unsafe { D3DKMTOpenAdapterFromLuid(&mut open) };
            if status.is_ok() {
                handles.push((g.key.clone(), open.hAdapter, Cell::new(None)));
            } else {
                open_errors.push(format!("gpu temperature: D3DKMTOpenAdapterFromLuid failed {:#010x} for {}", status.0, g.name));
            }
        }
        Self { handles, open_errors }
    }

    /// Temperatures of the GPUs that report one, keyed by `GpuInfo::key` (borrowed: nothing is
    /// allocated per tick). A GPU without a sensor is absent, never 0.
    pub fn read(&self) -> Vec<(&str, f32)> {
        self.read_checked().0
    }

    /// `read` plus a message for every failed open, and for a failed query when it starts failing
    /// (that adapter is then queried again only every QUERY_RETRY, and not reported again until it
    /// has worked). A query that succeeds with no sensor is not a failure.
    pub fn read_checked(&self) -> (Vec<(&str, f32)>, Vec<String>) {
        self.read_checked_at(Instant::now())
    }

    fn read_checked_at(&self, now: Instant) -> (Vec<(&str, f32)>, Vec<String>) {
        let mut errors = self.open_errors.clone();
        let mut temps = Vec::new();
        for (key, h, retry_at) in &self.handles {
            if retry_at.get().is_some_and(|at| now < at) {
                continue;
            }
            let mut perf = D3DKMT_ADAPTER_PERFDATA { PhysicalAdapterIndex: 0, ..Default::default() };
            let mut query = D3DKMT_QUERYADAPTERINFO {
                hAdapter: *h,
                Type: KMTQAITYPE_ADAPTERPERFDATA,
                pPrivateDriverData: &mut perf as *mut _ as *mut c_void,
                PrivateDriverDataSize: size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
            };
            // SAFETY: valid adapter handle; `perf` is the documented output for this query type
            // and outlives the call.
            let status = unsafe { D3DKMTQueryAdapterInfo(&mut query) };
            if status.is_err() {
                // Reported once, then backed off: an adapter without support (iGPU, virtual) would
                // fail every tick, but a transient failure must not disable it for good.
                if retry_at.replace(Some(now + QUERY_RETRY)).is_none() {
                    errors.push(format!("gpu temperature: D3DKMTQueryAdapterInfo failed {:#010x} for {key}", status.0));
                }
            } else {
                retry_at.set(None);
                if let Some(c) = deci_celsius(perf.Temperature) {
                    temps.push((key.as_str(), c));
                }
            }
        }
        (temps, errors)
    }
}

impl Drop for GpuTemps {
    fn drop(&mut self) {
        for (_, h, _) in &self.handles {
            let close = D3DKMT_CLOSEADAPTER { hAdapter: *h };
            // SAFETY: closes a handle opened in `open`, exactly once.
            unsafe {
                let _ = D3DKMTCloseAdapter(&close);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_keys_match_pdh_instances() {
        assert_eq!(adapter_key(0, 0x0001_569B), "luid_0x00000000_0x0001569b_phys_0");
        assert_eq!(adapter_key(-1, 1), "luid_0xffffffff_0x00000001_phys_0");
    }

    #[test]
    fn temperature_is_tenths_of_a_degree_and_zero_means_no_sensor() {
        assert_eq!(deci_celsius(625), Some(62.5));
        assert_eq!(deci_celsius(0), None);
        assert_eq!(deci_celsius(5000), None, "implausible values are dropped");
    }

    #[test]
    fn a_failing_temperature_query_is_reported_once_then_retried_after_a_back_off() {
        // The handle is made up: never let Drop pass it to D3DKMTCloseAdapter (even on a failed assert).
        let temps = std::mem::ManuallyDrop::new(GpuTemps { handles: vec![("k".into(), 0xdead_beef, Cell::new(None))], open_errors: vec![] });
        let t0 = Instant::now();
        let (read, first) = temps.read_checked_at(t0);
        assert!(read.is_empty());
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(first[0].starts_with("gpu temperature: D3DKMTQueryAdapterInfo failed"), "{first:?}");
        assert!(temps.read_checked_at(t0 + Duration::from_secs(1)).1.is_empty(), "not re-queried or re-reported");
        assert_eq!(temps.handles[0].2.get(), Some(t0 + QUERY_RETRY), "skipped during the back-off");
        let later = t0 + QUERY_RETRY;
        assert!(temps.read_checked_at(later).1.is_empty(), "retried but not re-reported");
        assert_eq!(temps.handles[0].2.get(), Some(later + QUERY_RETRY), "queried again, failed again");
    }

    #[test]
    fn adapters_are_enumerated_again_only_for_a_new_pdh_key_and_at_most_every_interval() {
        let known = [GpuInfo { key: "a".into(), ..Default::default() }];
        let unmatched = ["basic".to_string()];
        let t0 = Instant::now();
        let later = t0 + RESCAN_INTERVAL;
        assert!(!needs_rescan(["a", "basic"], &known, &unmatched, t0, later), "all known or known to be unmatched");
        assert!(needs_rescan(["a", "new"], &known, &unmatched, t0, later), "new LUID after a driver update");
        assert!(!needs_rescan(["new"], &known, &unmatched, t0, t0 + Duration::from_secs(29)), "rate-limited");
    }

    #[test]
    fn an_adapter_that_cannot_be_opened_is_reported_not_swallowed() {
        let bogus = GpuInfo {
            key: "luid_0x7fffffff_0x7fffffff_phys_0".into(),
            luid: (0x7fff_ffff, 0x7fff_ffff),
            name: "ghost".into(),
            ..Default::default()
        };
        let temps = GpuTemps::open(&[bogus]);
        let (read, errors) = temps.read_checked();
        assert!(read.is_empty());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("gpu temperature: D3DKMTOpenAdapterFromLuid failed"), "{errors:?}");
    }
}
