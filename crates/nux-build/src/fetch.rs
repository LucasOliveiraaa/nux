//! Downloading sources into `packages/<name>/<version>/source`.
//!
//! Downloads go to `source.tmp` and are renamed into place only once they
//! completed, so an interrupted download is never mistaken for a valid one.
//! Git repositories are fetched into their mirror in the
//! [download cache](crate::cache) and checked out from there; downloaded
//! files are kept there too.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use nux_eval::SourceKind;

use crate::cache::{self, CacheLock};
use crate::state::{Metadata, SourceIdentity, SourceRecord};
use crate::{BuildError, Progress, Step, Workspace};

/// Where a fetched source came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Network,
    /// The [download cache](crate::cache), without network access.
    Cache,
}

pub(crate) fn fetch(
    workspace: &Workspace,
    step: &Step,
    on_progress: &dyn Fn(Progress),
) -> Result<Origin, BuildError> {
    let tmp = step.paths.root.join("source.tmp");
    let result = fetch_into(workspace, step, &tmp, on_progress);
    if result.is_err() {
        let _ = remove(&tmp);
    }
    result
}

fn fetch_into(
    workspace: &Workspace,
    step: &Step,
    tmp: &Path,
    on_progress: &dyn Fn(Progress),
) -> Result<Origin, BuildError> {
    let package = &step.package;
    let source = package
        .source
        .as_ref()
        .expect("only steps with a remote source are fetched");
    let paths = &step.paths;
    let fail = |message: String| BuildError::Download {
        package: package.name.clone(),
        version: package.version.clone(),
        message,
    };

    remove(tmp).map_err(fail)?;
    std::fs::create_dir_all(&paths.root)
        .map_err(|e| fail(format!("cannot create `{}`: {e}", paths.root.display())))?;

    let pin = step.pin.clone().unwrap_or_default();
    let (file, identity, origin) = match &source.kind {
        SourceKind::Git(url) => {
            if url.starts_with('-') {
                return Err(fail(format!("`{url}` is not a repository URL")));
            }
            let mirror = workspace.git_mirror(url);
            let _lock = CacheLock::take(&mirror).map_err(fail)?;
            cache::init_mirror(&mirror).map_err(fail)?;
            // Fetches `refspecs` from `url` into the mirror, with progress.
            let network = |options: &[&str], refspecs: &[&str]| {
                let mut fetch = Command::new("git");
                if let Some(proxy) = &source.proxy {
                    fetch.arg("-c").arg(format!("http.proxy={proxy}"));
                }
                fetch
                    .arg("-C")
                    .arg(&mirror)
                    .args(["fetch", "--progress", "--no-tags"])
                    .args(options)
                    .arg(url)
                    .args(refspecs);
                run(fetch, &mut |line| {
                    on_progress(parse_git_progress(line)?);
                    Some(())
                })
            };

            let (commit, origin) = match &pin.revision {
                Some(revision) if cache::has_commit(&mirror, revision) => {
                    (revision.clone(), Origin::Cache)
                }
                Some(revision) => {
                    // Just that commit; servers that only serve advertised
                    // refs get asked for everything instead.
                    let refspec = format!("+{revision}:refs/nux/{revision}");
                    let shallow = network(&["--depth", "1"], &[&refspec]);
                    if shallow.is_err() || !cache::has_commit(&mirror, revision) {
                        let unshallow: &[&str] = if mirror.join("shallow").exists() {
                            &["--unshallow"]
                        } else {
                            &[]
                        };
                        network(
                            unshallow,
                            &[
                                "+refs/heads/*:refs/nux/heads/*",
                                "+refs/tags/*:refs/nux/tags/*",
                            ],
                        )
                        .map_err(fail)?;
                    }
                    if !cache::has_commit(&mirror, revision) {
                        return Err(fail(format!(
                            "the locked revision {revision} is not in `{url}`"
                        )));
                    }
                    (revision.clone(), Origin::Network)
                }
                None => {
                    let reference = source.tag.as_deref().unwrap_or("HEAD");
                    network(
                        &["--depth", "1"],
                        &[&format!("+{reference}:refs/nux/fetched")],
                    )
                    .map_err(fail)?;
                    (
                        cache::resolve(&mirror, "refs/nux/fetched").map_err(fail)?,
                        Origin::Network,
                    )
                }
            };
            cache::keep(&mirror, &commit).map_err(fail)?;
            cache::check_out(&mirror, &commit, tmp).map_err(fail)?;
            (
                None,
                SourceIdentity {
                    revision: Some(commit),
                    sha256: None,
                },
                origin,
            )
        }
        SourceKind::Url(url) => {
            let name = file_name(url);
            std::fs::create_dir_all(tmp).map_err(|e| fail(e.to_string()))?;
            let output = tmp.join(&name);
            match pin
                .sha256
                .as_ref()
                .filter(|sha256| cache::take_download(workspace, sha256, &output))
            {
                Some(sha256) => (
                    Some(name),
                    SourceIdentity {
                        revision: None,
                        sha256: Some(sha256.clone()),
                    },
                    Origin::Cache,
                ),
                None => {
                    let sha256 = download(url, source.proxy.as_deref(), &output, on_progress)
                        .map_err(fail)?;
                    if let Some(expected) =
                        pin.sha256.as_ref().filter(|expected| **expected != sha256)
                    {
                        return Err(fail(format!(
                            "checksum mismatch for `{url}`: Nux.lock expects sha256 {expected}, the download has {sha256}"
                        )));
                    }
                    cache::store_download(workspace, &output, &sha256);
                    (
                        Some(name),
                        SourceIdentity {
                            revision: None,
                            sha256: Some(sha256),
                        },
                        Origin::Network,
                    )
                }
            }
        }
        SourceKind::Local(_) => unreachable!("local sources are not downloaded"),
    };

    remove(&paths.source).map_err(fail)?;
    std::fs::rename(tmp, &paths.source)
        .map_err(|e| fail(format!("cannot move download into place: {e}")))?;

    // Keep the installation record: it is validated separately.
    let mut metadata = Metadata::read(&paths.metadata);
    metadata.source = Some(SourceRecord {
        fingerprint: step.source_fingerprint.clone().unwrap(),
        file,
        identity,
    });
    metadata.write(&paths.metadata, &package.name, &package.version)?;
    Ok(origin)
}

/// Downloads `url` to `output` with curl, with progress; its SHA-256.
fn download(
    url: &str,
    proxy: Option<&str>,
    output: &Path,
    on_progress: &dyn Fn(Progress),
) -> Result<String, String> {
    let mut curl = Command::new("curl");
    curl.args(["--fail", "--location", "--progress-bar", "--output"])
        .arg(output);
    if let Some(proxy) = proxy {
        curl.arg("--proxy").arg(proxy);
    }
    curl.arg("--").arg(url);
    let started = Instant::now();
    run(curl, &mut |line| {
        let fraction = parse_curl_progress(line)?;
        let elapsed = started.elapsed().as_secs_f64();
        let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
        on_progress(Progress {
            fraction: Some(fraction),
            bytes_per_second: (elapsed > 0.0).then(|| size as f64 / elapsed),
        });
        Some(())
    })?;
    sha256_file(output).map_err(|e| format!("cannot read the download: {e}"))
}

/// The last path segment of `url`, without query or fragment.
fn file_name(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    // Skip `scheme://host` so a bare host is never taken for a file name.
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map_or("", |(_, path)| path),
        None => url,
    };
    path.rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("source")
        .to_string()
}

fn remove(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(_) => Ok(()),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
    }
    .map_err(|e| format!("cannot remove `{}`: {e}", path.display()))
}

/// Runs `command`, feeding each `\r`- or `\n`-separated piece of its stderr
/// to `on_line`. On failure, returns the last stderr lines.
fn run(mut command: Command, on_line: &mut dyn FnMut(&str) -> Option<()>) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run `{program}`: {e}"))?;

    let mut stderr = child.stderr.take().unwrap();
    let mut pending = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &byte in &buffer[..read] {
            if byte == b'\r' || byte == b'\n' {
                let line = String::from_utf8_lossy(&pending).trim().to_string();
                pending.clear();
                if !line.is_empty() {
                    on_line(&line);
                    if byte == b'\n' {
                        lines.push(line);
                    }
                }
            } else {
                pending.push(byte);
            }
        }
    }
    if !pending.is_empty() {
        lines.push(String::from_utf8_lossy(&pending).trim().to_string());
    }

    let status = child
        .wait()
        .map_err(|e| format!("`{program}` failed: {e}"))?;
    if status.success() {
        return Ok(());
    }
    let tail = lines.len().saturating_sub(5);
    let details = lines[tail..].join("\n");
    Err(if details.is_empty() {
        format!("`{program}` exited with {status}")
    } else {
        format!("`{program}` exited with {status}:\n{details}")
    })
}

/// The SHA-256 of a file, hex encoded.
pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `Receiving objects:  45% (450/1000), 1.20 MiB | 2.40 MiB/s`
fn parse_git_progress(line: &str) -> Option<Progress> {
    let rest = line.strip_prefix("Receiving objects:")?;
    let percent: f64 = rest.split('%').next()?.trim().parse().ok()?;
    let bytes_per_second = rest
        .rsplit('|')
        .next()
        .filter(|_| rest.contains('|'))
        .and_then(|rate| {
            let mut parts = rate.split_whitespace();
            let value: f64 = parts.next()?.parse().ok()?;
            let unit = match parts.next()?.trim_end_matches("/s") {
                "bytes" | "B" => 1.0,
                "KiB" => 1024.0,
                "MiB" => 1024.0 * 1024.0,
                "GiB" => 1024.0 * 1024.0 * 1024.0,
                _ => return None,
            };
            Some(value * unit)
        });
    Some(Progress {
        fraction: Some(percent / 100.0),
        bytes_per_second,
    })
}

/// `######                    12.3%`
fn parse_curl_progress(line: &str) -> Option<f64> {
    let percent: f64 = line
        .split_whitespace()
        .last()?
        .strip_suffix('%')?
        .parse()
        .ok()?;
    Some(percent / 100.0)
}

pub(crate) fn local_source(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    if path.is_dir() {
        Ok((path.to_path_buf(), path.to_path_buf()))
    } else if path.is_file() {
        Ok((
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            path.to_path_buf(),
        ))
    } else {
        Err(format!("local source `{}` does not exist", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress() {
        let p = parse_git_progress("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s")
            .unwrap();
        assert_eq!(p.fraction, Some(0.45));
        assert_eq!(p.bytes_per_second, Some(2.0 * 1024.0 * 1024.0));
        let p = parse_git_progress("Receiving objects: 100% (3/3), done.").unwrap();
        assert_eq!((p.fraction, p.bytes_per_second), (Some(1.0), None));
        assert!(parse_git_progress("Resolving deltas: 10%").is_none());

        assert_eq!(parse_curl_progress("########     12.5%"), Some(0.125));
        assert_eq!(parse_curl_progress("curl: (22) error"), None);
    }

    #[test]
    fn file_names_from_urls() {
        assert_eq!(
            file_name("https://x.org/a/steam_latest.tar.gz"),
            "steam_latest.tar.gz"
        );
        assert_eq!(
            file_name("https://x.org/dl/app.AppImage?token=1#x"),
            "app.AppImage"
        );
        assert_eq!(file_name("https://x.org/"), "source");
    }
}
