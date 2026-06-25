//! GitHub release discovery and binary download logic for the Kleos installer.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::InstallError;

/// User-Agent header value sent with every GitHub API and download request.
const USER_AGENT: &str = "kleos-installer/1.2.1";

/// A single downloadable asset attached to a GitHub release.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReleaseAsset {
    /// Filename of the asset as it appears in the release.
    pub name: String,
    /// Direct download URL for the asset binary.
    pub browser_download_url: String,
    /// File size in bytes reported by GitHub.
    pub size: u64,
}

/// A GitHub release, containing metadata and a list of downloadable assets.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Release {
    /// The Git tag name for this release (e.g. "v1.2.1").
    pub tag_name: String,
    /// All assets attached to this release.
    pub assets: Vec<ReleaseAsset>,
}

/// Callback interface for reporting download progress to the UI layer.
///
/// Implemented by both TUI and GUI frontends so that download logic is
/// completely decoupled from presentation.
pub trait DownloadProgress: Send + Sync {
    /// Called periodically as bytes arrive from the remote server.
    fn on_progress(&self, component: &str, bytes_downloaded: u64, total_bytes: u64);
    /// Called once when download completes and checksum verification begins.
    fn on_verifying(&self, component: &str);
    /// Called once when verification succeeds and the file is ready.
    fn on_complete(&self, component: &str);
    /// Called if the download or verification step fails.
    fn on_error(&self, component: &str, error: &str);
}

/// Fetch the latest published GitHub release for the given `owner/repo`.
///
/// Uses the GitHub REST API v3 endpoint `GET /repos/{repo}/releases/latest`.
/// Returns the parsed `Release` or an `InstallError::GitHub` on failure.
pub fn fetch_latest_release(repo: &str) -> Result<Release, InstallError> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    fetch_release_url(&url)
}

/// Fetch a specific tagged GitHub release for the given `owner/repo`.
///
/// Uses the GitHub REST API v3 endpoint `GET /repos/{repo}/releases/tags/{tag}`.
/// Returns the parsed `Release` or an `InstallError::GitHub` on failure.
pub fn fetch_release(repo: &str, tag: &str) -> Result<Release, InstallError> {
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    fetch_release_url(&url)
}

/// Internal helper that performs the actual HTTP GET and JSON parsing.
fn fetch_release_url(url: &str) -> Result<Release, InstallError> {
    let client = build_client()?;
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| InstallError::GitHub(format!("request failed: {e}")))?;

    if !response.status().is_success() {
        return Err(InstallError::GitHub(format!(
            "API returned status {}",
            response.status()
        )));
    }

    response
        .json::<Release>()
        .map_err(|e| InstallError::GitHub(format!("failed to parse release JSON: {e}")))
}

/// Download and parse the `SHA256SUMS` asset from a release.
///
/// Looks for an asset named exactly `SHA256SUMS` in `release.assets`,
/// downloads it as text, and parses each line as `<hash>  <filename>` into
/// a `HashMap<filename, hex_hash>`. Returns `InstallError::GitHub` if the
/// asset is missing or malformed.
pub fn fetch_checksums(release: &Release) -> Result<HashMap<String, String>, InstallError> {
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or_else(|| InstallError::GitHub("SHA256SUMS asset not found in release".to_string()))?;

    let client = build_client()?;
    let text = client
        .get(&asset.browser_download_url)
        .send()
        .map_err(|e| InstallError::Download(format!("failed to fetch SHA256SUMS: {e}")))?
        .text()
        .map_err(|e| InstallError::Download(format!("failed to read SHA256SUMS body: {e}")))?;

    let mut map = HashMap::new();
    for line in text.lines() {
        // Format: "<hash>  <filename>" (two spaces) or "<hash> <filename>"
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() == 2 {
            let hash = parts[0].trim().to_string();
            let name = parts[1].trim().to_string();
            if !hash.is_empty() && !name.is_empty() {
                map.insert(name, hash);
            }
        }
    }

    Ok(map)
}

/// Download a release asset to `dest`, optionally verifying its SHA-256 checksum.
///
/// Streams the response body in chunks, calling `progress.on_progress` for each
/// chunk. After the download completes, calls `progress.on_verifying` and then
/// verifies the checksum if `expected_sha256` is provided. On success calls
/// `progress.on_complete`. On any failure calls `progress.on_error` and returns
/// an `InstallError`.
pub fn download_component(
    asset: &ReleaseAsset,
    dest: &Path,
    expected_sha256: Option<&str>,
    progress: &dyn DownloadProgress,
) -> Result<(), InstallError> {
    let component = &asset.name;
    let client = build_client()?;

    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .map_err(|e| {
            let msg = format!("download request failed: {e}");
            progress.on_error(component, &msg);
            InstallError::Download(msg)
        })?;

    if !response.status().is_success() {
        let msg = format!("server returned status {}", response.status());
        progress.on_error(component, &msg);
        return Err(InstallError::Download(msg));
    }

    let total = asset.size;
    let mut file = std::fs::File::create(dest).map_err(|e| {
        let msg = format!("cannot create destination file: {e}");
        progress.on_error(component, &msg);
        InstallError::Io(e)
    })?;

    let mut downloaded: u64 = 0;
    let mut buf = vec![0u8; 65536];

    loop {
        let n = response.read(&mut buf).map_err(|e| {
            let msg = format!("read error: {e}");
            progress.on_error(component, &msg);
            InstallError::Download(msg)
        })?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| {
            let msg = format!("write error: {e}");
            progress.on_error(component, &msg);
            InstallError::Io(e)
        })?;
        downloaded += n as u64;
        progress.on_progress(component, downloaded, total);
    }

    // A missing checksum must not be treated as success: refuse to install a
    // binary that is absent from the published SHA256SUMS rather than running
    // an unverified executable.
    let Some(expected) = expected_sha256 else {
        let msg = format!(
            "no published SHA-256 checksum for {component}; refusing to install unverified binary"
        );
        progress.on_error(component, &msg);
        return Err(InstallError::Download(msg));
    };
    progress.on_verifying(component);
    verify_checksum(dest, expected).inspect_err(|e| {
        let msg = e.to_string();
        progress.on_error(component, &msg);
    })?;

    progress.on_complete(component);
    Ok(())
}

/// Verify that the file at `path` matches the expected SHA-256 hex digest.
///
/// Reads the file in 64 KiB chunks and computes its SHA-256 digest. Returns
/// `InstallError::Checksum` if the digests do not match.
pub fn verify_checksum(path: &Path, expected: &str) -> Result<(), InstallError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 65536];

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let actual = hex::encode(hasher.finalize());
    let expected_lower = expected.to_lowercase();

    if actual != expected_lower {
        return Err(InstallError::Checksum {
            expected: expected_lower,
            actual,
        });
    }

    Ok(())
}

/// Build a `reqwest` blocking HTTP client with the installer User-Agent set.
fn build_client() -> Result<reqwest::blocking::Client, InstallError> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        // Bound the connect phase so a dead host fails fast; keep the overall
        // timeout generous since release binaries can be large on slow links.
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| InstallError::Download(format!("failed to build HTTP client: {e}")))
}
