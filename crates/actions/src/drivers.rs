//! Running third-party kernel drivers that filter network traffic: Windows Filtering Platform
//! callout drivers, found by their import of `fwpkclnt.sys`. Read-only and works without admin
//! rights (WFP's own callout enumeration is denied unelevated). Call off the UI thread.
use std::path::{Path, PathBuf};

/// What the Ports page and End task name a running driver by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DriverInfo {
    pub display_name: String,
    pub company: Option<String>,
    pub product: Option<String>,
}

/// Whether a driver image imports the WFP kernel client (`fwpkclnt.sys`). Import names are plain
/// ASCII in the image, so a case-insensitive search finds them.
pub fn references_wfp(image: &[u8]) -> bool {
    const NAME: &[u8] = b"fwpkclnt.sys";
    image.windows(NAME.len()).any(|w| w.eq_ignore_ascii_case(NAME))
}

/// A driver's `ImagePath` as the service manager gives it, as a file path.
pub fn driver_path(raw: &str, windir: &Path) -> PathBuf {
    let raw = raw.trim().trim_matches('"');
    if let Some(rest) = raw.strip_prefix(r"\??\") {
        return PathBuf::from(rest);
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with(r"\systemroot\") {
        return windir.join(&raw[r"\SystemRoot\".len()..]);
    }
    if lower.starts_with(r"system32\") {
        return windir.join(raw);
    }
    PathBuf::from(raw)
}

/// The names to show: product name, else the service's display name; Microsoft's own drivers left
/// out; sorted, without repeats.
pub fn filter_names(drivers: &[DriverInfo]) -> Vec<String> {
    let mut names: Vec<String> = drivers
        .iter()
        .filter(|d| !d.company.as_deref().is_some_and(|c| c.contains("Microsoft")))
        .map(|d| {
            let product = d.product.as_deref().map(str::trim).filter(|p| !p.is_empty());
            product.unwrap_or(d.display_name.trim()).to_string()
        })
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The running third-party WFP callout drivers, by name. Empty when none, or when the drivers
/// can't be listed.
pub fn network_filters() -> Vec<String> {
    let Ok(drivers) = crate::services::running_drivers() else { return Vec::new() };
    let windir = PathBuf::from(negotium_collector::windows_dir());
    let infos: Vec<DriverInfo> = drivers
        .into_iter()
        .filter_map(|(display_name, raw)| {
            let path = driver_path(&raw, &windir);
            let version = negotium_collector::VersionInfo::read(path.to_str()?);
            let company = version.as_ref().and_then(|v| v.string("CompanyName"));
            // Windows' own drivers are left out anyway: skip reading their images.
            if company.as_deref().is_some_and(|c| c.contains("Microsoft")) {
                return None;
            }
            let image = std::fs::read(&path).ok()?;
            references_wfp(&image).then(|| DriverInfo {
                display_name,
                company,
                product: version.as_ref().and_then(|v| v.string("ProductName")),
            })
        })
        .collect();
    filter_names(&infos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_import_of_the_wfp_client_is_found_in_any_case() {
        assert!(references_wfp(b"\0\0FWPKCLNT.SYS\0NETIO.SYS\0"));
        assert!(references_wfp(b"x fwpkclnt.sys y"));
        assert!(!references_wfp(b"ndis.sys\0ntoskrnl.exe\0"));
        assert!(!references_wfp(b""));
    }

    #[test]
    fn image_paths_become_file_paths() {
        let win = Path::new(r"C:\Windows");
        assert_eq!(driver_path(r"\??\C:\Program Files\NordVPN\NDivert.sys", win), PathBuf::from(r"C:\Program Files\NordVPN\NDivert.sys"));
        assert_eq!(driver_path(r"\SystemRoot\System32\drivers\npcap.sys", win), PathBuf::from(r"C:\Windows\System32\drivers\npcap.sys"));
        assert_eq!(driver_path(r"System32\drivers\tcpip.sys", win), PathBuf::from(r"C:\Windows\System32\drivers\tcpip.sys"));
        assert_eq!(driver_path(r"C:\Program Files\Portmaster\portmaster-kext.sys", win), PathBuf::from(r"C:\Program Files\Portmaster\portmaster-kext.sys"));
    }

    fn info(display: &str, company: Option<&str>, product: Option<&str>) -> DriverInfo {
        DriverInfo { display_name: display.into(), company: company.map(String::from), product: product.map(String::from) }
    }

    #[test]
    fn names_skip_microsoft_prefer_the_product_and_have_no_repeats() {
        let drivers = [
            info("Windows Defender Firewall Authorization Driver", Some("Microsoft Corporation"), Some("Microsoft® Windows® Operating System")),
            info("PortmasterKext", Some("Safing ICS Technologies GmbH"), Some("Portmaster")),
            info("NordVPN Divert Driver", Some("NordVPN S.A."), Some("NordVPN")),
            info("NordVPN Other Driver", Some("NordVPN S.A."), Some("NordVPN")),
            info("Npcap Packet Driver (NPCAP)", Some("Nmap Software LLC"), None),
            info("Blank Product", None, Some("  ")),
        ];
        assert_eq!(filter_names(&drivers), ["Blank Product", "NordVPN", "Npcap Packet Driver (NPCAP)", "Portmaster"]);
    }

    #[test]
    fn windows_own_wfp_driver_is_found_and_a_plain_driver_is_not() {
        let drivers = PathBuf::from(negotium_collector::windows_dir()).join(r"System32\drivers");
        assert!(references_wfp(&std::fs::read(drivers.join("mpsdrv.sys")).expect("mpsdrv.sys")));
        assert!(!references_wfp(&std::fs::read(drivers.join("null.sys")).expect("null.sys")));
    }

    #[test]
    fn the_live_list_has_no_microsoft_driver_and_no_blank_name() {
        let names = network_filters();
        assert!(names.iter().all(|n| !n.trim().is_empty() && !n.contains("Microsoft")), "{names:?}");
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted);
    }
}
