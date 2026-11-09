//! The catalogs (sources) this launcher follows, persisted as `sources.json`.
//!
//! The `catalogUrl` from launcher.json becomes a pinned source (`fromConfig`)
//! whose URL keeps following the config, so a single-catalog build behaves
//! exactly as before. Players can add more catalogs from Settings or through a
//! `toybox://add?url=…` link; each is fetched, cached and refreshed on its own.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::library::{now, write_atomic};
use crate::location::Location;

/// Id given to the source created from launcher.json's `catalogUrl`.
pub const CONFIG_SOURCE_ID: &str = "default";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// Stable id; also the folder games from this source install into.
    pub id: String,
    /// The catalog location (URL or local path), as given.
    pub url: String,
    /// Name from the `toybox://add?name=…` link, used until the catalog
    /// provides its own.
    #[serde(default)]
    pub label: Option<String>,

    // ---- Copied from the catalog's top-level fields on each refresh ----
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    /// Raw icon reference (resolved against `url` when displayed).
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub support_url: Option<String>,
    #[serde(default)]
    pub accent: Option<String>,

    #[serde(default = "yes")]
    pub enabled: bool,
    /// Pinned to launcher.json's `catalogUrl`; can't be removed from the UI.
    #[serde(default)]
    pub from_config: bool,
    #[serde(default)]
    pub added_at: u64,
    /// Last refresh attempt.
    #[serde(default)]
    pub last_checked: Option<u64>,
    /// When the cached copy was last confirmed current.
    #[serde(default)]
    pub fetched_at: Option<u64>,
    /// HTTP cache validators for conditional refreshes.
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
}

fn yes() -> bool {
    true
}

impl Source {
    pub fn location(&self) -> Result<Location> {
        Location::parse(&self.url, None)
    }

    /// Best name to show: the catalog's, then the link's, then the host/file.
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.label.clone())
            .or_else(|| self.publisher.clone())
            .unwrap_or_else(|| match self.location() {
                Ok(Location::Remote(u)) => u.host_str().unwrap_or("Catalog").to_string(),
                Ok(Location::Local(p)) => p
                    .parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Local catalog".into()),
                Err(_) => "Catalog".into(),
            })
    }

    /// Copy the catalog's publisher info; returns whether anything changed.
    pub fn absorb(&mut self, c: &Catalog) -> bool {
        let clean = |v: &Option<String>| v.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let next = (
            clean(&c.name),
            clean(&c.publisher),
            clean(&c.icon),
            clean(&c.description),
            clean(&c.website),
            clean(&c.support_url),
            clean(&c.accent).filter(|a| valid_color(a)),
        );
        let cur = (
            self.name.clone(),
            self.publisher.clone(),
            self.icon.clone(),
            self.description.clone(),
            self.website.clone(),
            self.support_url.clone(),
            self.accent.clone(),
        );
        if next == cur {
            return false;
        }
        (self.name, self.publisher, self.icon, self.description, self.website, self.support_url, self.accent) = next;
        true
    }
}

fn valid_color(s: &str) -> bool {
    let h = s.strip_prefix('#').unwrap_or("");
    (h.len() == 6 || h.len() == 3) && h.chars().all(|c| c.is_ascii_hexdigit())
}

/// A source the player removed. Its library games keep their source id, so
/// re-adding the same catalog later reconnects them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedSource {
    pub id: String,
    pub url: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceList {
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub removed: Vec<RemovedSource>,
}

impl SourceList {
    pub fn load(path: &Path) -> SourceList {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("sources.json is corrupt ({e}); starting fresh");
                let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
                SourceList::default()
            }),
            Err(_) => SourceList::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }

    pub fn get(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Source> {
        self.sources.iter_mut().find(|s| s.id == id)
    }

    pub fn find_url(&self, url: &str) -> Option<&Source> {
        let url = normalize(url).ok()?;
        self.sources.iter().find(|s| normalize(&s.url).ok().as_deref() == Some(url.as_str()))
    }

    pub fn config_source(&self) -> Option<&Source> {
        self.sources.iter().find(|s| s.from_config)
    }

    /// Keep the pinned source in line with launcher.json's catalog. Returns
    /// whether the list changed, and the id of a source whose URL changed
    /// (its cache is stale).
    pub fn sync_config(&mut self, config_url: Option<&str>) -> (bool, Option<String>) {
        let Some(url) = config_url else {
            // No catalog in the config any more: keep the source, but as an
            // ordinary one the player can manage.
            let mut changed = false;
            for s in self.sources.iter_mut().filter(|s| s.from_config) {
                s.from_config = false;
                changed = true;
            }
            return (changed, None);
        };
        if let Some(s) = self.sources.iter_mut().find(|s| s.from_config) {
            if normalize(&s.url).ok() == normalize(url).ok() {
                return (false, None);
            }
            // The config was pointed somewhere else. Like before multi-catalog
            // support, the same library follows it.
            s.url = url.to_string();
            (s.etag, s.last_modified, s.fetched_at) = (None, None, None);
            return (true, Some(s.id.clone()));
        }
        if let Some(i) = self.find_url(url).map(|s| s.id.clone()).and_then(|id| self.sources.iter().position(|s| s.id == id)) {
            // The player already added this catalog themselves.
            self.sources[i].from_config = true;
            let s = self.sources.remove(i);
            self.sources.insert(0, s);
            return (true, None);
        }
        let id = if self.get(CONFIG_SOURCE_ID).is_some() || self.removed.iter().any(|r| r.id == CONFIG_SOURCE_ID) {
            new_id(url)
        } else {
            CONFIG_SOURCE_ID.to_string()
        };
        self.removed.retain(|r| r.id != id);
        self.sources.insert(0, Source { id, url: url.to_string(), enabled: true, from_config: true, added_at: now(), ..Default::default() });
        (true, None)
    }

    /// Add a catalog (or re-enable it if it's already here). Reuses the id of
    /// a previously removed source with the same URL so its library games
    /// reconnect. Returns the source id and whether it's new.
    pub fn add(&mut self, url: &str, label: Option<String>) -> Result<(String, bool)> {
        let url = normalize(url)?;
        if let Some(id) = self.find_url(&url).map(|s| s.id.clone()) {
            let s = self.get_mut(&id).unwrap();
            s.enabled = true;
            return Ok((id, false));
        }
        let id = match self.removed.iter().position(|r| normalize(&r.url).ok().as_deref() == Some(url.as_str())) {
            Some(i) => self.removed.remove(i).id,
            None => new_id(&url),
        };
        let label = label.map(|l| l.trim().chars().take(80).collect::<String>()).filter(|l| !l.is_empty());
        self.sources.push(Source { id: id.clone(), url, label, enabled: true, added_at: now(), ..Default::default() });
        Ok((id, true))
    }

    pub fn remove(&mut self, id: &str) -> Result<Source> {
        let i = self.sources.iter().position(|s| s.id == id).ok_or_else(|| Error::msg("That catalog isn't added."))?;
        if self.sources[i].from_config {
            return Err(Error::msg("This catalog is part of the launcher's configuration and can't be removed. You can turn it off instead."));
        }
        let s = self.sources.remove(i);
        self.removed.retain(|r| r.id != s.id);
        self.removed.push(RemovedSource { id: s.id.clone(), url: s.url.clone(), name: s.display_name() });
        Ok(s)
    }
}

/// Canonical form of a catalog location, for duplicate detection.
pub fn normalize(url: &str) -> Result<String> {
    Ok(Location::parse(url, None)?.display())
}

/// A short, filename-safe id that's unique per add.
pub fn new_id(seed: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let digest = Sha256::digest(format!("{seed}|{nanos}|{}", std::process::id()));
    format!("s{}", &hex::encode(digest)[..8])
}

// ---------------------------------------------------------------- export / import

/// The file written by "Export catalogs" and accepted by "Add catalog".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    /// Format marker/version; distinguishes an export from a catalog.
    pub toybox_sources: u32,
    #[serde(default)]
    pub exported_at: u64,
    pub sources: Vec<ExportedSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedSource {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl SourceList {
    pub fn export(&self) -> Export {
        Export {
            toybox_sources: 1,
            exported_at: now(),
            sources: self
                .sources
                .iter()
                .map(|s| ExportedSource { url: s.url.clone(), name: Some(s.display_name()), enabled: s.enabled })
                .collect(),
        }
    }
}

/// What a fetched file turned out to be.
pub enum Parsed {
    Catalog(Catalog),
    Export(Export),
}

pub fn parse_any(bytes: &[u8]) -> Result<Parsed> {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) {
        if v.get("toyboxSources").is_some() {
            return Ok(Parsed::Export(serde_json::from_value(v)?));
        }
    }
    Ok(Parsed::Catalog(crate::catalog::parse(bytes).map_err(|e| Error::msg(format!("That isn't a game catalog: {e}")))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_catalog_becomes_the_default_source() {
        let mut l = SourceList::default();
        assert_eq!(l.sync_config(Some("https://a.example/catalog.json")), (true, None));
        assert_eq!(l.sync_config(Some("https://a.example/catalog.json")), (false, None));
        let s = l.config_source().unwrap();
        assert_eq!((s.id.as_str(), s.enabled), (CONFIG_SOURCE_ID, true));

        // Repointing the config keeps the same source (and library).
        let (changed, stale) = l.sync_config(Some("https://b.example/catalog.json"));
        assert!(changed && stale.as_deref() == Some(CONFIG_SOURCE_ID));
        assert_eq!(l.sources.len(), 1);
        assert!(l.remove(CONFIG_SOURCE_ID).is_err(), "config source is pinned");

        // Dropping catalogUrl turns it into an ordinary source.
        l.sync_config(None);
        assert!(l.config_source().is_none() && l.remove(CONFIG_SOURCE_ID).is_ok());
    }

    #[test]
    fn adding_dedupes_and_readding_reconnects() {
        let mut l = SourceList::default();
        let (a, new) = l.add("https://pub.example/c.json", Some("Pub".into())).unwrap();
        assert!(new && a.starts_with('s') && a.len() == 9);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "ids double as folder names");
        assert_eq!(l.add("https://pub.example/c.json", None).unwrap(), (a.clone(), false));
        assert_eq!(l.get(&a).unwrap().display_name(), "Pub");

        l.remove(&a).unwrap();
        assert!(l.sources.is_empty() && l.removed.len() == 1);
        let (again, new) = l.add("https://pub.example/c.json", None).unwrap();
        assert!(new);
        assert_eq!(again, a, "re-adding reuses the old id so library games reconnect");
        assert!(l.removed.is_empty());
    }

    #[test]
    fn config_url_matching_a_user_source_adopts_it() {
        let mut l = SourceList::default();
        let (id, _) = l.add("https://pub.example/c.json", None).unwrap();
        l.sync_config(Some("https://pub.example/c.json"));
        assert_eq!(l.sources.len(), 1);
        assert_eq!(l.config_source().unwrap().id, id);
    }

    #[test]
    fn absorbs_catalog_info_and_ignores_bad_colors() {
        let mut s = Source { url: "https://x.example/catalog.json".into(), ..Default::default() };
        assert_eq!(s.display_name(), "x.example");
        let c = crate::catalog::parse(br#"{"name":"  X Games ","accent":"red","games":[]}"#).unwrap();
        assert!(s.absorb(&c));
        assert!(!s.absorb(&c));
        assert_eq!((s.name.as_deref(), s.accent.as_deref()), (Some("X Games"), None));
    }

    #[test]
    fn tells_exports_from_catalogs() {
        let mut l = SourceList::default();
        l.add("https://a.example/c.json", None).unwrap();
        let bytes = serde_json::to_vec(&l.export()).unwrap();
        assert!(matches!(parse_any(&bytes).unwrap(), Parsed::Export(e) if e.sources.len() == 1));
        assert!(matches!(parse_any(br#"{"games":[]}"#).unwrap(), Parsed::Catalog(_)));
        assert!(parse_any(b"<html>").is_err());
    }
}
