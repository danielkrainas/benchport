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
    pub fn parse(s: &str, base: Option<&Location>) -> Result<Location> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::msg("empty location"));
        }
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Ok(Location::Remote(Url::parse(s)?));
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
            Some(Location::Remote(u)) => Ok(Location::Remote(u.join(s)?)),
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
    fn file_urls_become_paths() {
        assert!(matches!(Location::parse("file:///tmp/x.json", None).unwrap(), Location::Local(_)));
        assert!(Location::parse("   ", None).is_err());
    }
}
