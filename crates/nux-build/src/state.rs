//! `package.json` records and the fingerprints that validate them.
//!
//! A record is only trusted when its fingerprint matches the definition
//! being built, so editing a definition (commands, source, `set_bin`, or any
//! dependency's definition) invalidates the installation without a version
//! bump.

use std::path::Path;

use nux_eval::{Source, SourceKind};
use serde_json::{Value, json};

use crate::BuildError;

/// A stable 64-bit FNV-1a hash, hex encoded. `std`'s hashers are not
/// guaranteed to be stable across Rust releases, which records must survive.
pub(crate) struct Fingerprint(u64);

impl Fingerprint {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn add(&mut self, field: &str) -> &mut Self {
        // A separator keeps ("ab", "c") and ("a", "bc") apart.
        for byte in field.bytes().chain([0xff]) {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        self
    }

    pub fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Identifies what a source download contains (the proxy does not matter).
pub(crate) fn source_fingerprint(source: &Source) -> String {
    let (kind, location) = match &source.kind {
        SourceKind::Git(url) => ("git", url),
        SourceKind::Url(url) => ("url", url),
        SourceKind::Local(path) => ("local", path),
    };
    Fingerprint::new()
        .add(kind)
        .add(location)
        .add(source.tag.as_deref().unwrap_or(""))
        .hex()
}

/// What exactly a download contained: the commit of a git clone, the
/// SHA-256 of a downloaded file. Recorded in `Nux.lock`, and given back to
/// [`crate::Builder::with_pins`] to reproduce the same sources.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SourceIdentity {
    pub revision: Option<String>,
    pub sha256: Option<String>,
}

impl SourceIdentity {
    pub fn is_empty(&self) -> bool {
        self.revision.is_none() && self.sha256.is_none()
    }

    /// Whether `self`, as recorded, is compatible with `pin`: every part the
    /// pin sets must be equal. Unknown parts (older records) are accepted.
    pub(crate) fn satisfies(&self, pin: &SourceIdentity) -> bool {
        let part = |have: &Option<String>, want: &Option<String>| match (have, want) {
            (Some(have), Some(want)) => have == want,
            _ => true,
        };
        part(&self.revision, &pin.revision) && part(&self.sha256, &pin.sha256)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SourceRecord {
    pub fingerprint: String,
    /// The downloaded file's name, for `url` sources.
    pub file: Option<String>,
    pub identity: SourceIdentity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InstallRecord {
    pub fingerprint: String,
    pub bin: Option<String>,
    /// The source the installation was built from.
    pub identity: SourceIdentity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub source: Option<SourceRecord>,
    pub installed: Option<InstallRecord>,
}

impl Metadata {
    /// Reads `path`; a missing or unreadable record counts as empty, so the
    /// package is simply downloaded and built again.
    pub fn read(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return Self::default();
        };
        let string = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
        let identity = |v: &Value| SourceIdentity { revision: string(v, "revision"), sha256: string(v, "sha256") };
        Self {
            source: value.get("source").filter(|s| s.is_object()).and_then(|s| {
                Some(SourceRecord {
                    fingerprint: string(s, "fingerprint")?,
                    file: string(s, "file"),
                    identity: identity(s),
                })
            }),
            installed: value.get("installed").filter(|s| s.is_object()).and_then(|s| {
                Some(InstallRecord {
                    fingerprint: string(s, "fingerprint")?,
                    bin: string(s, "bin"),
                    identity: identity(s),
                })
            }),
        }
    }

    /// Writes atomically: a crash leaves either the old or the new record.
    pub fn write(&self, path: &Path, name: &str, version: &str) -> Result<(), BuildError> {
        let value = json!({
            "name": name,
            "version": version,
            "source": self.source.as_ref().map(|s| json!({
                "fingerprint": s.fingerprint,
                "file": s.file,
                "revision": s.identity.revision,
                "sha256": s.identity.sha256,
            })),
            "installed": self.installed.as_ref().map(|i| json!({
                "fingerprint": i.fingerprint,
                "bin": i.bin,
                "revision": i.identity.revision,
                "sha256": i.identity.sha256,
            })),
        });
        let text = serde_json::to_string_pretty(&value).expect("json values always serialize") + "\n";
        write_atomic(path, text.as_bytes())
    }
}

/// Replaces `path` with `contents` atomically: a crash leaves either the old
/// file or the new one, never a mix.
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), BuildError> {
    let context = || format!("cannot write `{}`", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| BuildError::io(context(), e))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).map_err(|e| BuildError::io(context(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| BuildError::io(context(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable_and_field_separated() {
        assert_eq!(Fingerprint::new().hex(), "cbf29ce484222325");
        let ab_c = Fingerprint::new().add("ab").add("c").hex();
        let a_bc = Fingerprint::new().add("a").add("bc").hex();
        assert_ne!(ab_c, a_bc);
        assert_eq!(ab_c, Fingerprint::new().add("ab").add("c").hex());
    }

    #[test]
    fn source_fingerprint_ignores_proxy_but_not_tag() {
        let source = |tag: Option<&str>, proxy: Option<&str>| Source {
            kind: SourceKind::Git("https://example.com/x.git".into()),
            tag: tag.map(Into::into),
            proxy: proxy.map(Into::into),
        };
        assert_eq!(source_fingerprint(&source(None, None)), source_fingerprint(&source(None, Some("p"))));
        assert_ne!(source_fingerprint(&source(None, None)), source_fingerprint(&source(Some("v1"), None)));
    }

    #[test]
    fn metadata_round_trips_and_tolerates_garbage() {
        let dir = std::env::temp_dir().join(format!("nux-build-state-{}", std::process::id()));
        let path = dir.join("package.json");
        let metadata = Metadata {
            source: Some(SourceRecord {
                fingerprint: "a".into(),
                file: Some("x.tar.gz".into()),
                identity: SourceIdentity { revision: None, sha256: Some("abc".into()) },
            }),
            installed: Some(InstallRecord {
                fingerprint: "b".into(),
                bin: None,
                identity: SourceIdentity { revision: Some("deadbeef".into()), sha256: None },
            }),
        };
        metadata.write(&path, "pkg", "1.0").unwrap();
        assert_eq!(Metadata::read(&path), metadata);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Metadata::read(&path), Metadata::default());
        assert_eq!(Metadata::read(&dir.join("missing.json")), Metadata::default());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn identities_satisfy_matching_pins() {
        let id = |r: Option<&str>, s: Option<&str>| SourceIdentity { revision: r.map(Into::into), sha256: s.map(Into::into) };
        assert!(id(Some("a"), None).satisfies(&id(Some("a"), None)));
        assert!(!id(Some("a"), None).satisfies(&id(Some("b"), None)));
        // Unknown on either side is accepted.
        assert!(id(None, None).satisfies(&id(Some("b"), Some("c"))));
        assert!(id(Some("a"), Some("x")).satisfies(&SourceIdentity::default()));
        assert!(!id(None, Some("x")).satisfies(&id(None, Some("y"))));
    }
}
