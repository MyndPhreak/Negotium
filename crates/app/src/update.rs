//! Updates from GitHub Releases (spec goals 8–10). The installed copy asks GitHub's public API
//! for the latest release of MyndPhreak/Negotium; a newer `vX.Y.Z` with exactly one `.msi` on
//! GitHub over HTTPS is offered in a banner ("Update to vX.Y.Z"). Accepting downloads that MSI to
//! `%TEMP%\Negotium-update`, checks it against the release's SHA-256 digest (when GitHub gives
//! one), starts `msiexec /i "<msi>" /passive` elevated (one UAC prompt) and quits, so the installer
//! can replace the exe. A declined prompt keeps Negotium running.
//!
//! The pure parts (the release, the URLs, the file) are tested here against a fake [`Fetch`];
//! [`WinHttp`] is the real one, and [`Updater`] is the window's side. Nothing about the user or
//! the machine is sent: a plain GET with `User-Agent: Negotium/<version>`.
use crate::MainWindow;
use serde::Deserialize;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// The latest release of the repository (GitHub never answers a draft or a prerelease here, and
/// both are refused anyway).
pub const LATEST_URL: &str = "https://api.github.com/repos/MyndPhreak/Negotium/releases/latest";
/// The hosts a release's installer may be downloaded from: GitHub's download link, and the asset
/// stores it redirects to (older and newer). The API is asked at `api.github.com`. Every hop is
/// HTTPS.
pub const ASSET_HOSTS: [&str; 3] = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
const GITHUB: &str = ASSET_HOSTS[0];
/// Where a download on `github.com` redirects (signed, short-lived URLs).
const ASSET_STORES: [&str; 2] = [ASSET_HOSTS[1], ASSET_HOSTS[2]];
const API_HOST: &str = "api.github.com";
/// On `github.com`, the installer is `<ASSET_PATH><the release's tag>/<file>`: a download of that
/// release of this repository.
const ASSET_PATH: &str = "/MyndPhreak/Negotium/releases/download/";
/// A larger "installer" is not Negotium's.
pub const MAX_MSI_BYTES: usize = 200 * 1024 * 1024;
/// A release's JSON is a few kilobytes; more is not a release.
pub const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
/// Each request's resolve, connect, send and receive timeouts.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// What one request may take: at most `bytes` of body, and `deadline` overall (a trickle that
/// never trips [`TIMEOUT`] still ends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub bytes: usize,
    pub deadline: Duration,
}

/// The check: the release's JSON.
pub const JSON_LIMIT: Limit = Limit { bytes: MAX_JSON_BYTES, deadline: Duration::from_secs(30) };
/// The download: the installer, within ten minutes.
pub const MSI_LIMIT: Limit = Limit { bytes: MAX_MSI_BYTES, deadline: Duration::from_secs(10 * 60) };
/// Redirects followed per request (GitHub's download takes one).
const MAX_REDIRECTS: usize = 5;
/// An OLE compound file's signature, which every MSI starts with.
const MSI_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// A version, `X.Y.Z`, compared as numbers.
pub type Version = (u64, u64, u64);
/// A SHA-256 digest.
pub type Digest = [u8; 32];

/// A newer release, as offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Its one `.msi` asset, on [`ASSET_HOSTS`] over HTTPS.
    pub msi_url: String,
    /// The release's page (`html_url`), for the log.
    pub page_url: String,
    /// The asset's SHA-256, from GitHub's `digest` (`sha256:<hex>`) when it gives one; the
    /// installer is checked against it right before it runs ([`verify_and_launch`]).
    pub sha256: Option<Digest>,
}

impl Release {
    /// `vX.Y.Z`, as the banner says it.
    pub fn tag(&self) -> String {
        let (x, y, z) = self.version;
        format!("v{x}.{y}.{z}")
    }
}

/// Why a check, a download or the installer's start failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// WinHTTP failed (offline, a timeout, TLS).
    Network(String),
    /// An HTTP status other than 200 (a rate limit is 403 or 429).
    Status(u32),
    /// A redirect off GitHub or off HTTPS, or too many.
    Redirect(String),
    /// The API's answer isn't a release.
    Json(String),
    /// The release can't be offered: its tag isn't `vX.Y.Z`, or its installer is missing,
    /// ambiguous or not on GitHub over HTTPS.
    Release(String),
    /// What was downloaded isn't an installer.
    Download(String),
    /// The installer couldn't be saved.
    Io(String),
    /// The installer isn't the release's: its SHA-256 differs from the asset's digest.
    Digest(String),
    /// msiexec couldn't be started.
    Start(String),
    /// The UAC prompt for msiexec was declined: nothing ran.
    Cancelled,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(e) => write!(f, "network error: {e}"),
            UpdateError::Status(code) => write!(f, "GitHub answered HTTP {code}"),
            UpdateError::Redirect(e) => write!(f, "redirect refused: {e}"),
            UpdateError::Json(e) => write!(f, "the release couldn't be read: {e}"),
            UpdateError::Release(e) => write!(f, "the release can't be installed: {e}"),
            UpdateError::Download(e) => write!(f, "the download isn't an installer: {e}"),
            UpdateError::Io(e) => write!(f, "the installer couldn't be saved: {e}"),
            UpdateError::Digest(e) => write!(f, "the installer isn't the release's: {e}"),
            UpdateError::Start(e) => write!(f, "the installer couldn't be started: {e}"),
            UpdateError::Cancelled => f.write_str("it was cancelled"),
        }
    }
}

/// An HTTPS GET of a whole body within `limit`. The real one is [`WinHttp`]; tests answer from a
/// table.
pub trait Fetch {
    fn get(&self, url: &str, limit: Limit) -> Result<Vec<u8>, UpdateError>;
}

/// The User-Agent every request sends (GitHub's API requires one).
pub fn user_agent() -> String {
    format!("Negotium/{}", crate::install::VERSION)
}

/// `X.Y.Z`: three decimal numbers, nothing else.
fn parse_version(s: &str) -> Option<Version> {
    let mut parts = s.split('.');
    let mut next = || parts.next().filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))?.parse().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>` (GitHub adds it to assets uploaded since mid-2025); absent or null on older
    /// ones.
    #[serde(default)]
    digest: Option<String>,
}

/// GitHub's asset digest, `sha256:<64 hex digits>`.
fn parse_digest(digest: &str) -> Option<Digest> {
    let hex = digest.strip_prefix("sha256:").filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// The release in the API's answer `json`, if it should be offered to version `current`: not a
/// draft or prerelease, tagged `vX.Y.Z` newer than `current`, with exactly one `.msi` asset on
/// GitHub over HTTPS. An older or equal release is `None`; a newer-looking one that can't be
/// installed, or a tag that isn't a version, is an error (logged; the manual check says it
/// couldn't check rather than "up to date").
pub fn parse_latest(json: &str, current: &str) -> Result<Option<Release>, UpdateError> {
    let latest: Latest = serde_json::from_str(json).map_err(|e| UpdateError::Json(e.to_string()))?;
    let current = parse_version(current).ok_or_else(|| UpdateError::Release(format!("running version {current:?} is not X.Y.Z")))?;
    if latest.draft || latest.prerelease {
        return Ok(None);
    }
    let tag = &latest.tag_name;
    let version = tag
        .strip_prefix('v')
        .and_then(parse_version)
        .ok_or_else(|| UpdateError::Release(format!("tag {tag:?} is not vX.Y.Z")))?;
    if version <= current {
        return Ok(None);
    }
    let mut msis = latest.assets.iter().filter(|a| a.name.to_ascii_lowercase().ends_with(".msi"));
    let msi = match (msis.next(), msis.next()) {
        (Some(msi), None) => msi,
        (None, _) => return Err(UpdateError::Release(format!("{tag} has no .msi asset"))),
        (Some(_), Some(_)) => return Err(UpdateError::Release(format!("{tag} has more than one .msi asset"))),
    };
    let url = &msi.browser_download_url;
    if !asset_allowed(url, tag) {
        return Err(UpdateError::Release(format!("{tag}'s installer {url:?} is not a GitHub download of {tag} over HTTPS")));
    }
    let sha256 = match msi.digest.as_deref() {
        None => None,
        Some(digest) => Some(
            parse_digest(digest).ok_or_else(|| UpdateError::Release(format!("{tag}'s installer digest {digest:?} is not sha256:<hex>")))?,
        ),
    };
    Ok(Some(Release { version, msi_url: url.clone(), page_url: latest.html_url, sha256 }))
}

/// An `https://` URL, split.
struct Parts<'a> {
    /// Lowercase.
    host: String,
    /// The path alone (no query).
    path: &'a str,
    /// What is asked of the host: the path and the query, without any fragment.
    target: &'a str,
}

/// An `https://` URL's parts. No user info, no port, no whitespace, control characters or
/// backslashes (which parsers disagree on) anywhere; no dot segments in the path, plain or
/// percent-encoded (`%2e`, and `%2f` for a hidden slash). The query is not judged: signed asset
/// URLs carry encoded slashes there (`X-Amz-Credential=…%2F…`, `rsct=application%2Foctet-stream`).
fn https_parts(url: &str) -> Option<Parts<'_>> {
    if url.bytes().any(|b| b <= b' ' || b == b'\\' || b >= 0x7f) {
        return None;
    }
    let rest = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).map(|_| &url[8..])?;
    let rest = rest.split('#').next().unwrap_or_default();
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (host, target) = rest.split_at(end);
    if host.is_empty() || host.contains(['@', ':']) {
        return None;
    }
    let path = target.split('?').next().unwrap_or_default();
    let lower = path.to_ascii_lowercase();
    if path.split('/').any(|seg| seg == "." || seg == "..") || lower.contains("%2e") || lower.contains("%2f") {
        return None;
    }
    let target = if target.is_empty() { "/" } else { target };
    Some(Parts { host: host.to_ascii_lowercase(), path, target })
}

/// The tag of a `github.com` release download path, `<ASSET_PATH><tag>/<file>`.
fn download_tag(path: &str) -> Option<&str> {
    let rest = path.get(..ASSET_PATH.len()).filter(|p| p.eq_ignore_ascii_case(ASSET_PATH)).map(|_| &path[ASSET_PATH.len()..])?;
    let (tag, file) = rest.split_once('/')?;
    (!tag.is_empty() && !file.is_empty() && !file.contains('/')).then_some(tag)
}

/// A release's installer URL, over HTTPS: on `github.com`, a download of release `tag` of this
/// repository; or one of GitHub's asset stores.
fn asset_allowed(url: &str, tag: &str) -> bool {
    match https_parts(url) {
        Some(p) if p.host == GITHUB => download_tag(p.path) == Some(tag),
        Some(p) => ASSET_STORES.contains(&p.host.as_str()),
        None => false,
    }
}

/// A URL a request may start at: the API, a release download of this repository, or an asset
/// store, over HTTPS.
fn first_hop_allowed(url: &str) -> bool {
    match https_parts(url) {
        Some(p) if p.host == GITHUB => download_tag(p.path).is_some(),
        Some(p) => p.host == API_HOST || ASSET_STORES.contains(&p.host.as_str()),
        None => false,
    }
}

/// Where a redirect from `current` to `location` (absolute, or a path on the same host) goes, if
/// it stays on GitHub over HTTPS: the API only to itself, a release download only to the same
/// release's downloads or an asset store, an asset store only to an asset store.
pub fn next_hop(current: &str, location: &str) -> Result<String, UpdateError> {
    let refused = || UpdateError::Redirect(format!("to {location:?}"));
    let from = https_parts(current).ok_or_else(refused)?;
    let next = if location.starts_with('/') && !location.starts_with("//") {
        format!("https://{}{location}", from.host)
    } else {
        location.to_string()
    };
    let to = https_parts(&next).ok_or_else(refused)?;
    let allowed = match to.host.as_str() {
        API_HOST => from.host == API_HOST,
        GITHUB => from.host == GITHUB && download_tag(to.path).is_some() && download_tag(to.path) == download_tag(from.path),
        host if ASSET_STORES.contains(&host) => from.host == GITHUB || ASSET_STORES.contains(&from.host.as_str()),
        _ => false,
    };
    if allowed { Ok(next) } else { Err(refused()) }
}

/// The latest release, if newer than `current` ([`parse_latest`]).
pub fn check(fetch: &dyn Fetch, current: &str) -> Result<Option<Release>, UpdateError> {
    let body = fetch.get(LATEST_URL, JSON_LIMIT)?;
    let json = std::str::from_utf8(&body).map_err(|e| UpdateError::Json(e.to_string()))?;
    parse_latest(json, current)
}

/// Where downloads go: `%TEMP%\Negotium-update`.
pub fn update_dir() -> PathBuf {
    std::env::temp_dir().join("Negotium-update")
}

/// An installer of `len` bytes is plausible: not empty, not over [`MAX_MSI_BYTES`].
pub fn msi_size_ok(len: usize) -> bool {
    (1..=MAX_MSI_BYTES).contains(&len)
}

/// Downloads `release`'s installer to `dir\Negotium-X.Y.Z.msi` (replacing an earlier one). The URL
/// is checked again first, and the body must look like an MSI. Written beside and then renamed,
/// so a failed download never leaves half an installer under the final name.
pub fn download(fetch: &dyn Fetch, release: &Release, dir: &Path) -> Result<PathBuf, UpdateError> {
    if !asset_allowed(&release.msi_url, &release.tag()) {
        return Err(UpdateError::Release(format!("{:?} is not a GitHub download over HTTPS", release.msi_url)));
    }
    let body = fetch.get(&release.msi_url, MSI_LIMIT)?;
    if !msi_size_ok(body.len()) {
        return Err(UpdateError::Download(format!("{} bytes", body.len())));
    }
    if !body.starts_with(&MSI_SIGNATURE) {
        return Err(UpdateError::Download("not a Windows Installer package".into()));
    }
    let (x, y, z) = release.version;
    let path = dir.join(format!("Negotium-{x}.{y}.{z}.msi"));
    let part = path.with_extension("msi.part");
    let io = |e: std::io::Error| UpdateError::Io(e.to_string());
    std::fs::create_dir_all(dir).map_err(io)?;
    std::fs::write(&part, &body).map_err(io)?;
    std::fs::rename(&part, &path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        io(e)
    })?;
    Ok(path)
}

/// msiexec's arguments: install `msi` with progress only (no choices; upgrades keep the
/// installed options).
pub fn installer_args(msi: &Path) -> String {
    format!("/i \"{}\" /passive", msi.display())
}

/// SHA-256 of `bytes` (Windows' CNG).
pub fn sha256(bytes: &[u8]) -> Result<Digest, UpdateError> {
    use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
    let mut out = [0u8; 32];
    // SAFETY: the SHA-256 pseudo-handle needs no opening; the input and the 32-byte output are
    // live slices of the lengths passed.
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, bytes, &mut out) };
    if status.is_ok() { Ok(out) } else { Err(UpdateError::Digest(format!("SHA-256 failed (NTSTATUS {:#x})", status.0))) }
}

/// The last step before the installer runs: `msi` is checked against the release's digest (read
/// again now, not trusted from the download) and then `launch`ed. A mismatch deletes the file and
/// nothing runs; a release without a digest is logged and installed unverified (assets uploaded
/// before GitHub added digests have none). Off the UI thread in the app.
pub fn verify_and_launch(
    msi: &Path,
    expected: Option<&Digest>,
    launch: &dyn Fn(&Path) -> Result<(), UpdateError>,
    log: &dyn Fn(&str),
) -> Result<(), UpdateError> {
    match expected {
        Some(expected) => {
            let bytes = std::fs::read(msi).map_err(|e| UpdateError::Io(e.to_string()))?;
            if sha256(&bytes)? != *expected {
                let _ = std::fs::remove_file(msi);
                return Err(UpdateError::Digest("its SHA-256 doesn't match the release's digest".into()));
            }
        }
        None => log(&format!("{}: the release gives no SHA-256 digest; installing it unverified", msi.display())),
    }
    launch(msi)
}

/// Starts `<System32>\msiexec.exe` on `msi` ([`installer_args`]) elevated (the `runas` verb: one
/// UAC prompt, owned by window `owner`), without waiting for it. A declined prompt is
/// [`UpdateError::Cancelled`] and nothing runs. Negotium exits once this succeeds; msiexec doesn't
/// wait for it, but the installer asks a still-running Negotium to exit before replacing files
/// (spec goal 7). Never called by tests.
pub fn launch_msiexec(msi: &Path, owner: isize) -> Result<(), UpdateError> {
    let msiexec = crate::integrate::system32()
        .map(|dir| dir.join("msiexec.exe"))
        .ok_or_else(|| UpdateError::Start("System32 could not be found".into()))?;
    negotium_actions::run_elevated_detached(&msiexec, &installer_args(msi), owner).map_err(|e| match e {
        negotium_actions::ActionError::Cancelled => UpdateError::Cancelled,
        e => UpdateError::Start(e.to_string()),
    })
}

/// Whether this launch checks for updates on its own: the installed copy with "Check for updates
/// on startup" on. A development build never does.
pub fn startup_check_wanted(installed: bool, check_updates: bool) -> bool {
    installed && check_updates
}

/// Plain HTTPS GETs through WinHTTP (Windows' TLS and proxy settings): [`TIMEOUT`] for each
/// stage, [`user_agent`], no cookies, and redirects followed here only while they stay on GitHub
/// over HTTPS ([`next_hop`]). Each request keeps to its [`Limit`] (size and overall deadline).
pub struct WinHttp;

/// One hop's answer.
enum Hop {
    Body(Vec<u8>),
    Redirect(String),
}

/// A WinHTTP handle, closed when dropped.
struct Handle(*mut std::ffi::c_void);

impl Handle {
    fn new(h: *mut std::ffi::c_void) -> Result<Handle, UpdateError> {
        if h.is_null() { Err(network(windows::core::Error::from_thread())) } else { Ok(Handle(h)) }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP returned, closed exactly once; children are closed first (they
        // are declared later, so dropped earlier).
        unsafe {
            let _ = windows::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0);
        }
    }
}

fn network(e: windows::core::Error) -> UpdateError {
    UpdateError::Network(e.message().trim_end().to_string())
}

impl Fetch for WinHttp {
    fn get(&self, url: &str, limit: Limit) -> Result<Vec<u8>, UpdateError> {
        let mut url = url.to_string();
        if !first_hop_allowed(&url) {
            return Err(UpdateError::Redirect(format!("{url:?} is not GitHub over HTTPS")));
        }
        let deadline = std::time::Instant::now() + limit.deadline;
        for _ in 0..=MAX_REDIRECTS {
            match get_once(&url, limit, deadline)? {
                Hop::Body(body) => return Ok(body),
                Hop::Redirect(location) => url = next_hop(&url, &location)?,
            }
        }
        Err(UpdateError::Redirect("too many redirects".into()))
    }
}

/// One GET of `url` (already allowed), redirects not followed; the body at most `limit.bytes`,
/// read before `deadline`.
fn get_once(url: &str, limit: Limit, deadline: std::time::Instant) -> Result<Hop, UpdateError> {
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{HSTRING, PCWSTR, w};
    let parts = https_parts(url).ok_or_else(|| UpdateError::Redirect(format!("{url:?}")))?;
    let (host, path) = (parts.host, parts.target);
    let late = || UpdateError::Network(format!("took longer than {} s", limit.deadline.as_secs()));
    if std::time::Instant::now() > deadline {
        return Err(late());
    }
    let ms = TIMEOUT.as_millis() as i32;
    let agent = HSTRING::from(user_agent());
    // SAFETY: WinHTTP calls on handles owned by `Handle`s that outlive every use; the strings
    // are NUL-terminated HSTRINGs or literals alive for each call; the out-parameters are
    // writable locals of the sizes passed.
    unsafe {
        let session = Handle::new(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0))?;
        WinHttpSetTimeouts(session.0, ms, ms, ms, ms).map_err(network)?;
        let never = WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes();
        WinHttpSetOption(Some(session.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&never)).map_err(network)?;
        let connect = Handle::new(WinHttpConnect(session.0, &HSTRING::from(host.as_str()), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let request = Handle::new(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let disable = (WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS).to_ne_bytes();
        WinHttpSetOption(Some(request.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&disable)).map_err(network)?;
        let headers: Vec<u16> = if host == API_HOST {
            "Accept: application/vnd.github+json\r\n".encode_utf16().collect()
        } else {
            Vec::new()
        };
        WinHttpSendRequest(request.0, (!headers.is_empty()).then_some(&headers[..]), None, 0, 0, 0).map_err(network)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(network)?;
        let mut status = 0u32;
        let mut len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(network)?;
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let mut len = 0u32;
            // The first call only asks for the size (it fails with ERROR_INSUFFICIENT_BUFFER).
            let _ = WinHttpQueryHeaders(request.0, WINHTTP_QUERY_LOCATION, PCWSTR::null(), None, &mut len, std::ptr::null_mut());
            let mut buf = vec![0u16; (len as usize).div_ceil(2) + 1];
            let mut len = (buf.len() * 2) as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_LOCATION,
                PCWSTR::null(),
                Some(buf.as_mut_ptr() as *mut _),
                &mut len,
                std::ptr::null_mut(),
            )
            .map_err(network)?;
            buf.truncate(len as usize / 2);
            return Ok(Hop::Redirect(String::from_utf16_lossy(&buf)));
        }
        if status != 200 {
            return Err(UpdateError::Status(status));
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).map_err(network)?;
            if available == 0 {
                return Ok(Hop::Body(body));
            }
            if std::time::Instant::now() > deadline {
                return Err(late());
            }
            let start = body.len();
            if start + available as usize > limit.bytes {
                return Err(UpdateError::Download(format!("over {} bytes", limit.bytes)));
            }
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(request.0, body[start..].as_mut_ptr() as *mut _, available, &mut read).map_err(network)?;
            body.truncate(start + read as usize);
        }
    }
}

/// The banner's text for an offered release.
pub fn banner_text(release: &Release) -> String {
    format!("Update to {}", release.tag())
}

/// What the Settings page says about a manual check.
pub const CHECKING: &str = "Checking…";
pub const UP_TO_DATE: &str = "You're up to date";
pub const CHECK_FAILED: &str = "Couldn't check for updates";

/// A check's outcome, delivered on the UI thread.
pub type Checked = Box<dyn FnOnce(Result<Option<Release>, UpdateError>)>;
/// A download's outcome (the saved installer), delivered on the UI thread.
pub type Downloaded = Box<dyn FnOnce(Result<PathBuf, UpdateError>)>;
/// The installer's start (UAC answered), delivered on the UI thread.
pub type Installed = Box<dyn FnOnce(Result<(), UpdateError>)>;

/// A downloaded installer to check and start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installing {
    pub msi: PathBuf,
    /// The release's digest, if it gives one.
    pub sha256: Option<Digest>,
    /// The UAC prompt's owner window (0: none).
    pub owner: isize,
}

/// What [`Updater`] runs. The app passes WinHTTP off the UI thread, msiexec and `begin_exit`
/// (`main.rs`); tests pass fakes.
pub struct UpdateIo {
    /// This is the installed copy: only it checks and updates.
    pub installed: bool,
    /// [`check`] against GitHub, off the UI thread ([`check_in_background`] in the app).
    pub check: Box<dyn Fn(Checked)>,
    /// [`download`] to [`update_dir`], off the UI thread ([`download_in_background`] in the app).
    pub download: Box<dyn Fn(Release, Downloaded)>,
    /// [`verify_and_launch`] with [`launch_msiexec`], off the UI thread
    /// ([`install_in_background`] in the app).
    pub install: Box<dyn Fn(Installing, Installed)>,
    /// Quits Negotium so the installer can replace it (`begin_exit`).
    pub exit: Box<dyn Fn()>,
    /// A log line (the startup check's failures are only logged).
    pub log: Box<dyn Fn(&str)>,
}

/// The window's side of updates: the startup and manual checks, the banner, the update.
pub struct Updater {
    io: UpdateIo,
    weak: slint::Weak<MainWindow>,
    /// A check is running; `manual`: its outcome is reported on the Settings page.
    checking: Cell<bool>,
    manual: Cell<bool>,
    /// The release the banner offers.
    offered: RefCell<Option<Release>>,
    /// Downloading or starting the installer: the banner waits.
    updating: Cell<bool>,
}

impl Updater {
    /// Connects "Check for updates" and the banner's buttons.
    pub fn install(ui: &MainWindow, io: UpdateIo) -> Rc<Updater> {
        let updater = Rc::new(Updater {
            io,
            weak: ui.as_weak(),
            checking: Cell::new(false),
            manual: Cell::new(false),
            offered: RefCell::new(None),
            updating: Cell::new(false),
        });
        {
            let u = updater.clone();
            ui.on_check_updates_now(move || u.check_now());
        }
        {
            let u = updater.clone();
            ui.on_update_accepted(move || u.accept());
        }
        {
            let u = updater.clone();
            ui.on_update_dismissed(move || u.dismiss());
        }
        updater
    }

    /// The window is up: the installed copy checks once if "Check for updates on startup" is on.
    pub fn startup(self: &Rc<Self>, check_updates: bool) {
        if startup_check_wanted(self.io.installed, check_updates) {
            self.begin_check();
        }
    }

    /// Settings "Check for updates": always (the installed copy), reported on the page.
    pub fn check_now(self: &Rc<Self>) {
        if !self.io.installed || self.updating.get() {
            return;
        }
        self.manual.set(true);
        self.set_status(CHECKING);
        self.begin_check();
    }

    /// One check at a time; a manual click during the startup check reports its outcome.
    fn begin_check(self: &Rc<Self>) {
        if self.checking.replace(true) {
            return;
        }
        let this = self.clone();
        (self.io.check)(Box::new(move |result| this.checked(result)));
    }

    fn checked(&self, result: Result<Option<Release>, UpdateError>) {
        self.checking.set(false);
        let manual = self.manual.take();
        // An update under way keeps its banner: a check that lands meanwhile changes nothing.
        if self.updating.get() {
            return;
        }
        match result {
            Ok(Some(release)) => {
                if manual {
                    self.set_status("");
                }
                (self.io.log)(&format!("update available: {} ({})", release.tag(), release.page_url));
                *self.offered.borrow_mut() = Some(release);
                self.show_banner();
            }
            Ok(None) => {
                if manual {
                    self.set_status(UP_TO_DATE);
                }
            }
            Err(e) => {
                (self.io.log)(&format!("update check failed: {e}"));
                if manual {
                    self.set_status(CHECK_FAILED);
                }
            }
        }
    }

    /// "Update": download the offered installer, check it and start it elevated, then quit. A
    /// declined UAC prompt or a failure says so, Negotium keeps running and the banner offers the
    /// update again.
    pub fn accept(self: &Rc<Self>) {
        let Some(release) = self.offered.borrow().clone() else { return };
        if self.updating.replace(true) {
            return;
        }
        self.show_banner();
        let this = self.clone();
        (self.io.download)(
            release.clone(),
            Box::new(move |saved| match saved {
                Err(e) => this.update_failed(&release, e),
                Ok(msi) => {
                    let owner = this.weak.upgrade().and_then(|ui| crate::platform::main_hwnd(&ui)).map_or(0, |h| h.0 as isize);
                    let job = Installing { msi: msi.clone(), sha256: release.sha256, owner };
                    let after = this.clone();
                    (this.io.install)(
                        job,
                        Box::new(move |started| match started {
                            Ok(()) => {
                                (after.io.log)(&format!("updating to {}: started msiexec on {}", release.tag(), msi.display()));
                                (after.io.exit)();
                            }
                            Err(e) => after.update_failed(&release, e),
                        }),
                    );
                }
            }),
        );
    }

    /// The update didn't start: logged, said, and offered again.
    fn update_failed(&self, release: &Release, e: UpdateError) {
        let tag = release.tag();
        self.updating.set(false);
        self.show_banner();
        let toast = if e == UpdateError::Cancelled {
            (self.io.log)(&format!("update to {tag} cancelled at the UAC prompt"));
            "Update cancelled".to_string()
        } else {
            (self.io.log)(&format!("update to {tag} failed: {e}"));
            format!("Couldn't update to {tag}: {e}.")
        };
        if let Some(ui) = self.weak.upgrade() {
            ui.set_toast(toast.into());
        }
    }

    /// "Dismiss": the banner goes until the next check finds a release.
    pub fn dismiss(&self) {
        if self.updating.get() {
            return;
        }
        *self.offered.borrow_mut() = None;
        self.show_banner();
    }

    fn show_banner(&self) {
        let Some(ui) = self.weak.upgrade() else { return };
        let offered = self.offered.borrow();
        let text = match offered.as_ref() {
            Some(r) if self.updating.get() => format!("Downloading {}…", r.tag()),
            Some(r) => banner_text(r),
            None => String::new(),
        };
        ui.set_update_banner(text.into());
        ui.set_update_busy(self.updating.get());
    }

    fn set_status(&self, text: &str) {
        if let Some(ui) = self.weak.upgrade() {
            ui.set_settings_update_status(text.into());
        }
    }
}

/// The app's [`UpdateIo::check`]: [`check`] through [`WinHttp`] on its own thread. Never called
/// by tests.
pub fn check_in_background(done: Checked) {
    crate::settingspage::in_background(move || check(&WinHttp, crate::install::VERSION), done);
}

/// The app's [`UpdateIo::download`]: [`download`] through [`WinHttp`] to [`update_dir`] on its
/// own thread. Never called by tests.
pub fn download_in_background(release: Release, done: Downloaded) {
    crate::settingspage::in_background(move || download(&WinHttp, &release, &update_dir()), done);
}

/// The app's [`UpdateIo::install`]: [`verify_and_launch`] with [`launch_msiexec`] on its own
/// thread (it waits for the UAC prompt). Never called by tests.
pub fn install_in_background(job: Installing, done: Installed) {
    crate::settingspage::in_background(
        move || {
            let log = |line: &str| crate::log::Log::new(crate::log::Log::default_path()).warn(line);
            verify_and_launch(&job.msi, job.sha256.as_ref(), &|msi| launch_msiexec(msi, job.owner), &log)
        },
        done,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const MSI: &str = "https://github.com/MyndPhreak/Negotium/releases/download/v0.2.0/Negotium-0.2.0.msi";
    const PAGE: &str = "https://github.com/MyndPhreak/Negotium/releases/tag/v0.2.0";
    /// Shaped like GitHub's signed redirects (S3 for the older store, Azure SAS for the newer).
    const OBJECTS_SIGNED: &str = "https://objects.githubusercontent.com/github-production-release-asset-2e65be/123456789/0b5e1f2a-3c4d-4e5f-8a9b-0c1d2e3f4a5b?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=releaseassetproduction%2F20251017%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20251017T120000Z&X-Amz-Expires=300&X-Amz-Signature=0f1e2d3c4b5a69788796a5b4c3d2e1f0&X-Amz-SignedHeaders=host&actor_id=0&key_id=0&repo_id=123456789&response-content-disposition=attachment%3B%20filename%3DNegotium-0.2.0.msi&response-content-type=application%2Foctet-stream";
    const RELEASE_ASSETS_SIGNED: &str = "https://release-assets.githubusercontent.com/github-production-release-asset/123456789/0b5e1f2a-3c4d-4e5f-8a9b-0c1d2e3f4a5b?sp=r&sv=2018-11-09&sr=b&spr=https&se=2025-10-17T12%3A45%3A00Z&rscd=attachment%3B+filename%3DNegotium-0.2.0.msi&rsct=application%2Foctet-stream&skoid=96c2d410-5711-43a1-aedd-ab1947aa7ab0&sktid=398a6654-997b-47e9-b12b-9515b896b4de&skt=2025-10-17T11%3A44%3A00Z&ske=2025-10-17T12%3A45%3A00Z&sks=b&skv=2018-11-09&sig=AbCdEf%2Fgh%2BIj%3D&jwt=eyJhbGciOiJIUzI1NiJ9.eyJyIjoxfQ.c2ln&response-content-disposition=attachment%3B%20filename%3DNegotium-0.2.0.msi&response-content-type=application%2Foctet-stream";

    fn json(tag: &str, draft: bool, prerelease: bool, assets: &[(&str, &str)]) -> String {
        let assets: Vec<_> =
            assets.iter().map(|(name, url)| serde_json::json!({ "name": name, "browser_download_url": url })).collect();
        serde_json::json!({
            "tag_name": tag, "draft": draft, "prerelease": prerelease, "html_url": PAGE, "assets": assets,
        })
        .to_string()
    }

    fn latest(tag: &str, assets: &[(&str, &str)]) -> String {
        json(tag, false, false, assets)
    }

    fn one_msi(tag: &str) -> String {
        let url = format!("https://github.com/MyndPhreak/Negotium/releases/download/{tag}/Negotium.msi");
        latest(tag, &[("Negotium.msi", &url)])
    }

    #[test]
    fn a_newer_release_is_offered() {
        let release = parse_latest(&one_msi("v0.2.0"), "0.1.0").unwrap();
        assert_eq!(release, Some(Release { version: (0, 2, 0), msi_url: "https://github.com/MyndPhreak/Negotium/releases/download/v0.2.0/Negotium.msi".into(), page_url: PAGE.into(), sha256: None }));
        assert_eq!(release.unwrap().tag(), "v0.2.0");
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(parse_latest(&one_msi("v0.10.0"), "0.9.0").unwrap().is_some(), "0.10 is newer than 0.9");
        assert!(parse_latest(&one_msi("v1.0.0"), "0.99.99").unwrap().is_some());
        assert!(parse_latest(&one_msi("v0.1.1"), "0.1.0").unwrap().is_some());
    }

    #[test]
    fn an_older_or_equal_release_is_not_offered() {
        assert_eq!(parse_latest(&one_msi("v0.1.0"), "0.1.0"), Ok(None), "equal");
        assert_eq!(parse_latest(&one_msi("v0.0.9"), "0.1.0"), Ok(None), "older");
        assert_eq!(parse_latest(&one_msi("v0.9.0"), "0.10.0"), Ok(None), "older, numerically");
    }

    #[test]
    fn prereleases_and_drafts_are_not_offered() {
        let assets = [("Negotium-0.2.0.msi", MSI)];
        assert_eq!(parse_latest(&json("v0.2.0", false, true, &assets), "0.1.0"), Ok(None), "prerelease");
        assert_eq!(parse_latest(&json("v0.2.0", true, false, &assets), "0.1.0"), Ok(None), "draft");
    }

    #[test]
    fn only_vxyz_tags_count() {
        for tag in ["v1.2", "latest", "1.2.3", "v1.2.3-beta", "v1.2.3.4", "V1.2.3", "v1..3", "v-1.2.3", "v1.2.x", "v+1.2.3", ""] {
            assert!(matches!(parse_latest(&one_msi(tag), "0.1.0"), Err(UpdateError::Release(_))), "{tag:?}");
        }
    }

    #[test]
    fn the_installer_must_be_on_github_over_https() {
        for store in [OBJECTS_SIGNED, RELEASE_ASSETS_SIGNED] {
            assert!(parse_latest(&latest("v0.2.0", &[("Negotium.msi", store)]), "0.1.0").unwrap().is_some(), "{store}");
        }
        for url in [
            "https://example.com/Negotium-0.2.0.msi",
            "https://github.com/MyndPhreak/Negotium/releases/download/v0.1.9/Negotium-0.2.0.msi",
            "https://github.com/MyndPhreak/Negotium/releases/download/Negotium-0.2.0.msi",
            "https://github.com/MyndPhreak/Negotium/releases/download/v0.2.0/sub/Negotium-0.2.0.msi",
            "https://github.com/MyndPhreak/Negotium/releases/download/v0.2.0%2FNegotium-0.2.0.msi",
            "http://release-assets.githubusercontent.com/x?sig=y",
            "https://release-assets.githubusercontent.com.evil.example/x",
            "http://github.com/MyndPhreak/Negotium/releases/download/v0.2.0/Negotium-0.2.0.msi",
            "https://github.com/Someone/Negotium/releases/download/v0.2.0/Negotium-0.2.0.msi",
            "https://github.com/MyndPhreak/Other/releases/download/v0.2.0/Negotium-0.2.0.msi",
            "https://github.com.evil.example/MyndPhreak/Negotium/releases/download/v0.2.0/N.msi",
            "https://evil.github.com/MyndPhreak/Negotium/releases/download/v0.2.0/N.msi",
            "https://github.com@evil.example/MyndPhreak/Negotium/releases/download/v0.2.0/N.msi",
            "https://github.com:8443/MyndPhreak/Negotium/releases/download/v0.2.0/N.msi",
            "https://github.com/MyndPhreak/Negotium/releases/download/../../../x/N.msi",
            "https://github.com\\@evil.example/N.msi",
            "ftp://github.com/MyndPhreak/Negotium/releases/download/v0.2.0/N.msi",
            "github.com/MyndPhreak/Negotium/releases/download/v0.2.0/N.msi",
        ] {
            let got = parse_latest(&latest("v0.2.0", &[("Negotium-0.2.0.msi", url)]), "0.1.0");
            assert!(matches!(got, Err(UpdateError::Release(_))), "{url}: {got:?}");
        }
    }

    #[test]
    fn exactly_one_msi_asset() {
        let two = latest("v0.2.0", &[("Negotium-0.2.0.msi", MSI), ("Negotium-0.2.0-x.MSI", MSI)]);
        assert!(matches!(parse_latest(&two, "0.1.0"), Err(UpdateError::Release(_))), "two");
        let none = latest("v0.2.0", &[("Negotium-0.2.0.zip", MSI)]);
        assert!(matches!(parse_latest(&none, "0.1.0"), Err(UpdateError::Release(_))), "missing");
        assert!(matches!(parse_latest(&latest("v0.2.0", &[]), "0.1.0"), Err(UpdateError::Release(_))), "no assets");
        let mixed = latest("v0.2.0", &[("checksums.txt", "https://example.com/c"), ("Negotium-0.2.0.msi", MSI)]);
        assert_eq!(parse_latest(&mixed, "0.1.0").unwrap().unwrap().msi_url, MSI, "other assets are ignored");
    }

    fn with_digest(digest: serde_json::Value) -> String {
        serde_json::json!({
            "tag_name": "v0.2.0", "draft": false, "prerelease": false, "html_url": PAGE,
            "assets": [{ "name": "Negotium-0.2.0.msi", "browser_download_url": MSI, "digest": digest }],
        })
        .to_string()
    }

    /// SHA-256("abc").
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn the_assets_digest_is_read() {
        let release = parse_latest(&with_digest(format!("sha256:{ABC}").into()), "0.1.0").unwrap().unwrap();
        assert_eq!(release.sha256, Some(sha256(b"abc").unwrap()));
        assert_eq!(parse_latest(&with_digest(serde_json::Value::Null), "0.1.0").unwrap().unwrap().sha256, None, "null: none");
        assert_eq!(parse_latest(&one_msi("v0.2.0"), "0.1.0").unwrap().unwrap().sha256, None, "absent: none");
        for bad in [
            format!("sha256:{}", &ABC[1..]),
            format!("sha256:{ABC}0"),
            format!("sha256:{}", ABC.replace('b', "g")),
            format!("sha512:{ABC}"),
            format!("SHA256 {ABC}"),
            ABC.to_string(),
            String::new(),
        ] {
            let got = parse_latest(&with_digest(bad.clone().into()), "0.1.0");
            assert!(matches!(got, Err(UpdateError::Release(_))), "{bad:?}: {got:?}");
        }
    }

    #[test]
    fn sha256_is_sha256() {
        let hex: String = sha256(b"abc").unwrap().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, ABC);
    }

    /// `verify_and_launch` with a recording launcher that answers `answer`; returns the result,
    /// what was launched and the log.
    fn verify(msi: &Path, expected: Option<&Digest>, answer: Result<(), UpdateError>) -> (Result<(), UpdateError>, Vec<PathBuf>, Vec<String>) {
        let launched = RefCell::new(Vec::new());
        let log = RefCell::new(Vec::new());
        let result = verify_and_launch(
            msi,
            expected,
            &|m| {
                launched.borrow_mut().push(m.to_path_buf());
                answer.clone()
            },
            &|l| log.borrow_mut().push(l.to_string()),
        );
        (result, launched.into_inner(), log.into_inner())
    }

    #[test]
    fn the_installer_is_checked_right_before_it_runs() {
        let dir = scratch("verify");
        std::fs::create_dir_all(&dir).unwrap();
        let msi = dir.join("Negotium-0.2.0.msi");
        std::fs::write(&msi, b"abc").unwrap();
        let good = sha256(b"abc").unwrap();

        let (result, launched, log) = verify(&msi, Some(&good), Ok(()));
        assert_eq!(result, Ok(()));
        assert_eq!(launched, std::slice::from_ref(&msi), "a match runs");
        assert!(log.is_empty());

        let (result, launched, log) = verify(&msi, None, Ok(()));
        assert_eq!(result, Ok(()));
        assert_eq!(launched, std::slice::from_ref(&msi), "no digest: runs, unverified");
        assert_eq!(log.len(), 1, "logged once: {log:?}");
        assert!(log[0].contains("unverified"));

        let (result, _, _) = verify(&msi, Some(&good), Err(UpdateError::Cancelled));
        assert_eq!(result, Err(UpdateError::Cancelled), "a declined prompt comes back as such");

        // Swapped after the download: refused, deleted, never run.
        std::fs::write(&msi, b"abd").unwrap();
        let (result, launched, _) = verify(&msi, Some(&good), Ok(()));
        assert!(matches!(result, Err(UpdateError::Digest(_))), "{result:?}");
        assert!(launched.is_empty(), "a mismatch never runs");
        assert!(!msi.exists(), "and is deleted");

        let (result, launched, _) = verify(&msi, Some(&good), Ok(()));
        assert!(matches!(result, Err(UpdateError::Io(_))), "gone: {result:?}");
        assert!(launched.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_json_is_an_error() {
        for text in ["", "{", "[]", "null", r#"{"tag_name": 5}"#, r#"{"draft": false}"#] {
            assert!(matches!(parse_latest(text, "0.1.0"), Err(UpdateError::Json(_))), "{text:?}");
        }
    }

    /// Answers each URL from a table and records what was asked; never the network.
    #[derive(Default)]
    struct FakeFetch {
        answers: Vec<(String, Result<Vec<u8>, UpdateError>)>,
        asked: RefCell<Vec<String>>,
        limits: RefCell<Vec<Limit>>,
    }

    impl FakeFetch {
        fn with(url: &str, answer: Result<Vec<u8>, UpdateError>) -> FakeFetch {
            FakeFetch { answers: vec![(url.into(), answer)], ..Default::default() }
        }
    }

    impl Fetch for FakeFetch {
        fn get(&self, url: &str, limit: Limit) -> Result<Vec<u8>, UpdateError> {
            self.asked.borrow_mut().push(url.into());
            self.limits.borrow_mut().push(limit);
            self.answers.iter().find(|(u, _)| u == url).map(|(_, a)| a.clone()).unwrap_or(Err(UpdateError::Status(404)))
        }
    }

    #[test]
    fn check_asks_the_latest_release_of_the_repository() {
        assert_eq!(LATEST_URL, "https://api.github.com/repos/MyndPhreak/Negotium/releases/latest");
        let fetch = FakeFetch::with(LATEST_URL, Ok(one_msi("v0.2.0").into_bytes()));
        assert_eq!(check(&fetch, "0.1.0").unwrap().unwrap().version, (0, 2, 0));
        assert_eq!(*fetch.asked.borrow(), [LATEST_URL]);
        assert_eq!(*fetch.limits.borrow(), [JSON_LIMIT], "a small cap for JSON");
        assert_eq!(JSON_LIMIT.bytes, 2 * 1024 * 1024);
        let offline = FakeFetch::with(LATEST_URL, Err(UpdateError::Network("offline".into())));
        assert_eq!(check(&offline, "0.1.0"), Err(UpdateError::Network("offline".into())));
        let garbage = FakeFetch::with(LATEST_URL, Ok(vec![0xff, 0xfe]));
        assert!(matches!(check(&garbage, "0.1.0"), Err(UpdateError::Json(_))));
    }

    /// An OLE compound file's signature: every MSI starts with it.
    const MSI_BYTES: [u8; 12] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 1, 2, 3, 4];

    fn release() -> Release {
        Release { version: (0, 2, 0), msi_url: MSI.into(), page_url: PAGE.into(), sha256: None }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("negotium-update-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn download_saves_the_msi_by_version_and_overwrites() {
        let dir = scratch("download");
        let fetch = FakeFetch::with(MSI, Ok(MSI_BYTES.to_vec()));
        let path = download(&fetch, &release(), &dir).unwrap();
        assert_eq!(path, dir.join("Negotium-0.2.0.msi"));
        assert_eq!(std::fs::read(&path).unwrap(), MSI_BYTES);
        assert_eq!(*fetch.asked.borrow(), [MSI]);
        assert_eq!(*fetch.limits.borrow(), [MSI_LIMIT]);
        assert_eq!(MSI_LIMIT.deadline, std::time::Duration::from_secs(600), "a trickle ends");
        let mut newer = MSI_BYTES.to_vec();
        newer.push(9);
        let path = download(&FakeFetch::with(MSI, Ok(newer.clone())), &release(), &dir).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), newer, "an earlier download is replaced");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing else is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_refuses_what_is_not_an_msi() {
        let dir = scratch("refuse");
        for body in [vec![], b"<html>rate limited</html>".to_vec()] {
            let got = download(&FakeFetch::with(MSI, Ok(body.clone())), &release(), &dir);
            assert!(matches!(got, Err(UpdateError::Download(_))), "{body:?}: {got:?}");
        }
        assert!(msi_size_ok(1) && msi_size_ok(MAX_MSI_BYTES));
        assert!(!msi_size_ok(0) && !msi_size_ok(MAX_MSI_BYTES + 1));
        const { assert!(MAX_MSI_BYTES <= 200 * 1024 * 1024) };
        // A release that didn't come through `parse_latest` is still checked before any fetch.
        let foreign = Release { msi_url: "https://example.com/N.msi".into(), ..release() };
        let fetch = FakeFetch::with("https://example.com/N.msi", Ok(MSI_BYTES.to_vec()));
        assert!(matches!(download(&fetch, &foreign, &dir), Err(UpdateError::Release(_))));
        assert!(fetch.asked.borrow().is_empty(), "never fetched");
        assert!(!dir.join("Negotium-0.2.0.msi").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_update_goes_to_temp() {
        assert_eq!(update_dir(), std::env::temp_dir().join("Negotium-update"));
    }

    #[test]
    fn redirects_stay_on_github_over_https() {
        // GitHub's signed download redirects: encoded slashes in the query are normal there.
        for store in [OBJECTS_SIGNED, RELEASE_ASSETS_SIGNED] {
            assert_eq!(next_hop(MSI, store), Ok(store.to_string()), "{store}");
            assert_eq!(next_hop(store, store), Ok(store.to_string()), "store to store");
        }
        assert_eq!(next_hop(LATEST_URL, "/repos/x"), Ok("https://api.github.com/repos/x".to_string()), "relative");
        let same_release = "https://github.com/MyndPhreak/Negotium/releases/download/v0.2.0/Other.msi";
        assert_eq!(next_hop(MSI, same_release), Ok(same_release.to_string()), "the same release's downloads");
        for to in [
            "http://objects.githubusercontent.com/x",
            "http://release-assets.githubusercontent.com/x?sig=a%2Fb",
            "https://example.com/x",
            "//example.com/x",
            "https://objects.githubusercontent.com.evil.example/x",
            "https://release-assets.githubusercontent.com/a/%2e%2e/b?sig=x",
            "https://github.com/MyndPhreak/Negotium/releases/download/v0.1.0/Negotium-0.1.0.msi",
            "https://github.com/Someone/Negotium/releases/download/v0.2.0/N.msi",
            "https://github.com/login?return_to=x",
            "https://api.github.com/repos/x",
            "",
        ] {
            assert!(matches!(next_hop(MSI, to), Err(UpdateError::Redirect(_))), "{to:?}");
        }
        assert!(next_hop(LATEST_URL, OBJECTS_SIGNED).is_err(), "the API never redirects to a download");
        assert!(next_hop(OBJECTS_SIGNED, MSI).is_err(), "nor a store back to github.com");
        assert!(next_hop(OBJECTS_SIGNED, LATEST_URL).is_err());
    }

    #[test]
    fn a_request_asks_the_path_and_query_without_the_fragment() {
        let p = https_parts("https://Release-Assets.githubusercontent.com/a/b?sig=a%2Fb&x=1#frag").unwrap();
        assert_eq!((p.host.as_str(), p.path, p.target), ("release-assets.githubusercontent.com", "/a/b", "/a/b?sig=a%2Fb&x=1"));
        assert_eq!(https_parts("https://api.github.com").unwrap().target, "/");
        assert!(https_parts("https://github.com/a%2fb?x").is_none(), "an encoded slash in the path");
        assert!(https_parts("https://github.com/a/./b").is_none());
        assert!(first_hop_allowed(LATEST_URL) && first_hop_allowed(MSI) && first_hop_allowed(OBJECTS_SIGNED));
        assert!(!first_hop_allowed("https://github.com/MyndPhreak/Negotium") && !first_hop_allowed("https://example.com/"));
    }

    #[test]
    fn msiexec_installs_with_progress_only() {
        let msi = Path::new(r"C:\Users\Some One\AppData\Local\Temp\Negotium-update\Negotium-0.2.0.msi");
        assert_eq!(installer_args(msi), r#"/i "C:\Users\Some One\AppData\Local\Temp\Negotium-update\Negotium-0.2.0.msi" /passive"#);
    }

    #[test]
    fn the_startup_check_never_runs_on_a_dev_build() {
        assert!(!startup_check_wanted(false, true), "a development build never checks");
        assert!(!startup_check_wanted(false, false));
        assert!(!startup_check_wanted(true, false), "the switch is off");
        assert!(startup_check_wanted(true, true));
    }

    #[test]
    fn requests_name_negotium_and_its_version() {
        assert_eq!(user_agent(), format!("Negotium/{}", crate::install::VERSION));
        assert_eq!(TIMEOUT, std::time::Duration::from_secs(10));
    }
}
