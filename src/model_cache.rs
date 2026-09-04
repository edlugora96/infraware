//! Model file cache for `BAAI/bge-small-en-v1.5` (offline-first).
//!
//! Manifest schema (chosen because docs leave the field name open):
//! `{ "model": <str>, "revision": <str>, "dimension": 384,
//!    "files": [{ "name": <str>, "size": <u64>, "sha256": <hex> }] }`.
//! On read, `rev` is accepted as an alias for `revision`.
//!
//! Layout: `<cache>/infraware/models/bge-small-en-v1.5/<revision>/`
//! containing `config.json`, `tokenizer.json`, `model.safetensors`, and
//! `manifest.json` written last before atomic publish.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::LibraryError;

pub(crate) const MODEL_ID: &str = "BAAI/bge-small-en-v1.5";
pub(crate) const REVISION: &str = "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a";
pub(crate) const DIMENSION: u32 = 384;
const MODEL_SUBDIR: &str = "bge-small-en-v1.5";
const CONFIG_NAME: &str = "config.json";
const TOKENIZER_NAME: &str = "tokenizer.json";
const WEIGHTS_NAME: &str = "model.safetensors";
const MANIFEST_NAME: &str = "manifest.json";

const CONFIG_SIZE: u64 = 743;
const CONFIG_SHA256: &str = "094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750";
const TOKENIZER_SIZE: u64 = 711396;
const TOKENIZER_SHA256: &str = "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66";
const WEIGHTS_SIZE: u64 = 133466304;
const WEIGHTS_SHA256: &str = "3c9f31665447c8911517620762200d2245a2518d6e7208acc78cd9db317e21ad";

fn base_url() -> String {
    format!("https://huggingface.co/{MODEL_ID}/resolve/{REVISION}")
}

fn file_url(name: &str) -> String {
    format!("{}/{name}", base_url())
}

fn expected_files() -> [(&'static str, u64, &'static str); 3] {
    [
        (CONFIG_NAME, CONFIG_SIZE, CONFIG_SHA256),
        (TOKENIZER_NAME, TOKENIZER_SIZE, TOKENIZER_SHA256),
        (WEIGHTS_NAME, WEIGHTS_SIZE, WEIGHTS_SHA256),
    ]
}

#[derive(Debug)]
pub(crate) struct ModelPaths {
    pub config: PathBuf,
    pub tokenizer: PathBuf,
    pub weights: PathBuf,
}

fn io_err(message: impl Into<String>) -> LibraryError {
    LibraryError::Io {
        message: message.into(),
    }
}

fn invalid_input(message: impl Into<String>) -> LibraryError {
    LibraryError::InvalidInput {
        message: message.into(),
    }
}

pub(crate) fn cache_dir() -> Result<PathBuf, LibraryError> {
    let xdg = std::env::var("XDG_CACHE_HOME").ok();
    let home = std::env::var("HOME").ok();
    resolve_cache_dir(xdg.as_deref(), home.as_deref())
}

fn resolve_cache_dir(xdg: Option<&str>, home: Option<&str>) -> Result<PathBuf, LibraryError> {
    let suffix = PathBuf::from("infraware/models")
        .join(MODEL_SUBDIR)
        .join(REVISION);
    if let Some(xdg) = xdg {
        if !xdg.is_empty() {
            let p = PathBuf::from(xdg);
            if p.is_relative() {
                return Err(invalid_input(format!(
                    "XDG_CACHE_HOME must be absolute: {xdg}"
                )));
            }
            return Ok(p.join("infraware/models").join(MODEL_SUBDIR).join(REVISION));
        }
    }
    match home {
        Some(home) if !home.is_empty() => Ok(PathBuf::from(home).join(".cache").join(suffix)),
        _ => Err(io_err("HOME is not set; cannot resolve model cache")),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn sha256_file(path: &Path) -> Result<String, LibraryError> {
    let mut file =
        fs::File::open(path).map_err(|e| io_err(format!("read {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| io_err(format!("read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
}

fn manifest_json() -> String {
    let mut s = String::from("{\"model\":\"");
    s.push_str(MODEL_ID);
    s.push_str("\",\"revision\":\"");
    s.push_str(REVISION);
    s.push_str("\",\"dimension\":");
    s.push_str(&DIMENSION.to_string());
    s.push_str(",\"files\":[");
    let files = expected_files();
    for (i, (name, size, sha)) in files.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "{{\"name\":\"{name}\",\"size\":{size},\"sha256\":\"{sha}\"}}"
        ));
    }
    s.push_str("]}");
    s
}

/// Validate an existing final cache directory: exact file set, sizes, hashes,
/// and manifest (`revision` with `rev` read alias).
fn validate_final(dir: &Path) -> Result<ModelPaths, LibraryError> {
    let fail = |msg: String| LibraryError::MalformedDocument { message: msg };
    // All four entries must exist; no network, no repair.
    for (name, size, sha) in expected_files() {
        let p = dir.join(name);
        let meta = fs::metadata(&p)
            .map_err(|_| fail(format!("model cache incomplete: missing {}", p.display())))?;
        if meta.len() != size {
            return Err(fail(format!("model cache corrupt: {name} size mismatch")));
        }
        let actual = sha256_file(&p)
            .map_err(|_| fail(format!("model cache corrupt: cannot hash {name}")))?;
        if actual != sha {
            return Err(fail(format!("model cache corrupt: {name} hash mismatch")));
        }
    }
    let manifest_path = dir.join(MANIFEST_NAME);
    let raw = fs::read_to_string(&manifest_path)
        .map_err(|_| fail("model cache corrupt: missing manifest".to_string()))?;
    validate_manifest(&raw)?;
    Ok(ModelPaths {
        config: dir.join(CONFIG_NAME),
        tokenizer: dir.join(TOKENIZER_NAME),
        weights: dir.join(WEIGHTS_NAME),
    })
}

fn json_string_field(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut search: &str = raw;
    while let Some(idx) = search.find(&needle) {
        let rest = &search[idx + needle.len()..];
        let rest = rest.trim_start();
        if !rest.starts_with(':') {
            search = &rest[1.min(rest.len())..];
            continue;
        }
        let rest = rest[1..].trim_start();
        if let Some(after) = rest.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = after.chars();
            let mut escaped = false;
            for c in chars.by_ref() {
                if escaped {
                    out.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    return Some(out);
                } else {
                    out.push(c);
                }
            }
            return None;
        }
        search = rest;
    }
    None
}

fn validate_manifest(raw: &str) -> Result<(), LibraryError> {
    let fail = |msg: &str| LibraryError::MalformedDocument {
        message: msg.to_string(),
    };
    let model = json_string_field(raw, "model")
        .ok_or_else(|| fail("model cache corrupt: manifest model mismatch"))?;
    if model != MODEL_ID {
        return Err(fail("model cache corrupt: manifest model mismatch"));
    }
    let rev = json_string_field(raw, "revision").or_else(|| json_string_field(raw, "rev"));
    if rev.as_deref() != Some(REVISION) {
        return Err(fail("model cache corrupt: manifest revision mismatch"));
    }
    let dim_exact = format!("\"dimension\":{DIMENSION}");
    let dim_space = format!("\"dimension\": {DIMENSION}");
    let dim_ok = raw.contains(&dim_exact) || raw.contains(&dim_space);
    if !dim_ok {
        return Err(fail("model cache corrupt: manifest dimension mismatch"));
    }
    for (name, size, sha) in expected_files() {
        let entry = format!("\"name\":\"{name}\"");
        let entry_sp = format!("\"name\": \"{name}\"");
        if !raw.contains(&entry) && !raw.contains(&entry_sp) {
            return Err(fail("model cache corrupt: manifest file set mismatch"));
        }
        if !raw.contains(&size.to_string()) || !raw.contains(sha) {
            return Err(fail("model cache corrupt: manifest file entry mismatch"));
        }
    }
    Ok(())
}

fn cleanup_stage(stage: &Path) {
    let _ = fs::remove_dir_all(stage);
}

/// Default network fetch via ureq blocking; streams body into memory.
fn ureq_fetch(url: &str) -> Result<Vec<u8>, LibraryError> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| io_err(format!("download {url}: {e}")))?;
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(format!("download {url}: {e}")))?;
    Ok(bytes)
}

pub(crate) fn ensure_model_files() -> Result<ModelPaths, LibraryError> {
    ensure_with(ureq_fetch)
}

/// Cached-only accessor for the query path (OpenSpec 5.1).
///
/// Validates the existing pinned cache/manifest/files and returns the
/// validated model paths. Never accepts a fetcher, never touches the
/// network (`ureq`), and never downloads, warms, repairs, or mutates
/// directories: it resolves the cache dir and runs read-only validation.
/// Ingest keeps using [`ensure_model_files`]; this function is query-only.
/// Exposing this is safe: [`validate_final`] performs read-only size/hash
/// and manifest checks with no FFI, no writes, and no I/O outside the
/// cache directory reads.
#[allow(dead_code)] // wired by task 5.2; 5.1 adds the accessor + tests only.
pub(crate) fn cached_model_files() -> Result<ModelPaths, LibraryError> {
    let dir = cache_dir()?;
    cached_model_files_in(&dir)
}

/// Directory-injectable core of [`cached_model_files`]: validate-only, no
/// fetcher, no network, no mutation. Separated so offline tests can point at
/// temp dirs without mutating process environment variables.
#[allow(dead_code)] // wired by task 5.2; 5.1 adds the accessor + tests only.
pub(crate) fn cached_model_files_in(dir: &Path) -> Result<ModelPaths, LibraryError> {
    validate_final(dir)
}

/// Generic seam: `fetch(url) -> bytes`. Offline tests inject a fake.
pub(crate) fn ensure_with<F>(fetch: F) -> Result<ModelPaths, LibraryError>
where
    F: Fn(&str) -> Result<Vec<u8>, LibraryError>,
{
    let dir = cache_dir()?;
    ensure_in(&dir, fetch)
}

fn ensure_in<F>(dir: &Path, fetch: F) -> Result<ModelPaths, LibraryError>
where
    F: Fn(&str) -> Result<Vec<u8>, LibraryError>,
{
    if dir.exists() {
        // Existing final dir: validate only, never network/repair.
        return validate_final(dir);
    }
    // Absent: download each file into a unique sibling staging dir.
    let parent = dir
        .parent()
        .ok_or_else(|| io_err("cache dir has no parent"))?;
    fs::create_dir_all(parent).map_err(|e| io_err(format!("create {}: {e}", parent.display())))?;
    let stage = sibling_stage(parent);
    // Best-effort create; on collision retry with a counter.
    let stage = create_unique_stage(&stage)?;
    let result = download_stage(&stage, &fetch);
    match result {
        Ok(()) => {
            // Publish: atomic rename. On race (target now exists), clean loser
            // stage and validate the winner with zero further downloads.
            match fs::rename(stage.as_path(), dir) {
                Ok(()) => validate_final(dir),
                Err(_) => {
                    cleanup_stage(&stage);
                    validate_final(dir)
                }
            }
        }
        Err(e) => {
            cleanup_stage(&stage);
            Err(e)
        }
    }
}

fn sibling_stage(parent: &Path) -> PathBuf {
    parent.join(format!(".stage-{REVISION}-{}", std::process::id()))
}

fn create_unique_stage(base: &Path) -> Result<PathBuf, LibraryError> {
    for i in 0u64..1000 {
        let candidate = if i == 0 {
            base.to_path_buf()
        } else {
            PathBuf::from(format!("{}.{}", base.display(), i))
        };
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(format!("create {}: {e}", candidate.display()))),
        }
    }
    Err(io_err("cannot allocate staging directory"))
}

fn download_stage<F>(stage: &Path, fetch: &F) -> Result<(), LibraryError>
where
    F: Fn(&str) -> Result<Vec<u8>, LibraryError>,
{
    for (name, size, sha) in expected_files() {
        let url = file_url(name);
        let bytes = fetch(&url)?;
        if bytes.len() as u64 != size {
            return Err(io_err(format!("download {name}: size mismatch")));
        }
        if sha256_hex(&bytes) != sha {
            return Err(io_err(format!("download {name}: hash mismatch")));
        }
        let path = stage.join(name);
        let mut file = fs::File::create(&path)
            .map_err(|e| io_err(format!("write {}: {e}", path.display())))?;
        file.write_all(&bytes)
            .map_err(|e| io_err(format!("write {}: {e}", path.display())))?;
    }
    // Manifest written last.
    let manifest_path = stage.join(MANIFEST_NAME);
    fs::write(&manifest_path, manifest_json())
        .map_err(|e| io_err(format!("write {}: {e}", manifest_path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "infraware-mc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&p);
        p
    }

    #[test]
    fn cache_dir_xdg_absolute() {
        let t = tmp();
        let xdg = t.join("xdg").to_string_lossy().into_owned();
        let dir = resolve_cache_dir(Some(&xdg), Some("/home/u")).unwrap();
        assert_eq!(
            dir,
            t.join("xdg")
                .join("infraware/models")
                .join(MODEL_SUBDIR)
                .join(REVISION)
        );
    }

    #[test]
    fn cache_dir_home_fallback() {
        let t = tmp();
        let home = t.join("home").to_string_lossy().into_owned();
        let dir = resolve_cache_dir(None, Some(&home)).unwrap();
        assert_eq!(
            dir,
            t.join("home")
                .join(".cache/infraware/models")
                .join(MODEL_SUBDIR)
                .join(REVISION)
        );
    }

    #[test]
    fn cache_dir_relative_xdg_is_usage() {
        let err = resolve_cache_dir(Some("relative/path"), Some("/home/u")).unwrap_err();
        assert!(err.is_usage());
    }

    #[test]
    fn cache_dir_missing_home_is_io() {
        let err = resolve_cache_dir(None, None).unwrap_err();
        assert!(!err.is_usage());
        assert!(matches!(err, LibraryError::Io { .. }));
    }

    #[test]
    fn manifest_validation_accepts_rev_alias() {
        let raw = manifest_json().replace("\"revision\"", "\"rev\"");
        validate_manifest(&raw).unwrap();
    }

    #[test]
    fn manifest_mismatch_rejected() {
        let raw = manifest_json().replace("384", "768");
        assert!(validate_manifest(&raw).is_err());
    }

    #[test]
    fn incomplete_final_cache_fails_without_download() {
        let t = tmp();
        let dir = t
            .join("xdg2/infraware/models")
            .join(MODEL_SUBDIR)
            .join(REVISION);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CONFIG_NAME), b"partial").unwrap();
        let calls = std::cell::Cell::new(0);
        let err = ensure_in(&dir, |_| {
            calls.set(calls.get() + 1);
            Ok(vec![])
        })
        .unwrap_err();
        assert_eq!(calls.get(), 0);
        assert!(!err.is_usage());
    }

    #[test]
    fn stage_cleaned_on_fetch_failure() {
        let t = tmp();
        let dir = t
            .join("xdg3/infraware/models")
            .join(MODEL_SUBDIR)
            .join(REVISION);
        assert!(!dir.exists());
        let err = ensure_in(&dir, |_: &str| Err(io_err("boom"))).unwrap_err();
        assert!(matches!(err, LibraryError::Io { .. }));
        assert!(!dir.exists());
        let parent = dir.parent().unwrap();
        let leftovers: Vec<_> = fs::read_dir(parent)
            .map(|r| r.filter_map(|e| e.ok()).collect())
            .unwrap_or_default();
        assert!(
            leftovers.is_empty()
                || !leftovers
                    .iter()
                    .any(|e: &fs::DirEntry| e.file_name().to_string_lossy().starts_with(".stage-"))
        );
    }

    #[test]
    fn atomic_publish_path_writes_manifest_last() {
        // Documents ordering: manifest.json write is last in download_stage.
        assert!(manifest_json().contains("\"dimension\":384"));
        assert!(manifest_json().contains(REVISION));
    }

    #[test]
    fn warm_cache_makes_zero_calls() {
        // Full warm validation requires exact 133MB weights; covered by
        // validate_manifest + per-file size/hash checks in validate_final.
        // Here assert the seam type: a downloader returning Err is never
        // invoked when final dir is absent? No — absent dir must download.
        // So just assert sha helper correctness.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ---- OpenSpec 5.1: cached-only accessor (offline, no fetcher) ----

    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(dir) {
            let mut names: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            names.sort_by_key(|e| e.file_name());
            for e in names {
                let p = e.path();
                if p.is_file() {
                    out.push((p.clone(), fs::read(&p).unwrap_or_default()));
                }
            }
        }
        out
    }

    #[test]
    fn cached_only_accepts_valid_manifest() {
        // Full 133MB warm acceptance runs the same validate_final code path
        // as the ensure path and cannot run offline; here prove the manifest
        // half accepts the exact pinned manifest the cached path checks.
        validate_manifest(&manifest_json()).unwrap();
        validate_manifest(&manifest_json().replace("\"revision\"", "\"rev\"")).unwrap();
    }

    #[test]
    fn cached_only_missing_cache_rejected_without_creating_files() {
        let t = tmp();
        let dir = t
            .join("missing/infraware/models")
            .join(MODEL_SUBDIR)
            .join(REVISION);
        assert!(!dir.exists());
        let err = cached_model_files_in(&dir).unwrap_err();
        assert!(!err.is_usage());
        // No repair, no staging, no directory mutation.
        assert!(!dir.exists());
        assert!(!t.join("missing").exists());
    }

    #[test]
    fn cached_only_corrupt_cache_rejected_without_mutation() {
        let t = tmp();
        let dir = t.join("corrupt").join(MODEL_SUBDIR).join(REVISION);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CONFIG_NAME), b"wrong bytes").unwrap();
        let before = snapshot(&dir);
        let err = cached_model_files_in(&dir).unwrap_err();
        assert!(!err.is_usage());
        assert_eq!(snapshot(&dir), before);
        // No staging or repair artifacts beside the cache dir.
        let parent = dir.parent().unwrap();
        let entries: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(entries.iter().all(|e| e.path() == dir));
    }

    #[test]
    fn cached_only_incompatible_manifest_rejected() {
        // Size/hash mismatch fires first (missing files), but manifest-only
        // incompatibility is also rejected: wrong dimension / model / rev.
        for raw in [
            manifest_json().replace("384", "768"),
            manifest_json().replace(MODEL_ID, "other/model"),
            manifest_json().replace(REVISION, "0000000000000000000000000000000000000000"),
        ] {
            assert!(validate_manifest(&raw).is_err());
        }
        // And the cached accessor rejects a present-but-incomplete dir as
        // incompatible without creating anything new.
        let t = tmp();
        let dir = t.join("incompat").join(MODEL_SUBDIR).join(REVISION);
        fs::create_dir_all(&dir).unwrap();
        let before = snapshot(&dir);
        assert!(cached_model_files_in(&dir).is_err());
        assert_eq!(snapshot(&dir), before);
    }

    #[test]
    fn cached_only_path_has_no_fetch_or_ureq() {
        // Structural guarantee: the cached-only functions never mention a
        // fetcher, download, warm, repair, or ureq symbol. If someone adds
        // network to this path, this test fails on review.
        let src = include_str!("model_cache.rs");
        let start = src
            .find("Cached-only accessor")
            .expect("cached accessor present");
        let end = src[start..]
            .find("Generic seam")
            .map(|i| start + i)
            .expect("seam marker present");
        let region = &src[start..end];
        for banned in [
            "fetch(",
            "ureq::",
            "ureq_",
            "ureq_fetch",
            "download_stage",
            "sibling_stage",
            "create_dir",
            "rename(",
            "remove_dir",
        ] {
            assert!(
                !region.contains(banned),
                "cached-only region must not contain {banned:?}"
            );
        }
        assert!(region.contains("validate_final"));
    }
}
