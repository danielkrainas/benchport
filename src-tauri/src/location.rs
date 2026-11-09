//! A "location" is anything the launcher can fetch from: an http(s) URL or a
//! local path. Supporting local paths makes it trivial to test a catalog
//! without a web server, or to distribute builds over a shared/LAN drive.
//! Relative references inside a catalog are resolved against the catalog's
//! own location, so a static folder of `catalog.json` + zips just works.

use std::path::{Path, PathBuf};

use base64::Engine;
use url::Url;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub enum Location {
    Remote(Url),
    Local(PathBuf),
}

impl Location {
    /// Parse `s`, resolving relative references against `base`.
    ///
    /// A catalog on the web may only point at web addresses: an absolute path
    /// or `file://` URL in it is rejected, so a third-party catalog can't make
    /// the launcher read (or show) files from the player's disk.
    pub fn parse(s: &str, base: Option<&Location>) -> Result<Location> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::msg("empty location"));
        }
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Ok(Location::Remote(Url::parse(s)?));
        }
        if let Some(Location::Remote(u)) = base {
            if lower.starts_with("file:") || Path::new(s).is_absolute() || Url::parse(s).is_ok() {
                return Err(Error::msg(format!("\"{s}\" isn't a web address (a catalog on the web can't refer to local files)")));
            }
            return Ok(Location::Remote(u.join(s)?));
        }
        if lower.starts_with("file://") {
            let url = Url::parse(s)?;
            return url
                .to_file_path()
                .map(Location::Local)
                .map_err(|_| Error::msg(format!("invalid file URL: {s}")));
        }
        let path = Path::new(s);
        if path.is_absolute() {
            return Ok(Location::Local(path.to_path_buf()));
        }
        match base {
            Some(Location::Remote(_)) => unreachable!("handled above"),
            Some(Location::Local(p)) => {
                let dir = if p.is_dir() { p.as_path() } else { p.parent().unwrap_or(Path::new(".")) };
                Ok(Location::Local(dir.join(s)))
            }
            None => Ok(Location::Local(std::env::current_dir()?.join(s))),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Location::Remote(u) => u.to_string(),
            Location::Local(p) => p.display().to_string(),
        }
    }

    /// Fetch, skipping the body when the server says the copy we have (per
    /// its `ETag` / `Last-Modified`) is still current. Local files are
    /// always read.
    pub async fn fetch_if_changed(&self, client: &reqwest::Client, etag: Option<&str>, last_modified: Option<&str>) -> Result<Fetched> {
        match self {
            Location::Remote(u) => {
                let mut req = client.get(u.clone());
                if let Some(e) = etag {
                    req = req.header(reqwest::header::IF_NONE_MATCH, e);
                }
                if let Some(m) = last_modified {
                    req = req.header(reqwest::header::IF_MODIFIED_SINCE, m);
                }
                let resp = req.send().await?;
                if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
                    return Ok(Fetched::NotModified);
                }
                let resp = resp.error_for_status()?;
                let header = |h| resp.headers().get(h).and_then(|v| v.to_str().ok()).map(str::to_string);
                let (etag, last_modified) = (header(reqwest::header::ETAG), header(reqwest::header::LAST_MODIFIED));
                Ok(Fetched::Body { bytes: resp.bytes().await?.to_vec(), etag, last_modified })
            }
            Location::Local(_) => Ok(Fetched::Body { bytes: self.fetch(client).await?, etag: None, last_modified: None }),
        }
    }

    pub async fn fetch(&self, client: &reqwest::Client) -> Result<Vec<u8>> {
        match self {
            Location::Remote(u) => {
                let resp = client.get(u.clone()).send().await?.error_for_status()?;
                Ok(resp.bytes().await?.to_vec())
            }
            Location::Local(p) => tokio::fs::read(p)
                .await
                .map_err(|e| Error::msg(format!("could not read {}: {e}", p.display()))),
        }
    }
}

pub enum Fetched {
    NotModified,
    Body { bytes: Vec<u8>, etag: Option<String>, last_modified: Option<String> },
}

/// Turn an image reference into something the webview can display:
/// remote URLs pass through, local files are inlined as data URLs.
pub fn resolve_image(s: &Option<String>, base: Option<&Location>) -> Option<String> {
    let s = s.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with("data:") {
        return Some(s.to_string());
    }
    match Location::parse(s, base).ok()? {
        Location::Remote(u) => Some(u.to_string()),
        Location::Local(p) => {
            let bytes = std::fs::read(&p).ok()?;
            let mime = match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
                Some("png") => "image/png",
                Some("jpg") | Some("jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                Some("svg") => "image/svg+xml",
                Some("ico") => "image/x-icon",
                _ => "application/octet-stream",
            };
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            Some(format!("data:{mime};base64,{b64}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_against_remote_catalog() {
        let base = Location::parse("https://cdn.example.com/games/catalog.json", None).unwrap();
        match Location::parse("builds/a.zip", Some(&base)).unwrap() {
            Location::Remote(u) => assert_eq!(u.as_str(), "https://cdn.example.com/games/builds/a.zip"),
            _ => panic!("expected remote"),
        }
        match Location::parse("https://other.example.com/x.zip", Some(&base)).unwrap() {
            Location::Remote(u) => assert_eq!(u.host_str(), Some("other.example.com")),
            _ => panic!("expected remote"),
        }
    }

    #[test]
    fn resolves_relative_against_local_file() {
        let base = Location::Local(PathBuf::from("/srv/demo/catalog.json"));
        match Location::parse("builds/a.zip", Some(&base)).unwrap() {
            Location::Local(p) => assert_eq!(p, PathBuf::from("/srv/demo/builds/a.zip")),
            _ => panic!("expected local"),
        }
    }

    #[test]
    fn web_catalogs_cant_reach_local_files() {
        let base = Location::parse("https://cdn.example.com/catalog.json", None).unwrap();
        for evil in ["/etc/passwd", "file:///etc/passwd", "C:\\Windows\\win.ini", "data:text/html,hi", "javascript:alert(1)"] {
            assert!(Location::parse(evil, Some(&base)).is_err(), "{evil} should be rejected");
        }
        // Relative paths (even with ..) stay on the web.
        match Location::parse("../builds/a.zip", Some(&base)).unwrap() {
            Location::Remote(u) => assert_eq!(u.as_str(), "https://cdn.example.com/builds/a.zip"),
            _ => panic!("expected remote"),
        }
    }

    #[test]
    fn file_urls_become_paths() {
        assert!(matches!(Location::parse("file:///tmp/x.json", None).unwrap(), Location::Local(_)));
        assert!(Location::parse("   ", None).is_err());
    }
}
