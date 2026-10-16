//! The environment a benchmarked Negotium runs in. It never shares the user's real
//! `%LOCALAPPDATA%\Negotium` (settings.json, renderer-fallback marker, log): each run gets a fresh
//! scratch directory, and the renderer is chosen explicitly (GPU by default). Pure: computes what
//! to change; `apply_to_process` is the only side effect.
use anyhow::{Result, bail};
#[cfg(test)]
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Renderer {
    /// Negotium's own default: winit + FemtoVG.
    #[default]
    Gpu,
    /// winit + software.
    Software,
}

impl Renderer {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "gpu" => Ok(Self::Gpu),
            "software" => Ok(Self::Software),
            other => bail!("--renderer takes gpu or software, not {other}"),
        }
    }
}

/// Environment variables to set and to remove for a Negotium child.
#[derive(Debug, PartialEq)]
pub struct ChildEnv {
    pub set: Vec<(&'static str, OsString)>,
    pub remove: Vec<&'static str>,
}

/// The scratch `%LOCALAPPDATA%` for one run. Refuses the user's real one.
pub fn scratch_local_appdata(temp: &Path, real: Option<&OsStr>, bench_pid: u32, run: u32) -> Result<PathBuf> {
    let dir = temp.join(format!("negotium-bench-{bench_pid}-run{run}"));
    if real.is_some_and(|r| same_path(r, dir.as_os_str())) {
        bail!("refusing to use the real LOCALAPPDATA as the benchmark's scratch directory");
    }
    Ok(dir)
}

fn same_path(a: &OsStr, b: &OsStr) -> bool {
    let norm = |s: &OsStr| s.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// What a Negotium child needs: its own LOCALAPPDATA, the frame-stats file, and the renderer.
pub fn negotium_child_env(scratch_local_appdata: &Path, stats: &Path, renderer: Renderer) -> ChildEnv {
    let mut set = vec![
        ("LOCALAPPDATA", scratch_local_appdata.as_os_str().to_owned()),
        ("NEGOTIUM_FRAME_STATS", stats.as_os_str().to_owned()),
    ];
    let mut remove = vec!["NEGOTIUM_SIMULATE_STALL"];
    match renderer {
        // No SLINT_BACKEND: Negotium selects GPU itself (and keeps its watchdog), with fresh settings.
        Renderer::Gpu => remove.push("SLINT_BACKEND"),
        Renderer::Software => set.push(("SLINT_BACKEND", "winit-software".into())),
    }
    ChildEnv { set, remove }
}

/// A scratch directory that is removed when dropped, on every exit path including errors and
/// panics. It only ever removes the one path it created.
#[derive(Debug)]
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Creates `path` empty (a stale directory of the same per-run name is ours and is replaced).
    pub fn create(path: PathBuf) -> Result<Self> {
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).map_err(|e| anyhow::anyhow!("create {}: {e}", path.display()))?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ChildEnv {
    /// Applies the change to an environment map (keys upper-cased, as Windows compares them);
    /// the test-side twin of `apply_to_process`, driven by the same `set`/`remove` lists.
    #[cfg(test)]
    pub fn apply_to(&self, env: &mut HashMap<String, OsString>) {
        for name in &self.remove {
            env.remove(&name.to_uppercase());
        }
        for (name, value) in &self.set {
            env.insert(name.to_uppercase(), value.clone());
        }
    }

    /// Changes this process's environment, which the launched child inherits.
    pub fn apply_to_process(&self) {
        // SAFETY: Rust marks env mutation `unsafe` because other threads may use the C runtime's
        // getenv/setenv. This process only changes its environment here, and the Windows call
        // underneath (SetEnvironmentVariableW) is thread-safe, so a shell thread is not corrupted.
        unsafe {
            for name in &self.remove {
                std::env::remove_var(name);
            }
            for (name, value) in &self.set {
                std::env::set_var(name, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent_env(real: &str) -> HashMap<String, OsString> {
        [("LOCALAPPDATA", real), ("SLINT_BACKEND", "winit-software"), ("NEGOTIUM_SIMULATE_STALL", "1"), ("PATH", "p")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect()
    }

    #[test]
    fn the_child_never_sees_the_real_local_app_data() {
        let real = r"C:\Users\someone\AppData\Local";
        let scratch = scratch_local_appdata(Path::new(r"C:\Temp"), Some(OsStr::new(real)), 7, 2).unwrap();
        let mut env = parent_env(real);
        negotium_child_env(&scratch, Path::new(r"C:\Temp\stats.json"), Renderer::Gpu).apply_to(&mut env);
        assert_eq!(env["LOCALAPPDATA"], scratch.as_os_str());
        assert_ne!(env["LOCALAPPDATA"], OsString::from(real));
        assert!(scratch.starts_with(r"C:\Temp"), "{scratch:?}");
        assert_eq!(env["PATH"], "p", "the rest is untouched");
        assert_eq!(env["NEGOTIUM_FRAME_STATS"], r"C:\Temp\stats.json");
        assert!(!env.contains_key("NEGOTIUM_SIMULATE_STALL"));
    }

    #[test]
    fn dropping_the_scratch_guard_removes_only_its_own_directory() {
        let root = std::env::temp_dir().join(format!("negotium-bench-test-{}", std::process::id()));
        let (mine, other) = (root.join("mine"), root.join("other"));
        std::fs::create_dir_all(&other).unwrap();
        let guard = ScratchDir::create(mine.clone()).unwrap();
        std::fs::write(guard.path().join("settings.json"), "x").unwrap();
        assert!(mine.is_dir());
        drop(guard);
        assert!(!mine.exists(), "its directory and contents are gone");
        assert!(other.is_dir(), "a sibling is untouched");
        // An unwinding panic drops it too.
        let m2 = root.join("panic");
        let r = std::panic::catch_unwind(|| {
            let _g = ScratchDir::create(m2.clone()).unwrap();
            panic!("boom");
        });
        assert!(r.is_err() && !m2.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn each_run_gets_its_own_directory_and_the_real_one_is_refused() {
        let t = Path::new(r"C:\Temp");
        let a = scratch_local_appdata(t, None, 7, 0).unwrap();
        assert_ne!(a, scratch_local_appdata(t, None, 7, 1).unwrap(), "runs do not share a marker or settings");
        let real = r"C:\Temp\negotium-bench-7-run0\";
        assert!(scratch_local_appdata(t, Some(OsStr::new(real)), 7, 0).is_err());
    }

    #[test]
    fn gpu_is_the_default_and_leaves_slint_backend_unset_and_software_selects_it() {
        assert_eq!(Renderer::default(), Renderer::Gpu);
        let (dir, stats) = (Path::new(r"C:\S"), Path::new(r"C:\s.json"));
        let mut env = parent_env(r"C:\Real");
        negotium_child_env(dir, stats, Renderer::Gpu).apply_to(&mut env);
        assert!(!env.contains_key("SLINT_BACKEND"), "an inherited SLINT_BACKEND must not leak into the GPU run");
        negotium_child_env(dir, stats, Renderer::Software).apply_to(&mut env);
        assert_eq!(env["SLINT_BACKEND"], "winit-software");
        assert_eq!(Renderer::parse("Software").unwrap(), Renderer::Software);
        assert!(Renderer::parse("vulkan").is_err());
    }
}
