//! GPU adapters from DXGI (name, memory sizes, LUID) and their temperature from the kernel
//! adapter performance data (D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA), as Task Manager.
//! Every failure comes back as a message for `source_errors`; none of them stops the other data.
use std::ffi::c_void;
use std::mem::size_of;
use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO,
    D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA,
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
}

pub fn adapter_key(high: i32, low: u32) -> String {
    format!("luid_0x{:08x}_0x{low:08x}_phys_0", high as u32)
}

/// Hardware adapters in DXGI order, one per LUID (the software rasterizer is skipped).
pub fn list_gpus() -> Vec<GpuInfo> {
    enumerate_gpus().0
}

/// `list_gpus` plus a message for every DXGI call that failed. An adapter that cannot be described
/// is skipped (and reported); the remaining adapters are still listed.
pub fn enumerate_gpus() -> (Vec<GpuInfo>, Vec<String>) {
    let mut out: Vec<GpuInfo> = Vec::new();
    let mut errors = Vec::new();
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
        let key = adapter_key(desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart);
        if out.iter().any(|g| g.key == key) {
            continue;
        }
        let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        out.push(GpuInfo {
            key,
            luid: (desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart),
            name: String::from_utf16_lossy(&desc.Description[..end]).trim().to_string(),
            dedicated_total: desc.DedicatedVideoMemory as u64,
            shared_total: desc.SharedSystemMemory as u64,
        });
    }
    (out, errors)
}

/// Adapter temperature is reported in tenths of a degree; 0 means the driver has no sensor.
pub fn deci_celsius(raw: u32) -> Option<f32> {
    (raw > 0 && raw < 2000).then(|| raw as f32 / 10.0)
}

/// Kernel adapter handles opened once; queried every tick; closed on drop.
pub struct GpuTemps {
    handles: Vec<(String, u32)>,
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
                handles.push((g.key.clone(), open.hAdapter));
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

    /// `read` plus a message for every failed open or query. A query that succeeds with no sensor
    /// is not a failure.
    pub fn read_checked(&self) -> (Vec<(&str, f32)>, Vec<String>) {
        let mut errors = self.open_errors.clone();
        let mut temps = Vec::new();
        for (key, h) in &self.handles {
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
                errors.push(format!("gpu temperature: D3DKMTQueryAdapterInfo failed {:#010x} for {key}", status.0));
            } else if let Some(c) = deci_celsius(perf.Temperature) {
                temps.push((key.as_str(), c));
            }
        }
        (temps, errors)
    }
}

impl Drop for GpuTemps {
    fn drop(&mut self) {
        for (_, h) in &self.handles {
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
