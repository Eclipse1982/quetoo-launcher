//! Incremental game-data sync against quetoo-data's manifest.mf.

use crate::error::{LauncherError, Result};
use crate::installer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// Base URL for the `default` game data set on S3 (note trailing slash).
const DATA_BASE_URL: &str = "https://quetoo-data.s3.amazonaws.com/default/";
/// The manifest object within the data set.
const MANIFEST_URL: &str = "https://quetoo-data.s3.amazonaws.com/default/manifest.mf";
/// Max files hashed concurrently during the verify/seed pass.
const HASH_CONCURRENCY: usize = 8;

/// One line of `manifest.mf`: `md5 size path` (path is relative to the
/// `default/` game dir, e.g. `maps/2deaths.bsp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub md5: String,
    pub size: u64,
    pub path: String,
}

/// Parse manifest text into entries. Blank lines, malformed lines (missing
/// fields, non-hex md5, non-numeric size) and unsafe paths are skipped. The
/// path is the remainder after the first two spaces, so paths may contain spaces.
pub fn parse_manifest(text: &str) -> Vec<ManifestEntry> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.splitn(3, ' ');
        let (Some(md5), Some(size), Some(path)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if md5.len() != 32 || !md5.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(size) = size.parse::<u64>() else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() || crate::installer::is_unsafe_rel_path(path) {
            continue;
        }
        out.push(ManifestEntry {
            md5: md5.to_ascii_lowercase(),
            size,
            path: path.to_string(),
        });
    }
    out
}

/// Absolute local path for a manifest entry: `<install_dir>/share/default/<rel>`.
/// `rel` uses forward slashes (manifest format); split so separators are correct
/// on every platform.
pub fn local_path(install_dir: &Path, rel: &str) -> PathBuf {
    let mut p = install_dir.join("share").join("default");
    for comp in rel.split('/') {
        if !comp.is_empty() {
            p = p.join(comp);
        }
    }
    p
}

/// Public S3 URL for a manifest entry.
pub fn download_url(rel: &str) -> String {
    format!("{DATA_BASE_URL}{rel}")
}

/// One indexed file: the md5/size we installed and the on-disk mtime we last saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub md5: String,
    pub size: u64,
    pub mtime: i64,
}

/// The managed-files index: every file the launcher installed, keyed by the
/// manifest-relative path. Only files corresponding to a manifest entry are ever
/// recorded, so pruning never deletes user-added content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataIndex {
    pub version: u32,
    pub files: BTreeMap<String, IndexEntry>,
}

impl Default for DataIndex {
    fn default() -> Self {
        DataIndex {
            version: 1,
            files: BTreeMap::new(),
        }
    }
}

impl DataIndex {
    /// Load the index, returning a default (empty) index if the file is missing.
    pub fn load(path: &Path) -> Result<DataIndex> {
        if !path.exists() {
            return Ok(DataIndex::default());
        }
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(|e| LauncherError::Config(e.to_string()))
    }

    /// Save the index, creating parent dirs, writing a temp file then renaming.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|e| LauncherError::Config(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Minimal local-file stat used for classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStat {
    pub size: u64,
    pub mtime: i64,
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Stat a local file. Returns `None` if it does not exist or is not a file.
pub fn stat_local(path: &Path) -> Option<LocalStat> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(LocalStat {
        size: meta.len(),
        mtime: mtime_secs(&meta),
    })
}

/// Streaming MD5 of a file, lowercase hex.
pub fn md5_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut ctx = md5::Context::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.consume(&buf[..n]);
    }
    Ok(format!("{:x}", ctx.compute()))
}

/// True if the file exists with the given size and MD5.
pub fn verify_file(path: &Path, size: u64, md5: &str) -> Result<bool> {
    let Some(stat) = stat_local(path) else {
        return Ok(false);
    };
    if stat.size != size {
        return Ok(false);
    }
    Ok(md5_file(path)?.eq_ignore_ascii_case(md5))
}

/// Outcome of comparing one manifest entry to local state, without hashing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Local file already current; no work.
    UpToDate,
    /// Must (re)download.
    Download,
    /// Size matches but trust is unproven; caller must hash the file to decide.
    NeedsHash,
}

/// Decide what to do for one entry given its index record and local stat.
/// `verify = true` ignores the fast path (forces a hash when the size matches).
pub fn decide(
    entry: &ManifestEntry,
    indexed: Option<&IndexEntry>,
    local: Option<&LocalStat>,
    verify: bool,
) -> Decision {
    let Some(local) = local else {
        return Decision::Download;
    };
    if local.size != entry.size {
        return Decision::Download;
    }
    if !verify {
        if let Some(ix) = indexed {
            if ix.md5.eq_ignore_ascii_case(&entry.md5)
                && ix.size == local.size
                && ix.mtime == local.mtime
            {
                return Decision::UpToDate;
            }
        }
    }
    Decision::NeedsHash
}

/// Paths present in the index but absent from the manifest — i.e. files the
/// launcher previously installed that have since been removed upstream. Sorted.
pub fn prune_set(index: &DataIndex, manifest: &[ManifestEntry]) -> Vec<String> {
    use std::collections::HashSet;
    let present: HashSet<&str> = manifest.iter().map(|e| e.path.as_str()).collect();
    let mut out: Vec<String> = index
        .files
        .keys()
        .filter(|k| !present.contains(k.as_str()))
        .cloned()
        .collect();
    out.sort();
    out
}

/// Write `share/default/manifest.mf` in the engine's `hash size path` format,
/// listing every manifest file present locally at the correct hash. Quetoo's
/// built-in installer reads this on launch to decide what to download; keeping
/// it current stops the engine re-downloading data the launcher already synced.
fn write_engine_manifest(
    install_dir: &Path,
    manifest: &[ManifestEntry],
    index: &DataIndex,
) -> Result<()> {
    let mut lines: Vec<String> = manifest
        .iter()
        .filter(|e| {
            index
                .files
                .get(&e.path)
                .map(|ix| ix.md5.eq_ignore_ascii_case(&e.md5) && ix.size == e.size)
                .unwrap_or(false)
        })
        .map(|e| format!("{} {} {}", e.md5, e.size, e.path))
        .collect();
    lines.sort();
    let mut body = lines.join("\n");
    body.push('\n');

    let path = local_path(install_dir, "manifest.mf");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("mf.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Result of a sync run, reported to the UI.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub checked: usize,
    pub downloaded: usize,
    pub deleted: usize,
    pub bytes_downloaded: u64,
    pub warnings: usize,
    /// True when sync was skipped (offline / not installed).
    pub skipped: bool,
}

/// Fetch and parse `manifest.mf`.
pub async fn fetch_manifest(client: &reqwest::Client) -> Result<Vec<ManifestEntry>> {
    let resp = client
        .get(MANIFEST_URL)
        .header("User-Agent", "quetoo-launcher")
        .send()
        .await
        .map_err(|e| LauncherError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(LauncherError::Network(format!("HTTP {}", resp.status())));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| LauncherError::Network(e.to_string()))?;
    Ok(parse_manifest(&text))
}

/// Download one file to `dest`: stream to a sibling temp file, verify size+md5,
/// then atomically rename. Removes the temp on any failure.
pub async fn download_one(
    client: &reqwest::Client,
    rel: &str,
    size: u64,
    md5: &str,
    dest: &Path,
) -> Result<()> {
    use futures_util::StreamExt;
    use std::io::Write;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let resp = client
        .get(download_url(rel))
        .header("User-Agent", "quetoo-launcher")
        .send()
        .await
        .map_err(|e| LauncherError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(LauncherError::Network(format!(
            "HTTP {} for {rel}",
            resp.status()
        )));
    }

    let file_name = dest.file_name().and_then(|s| s.to_str()).unwrap_or("data");
    let tmp = dest.with_file_name(format!(".{file_name}.part"));

    let mut ctx = md5::Context::new();
    let mut got: u64 = 0;
    {
        let mut file = std::fs::File::create(&tmp)?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| LauncherError::Network(e.to_string()))?;
            ctx.consume(&chunk);
            file.write_all(&chunk)?;
            got += chunk.len() as u64;
        }
        file.flush()?;
    }

    let digest = format!("{:x}", ctx.compute());
    if got != size || !digest.eq_ignore_ascii_case(md5) {
        let _ = std::fs::remove_file(&tmp);
        return Err(LauncherError::Data(format!(
            "checksum/size mismatch for {rel}"
        )));
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Full incremental sync of the `default` data set into `install_dir`.
/// Offline-safe: a failed manifest fetch returns a `skipped` summary, never an
/// error, so Play stays available with existing data. `verify = true` forces a
/// full re-hash classification ("Verify data").
pub async fn run_sync(
    app: &AppHandle,
    client: &reqwest::Client,
    install_dir: &Path,
    verify: bool,
) -> Result<SyncSummary> {
    use futures_util::stream::{self, StreamExt};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    let index_path = install_dir.join(".quetoo-launcher").join("data-index.json");
    let mut index = DataIndex::load(&index_path)?;

    let manifest = match fetch_manifest(client).await {
        Ok(m) => m,
        Err(_) => {
            return Ok(SyncSummary {
                skipped: true,
                ..Default::default()
            });
        }
    };

    // --- Plan pass (cheap): classify by stat only. Hashing is deferred to the
    // parallel pass below instead of blocking this loop one file at a time.
    let total = manifest.len();
    let mut to_download: Vec<ManifestEntry> = Vec::new();
    let mut to_hash: Vec<ManifestEntry> = Vec::new();
    for entry in &manifest {
        let dest = local_path(install_dir, &entry.path);
        let local = stat_local(&dest);
        match decide(entry, index.files.get(&entry.path), local.as_ref(), verify) {
            Decision::UpToDate => {
                if let Some(l) = local {
                    index.files.insert(
                        entry.path.clone(),
                        IndexEntry {
                            md5: entry.md5.clone(),
                            size: entry.size,
                            mtime: l.mtime,
                        },
                    );
                }
            }
            Decision::Download => to_download.push(entry.clone()),
            Decision::NeedsHash => to_hash.push(entry.clone()),
        }
    }

    // --- Hash pass (parallel): verify size-matched files whose trust is
    // unproven — mainly the one-time seed right after a bundle install. Each
    // hash runs on a blocking thread so many files are read+hashed at once,
    // instead of one-at-a-time on the async runtime (the slow part otherwise).
    let hash_total = to_hash.len();
    if hash_total > 0 {
        let hashed = Arc::new(AtomicUsize::new(0));
        installer::emit_progress(app, "data", 0, format!("Checking {hash_total} files"));
        let hash_results: Vec<std::result::Result<(ManifestEntry, i64), ManifestEntry>> =
            stream::iter(to_hash.into_iter().map(|entry| {
                let dest = local_path(install_dir, &entry.path);
                let app = app.clone();
                let hashed = hashed.clone();
                async move {
                    let size = entry.size;
                    let md5 = entry.md5.clone();
                    let d = dest.clone();
                    let ok = tokio::task::spawn_blocking(move || {
                        verify_file(&d, size, &md5).unwrap_or(false)
                    })
                    .await
                    .unwrap_or(false);
                    let n = hashed.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(100) || n == hash_total {
                        installer::emit_progress(
                            &app,
                            "data",
                            installer::percent(n as u64, hash_total as u64),
                            format!("Checking {n}/{hash_total} files"),
                        );
                    }
                    if ok {
                        let mtime = stat_local(&dest).map(|s| s.mtime).unwrap_or(0);
                        Ok((entry, mtime))
                    } else {
                        Err(entry)
                    }
                }
            }))
            .buffer_unordered(HASH_CONCURRENCY)
            .collect()
            .await;

        for r in hash_results {
            match r {
                Ok((entry, mtime)) => {
                    index.files.insert(
                        entry.path,
                        IndexEntry { md5: entry.md5, size: entry.size, mtime },
                    );
                }
                Err(entry) => to_download.push(entry),
            }
        }
        // Persist the seed immediately so an interrupted download pass doesn't
        // force a full re-hash on the next open.
        let _ = index.save(&index_path);
    }

    // --- Download pass: bounded concurrency + retry.
    let dl_total = to_download.len();
    let total_bytes: u64 = to_download.iter().map(|e| e.size).sum::<u64>().max(1);
    let bytes = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicUsize::new(0));

    let results: Vec<std::result::Result<(String, IndexEntry, u64), ()>> =
        stream::iter(to_download.into_iter().map(|entry| {
            let client = client.clone();
            let app = app.clone();
            let dest = local_path(install_dir, &entry.path);
            let bytes = bytes.clone();
            let done = done.clone();
            let path = entry.path;
            let md5 = entry.md5;
            let size = entry.size;
            async move {
                for attempt in 0..3u32 {
                    match download_one(&client, &path, size, &md5, &dest).await {
                        Ok(()) => {
                            let mtime = stat_local(&dest).map(|s| s.mtime).unwrap_or(0);
                            let nb = bytes.fetch_add(size, Ordering::Relaxed) + size;
                            let dc = done.fetch_add(1, Ordering::Relaxed) + 1;
                            installer::emit_progress(
                                &app,
                                "data",
                                installer::percent(nb, total_bytes),
                                format!("Downloaded {dc}/{dl_total} files"),
                            );
                            return Ok((path, IndexEntry { md5, size, mtime }, size));
                        }
                        Err(_) if attempt < 2 => {
                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        }
                        Err(_) => return Err(()),
                    }
                }
                Err(())
            }
        }))
        .buffer_unordered(6)
        .collect()
        .await;

    let mut downloaded = 0usize;
    let mut warnings = 0usize;
    let mut bytes_downloaded = 0u64;
    for r in results {
        match r {
            Ok((path, ie, b)) => {
                index.files.insert(path, ie);
                downloaded += 1;
                bytes_downloaded += b;
            }
            Err(()) => warnings += 1,
        }
    }

    // --- Prune pass: delete managed files no longer in the manifest.
    let prune = prune_set(&index, &manifest);
    let mut deleted = 0usize;
    for rel in &prune {
        let dest = local_path(install_dir, rel);
        if std::fs::remove_file(&dest).is_ok() {
            deleted += 1;
        }
        index.files.remove(rel);
    }

    index.save(&index_path)?;
    // Write the engine-format manifest so Quetoo's built-in installer sees the
    // synced data as current and doesn't re-download it on launch.
    let _ = write_engine_manifest(install_dir, &manifest, &index);
    installer::emit_progress(app, "data", 100, "Game data up to date".into());

    Ok(SyncSummary {
        checked: total,
        downloaded,
        deleted,
        bytes_downloaded,
        warnings,
        skipped: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_lines() {
        let text = "46315d3c415338d8cb384e7fd00e569c 14507 decals/blood_0.png\n\
                    c137e742e15fe1c3b9d0a015fb999cc6 8806677 maps/2deaths.bsp\n";
        let m = parse_manifest(text);
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[0],
            ManifestEntry {
                md5: "46315d3c415338d8cb384e7fd00e569c".into(),
                size: 14507,
                path: "decals/blood_0.png".into(),
            }
        );
        assert_eq!(m[1].path, "maps/2deaths.bsp");
    }

    #[test]
    fn skips_blank_and_malformed_lines() {
        let text = "\n   \n\
                    nothex_nothex_nothex_nothex_xxxx 10 a.png\n\
                    46315d3c415338d8cb384e7fd00e569c notanumber b.png\n\
                    46315d3c415338d8cb384e7fd00e569c 10\n\
                    46315d3c415338d8cb384e7fd00e569c 10 ok.png\n";
        let m = parse_manifest(text);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, "ok.png");
    }

    #[test]
    fn preserves_spaces_in_path() {
        let text = "46315d3c415338d8cb384e7fd00e569c 10 maps/my map.bsp\n";
        let m = parse_manifest(text);
        assert_eq!(m[0].path, "maps/my map.bsp");
    }

    #[test]
    fn rejects_unsafe_paths() {
        let text = "46315d3c415338d8cb384e7fd00e569c 10 ../evil.png\n\
                    46315d3c415338d8cb384e7fd00e569c 10 .rollback/x\n\
                    46315d3c415338d8cb384e7fd00e569c 10 safe.png\n";
        let m = parse_manifest(text);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, "safe.png");
    }

    #[test]
    fn local_path_prepends_share_default() {
        let p = local_path(Path::new("/games/quetoo"), "maps/2deaths.bsp");
        assert!(p.ends_with("share/default/maps/2deaths.bsp"), "{p:?}");
    }

    #[test]
    fn download_url_is_default_prefixed() {
        assert_eq!(
            download_url("maps/2deaths.bsp"),
            "https://quetoo-data.s3.amazonaws.com/default/maps/2deaths.bsp"
        );
    }

    #[test]
    fn index_load_missing_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let idx = DataIndex::load(&dir.path().join("data-index.json")).unwrap();
        assert_eq!(idx, DataIndex::default());
        assert_eq!(idx.version, 1);
        assert!(idx.files.is_empty());
    }

    #[test]
    fn index_save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("data-index.json");
        let mut idx = DataIndex::default();
        idx.files.insert(
            "maps/2deaths.bsp".into(),
            IndexEntry {
                md5: "abc".into(),
                size: 10,
                mtime: 123,
            },
        );
        idx.save(&path).unwrap();
        let loaded = DataIndex::load(&path).unwrap();
        assert_eq!(loaded, idx);
    }

    use std::io::Write as _;

    #[test]
    fn md5_file_matches_known_digest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        std::fs::File::create(&p).unwrap().write_all(b"abc").unwrap();
        // md5("abc") = 900150983cd24fb0d6963f7d28e17f72
        assert_eq!(md5_file(&p).unwrap(), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn verify_file_true_on_match_false_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.bin");
        std::fs::File::create(&p).unwrap().write_all(b"abc").unwrap();
        assert!(verify_file(&p, 3, "900150983cd24fb0d6963f7d28e17f72").unwrap());
        assert!(!verify_file(&p, 3, "deadbeefdeadbeefdeadbeefdeadbeef").unwrap()); // wrong md5
        assert!(!verify_file(&p, 4, "900150983cd24fb0d6963f7d28e17f72").unwrap()); // wrong size
        assert!(!verify_file(&dir.path().join("missing"), 3, "x").unwrap()); // missing
    }

    #[test]
    fn stat_local_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(stat_local(&dir.path().join("nope")).is_none());
    }

    fn entry(md5: &str, size: u64) -> ManifestEntry {
        ManifestEntry {
            md5: md5.into(),
            size,
            path: "p".into(),
        }
    }

    #[test]
    fn decide_download_when_missing_or_size_differs() {
        let e = entry("aa", 10);
        assert_eq!(decide(&e, None, None, false), Decision::Download);
        let local = LocalStat { size: 11, mtime: 5 };
        assert_eq!(decide(&e, None, Some(&local), false), Decision::Download);
    }

    #[test]
    fn decide_fast_path_uptodate_on_full_index_match() {
        let e = entry("aa", 10);
        let local = LocalStat { size: 10, mtime: 5 };
        let ix = IndexEntry {
            md5: "aa".into(),
            size: 10,
            mtime: 5,
        };
        assert_eq!(decide(&e, Some(&ix), Some(&local), false), Decision::UpToDate);
    }

    #[test]
    fn decide_needs_hash_when_unindexed_but_size_matches() {
        // First sync after a bundle install: file present, index empty.
        let e = entry("aa", 10);
        let local = LocalStat { size: 10, mtime: 5 };
        assert_eq!(decide(&e, None, Some(&local), false), Decision::NeedsHash);
    }

    #[test]
    fn decide_needs_hash_when_index_stale_by_mtime() {
        let e = entry("aa", 10);
        let local = LocalStat { size: 10, mtime: 9 };
        let ix = IndexEntry {
            md5: "aa".into(),
            size: 10,
            mtime: 5,
        };
        assert_eq!(decide(&e, Some(&ix), Some(&local), false), Decision::NeedsHash);
    }

    #[test]
    fn decide_verify_forces_hash_even_on_index_match() {
        let e = entry("aa", 10);
        let local = LocalStat { size: 10, mtime: 5 };
        let ix = IndexEntry {
            md5: "aa".into(),
            size: 10,
            mtime: 5,
        };
        assert_eq!(decide(&e, Some(&ix), Some(&local), true), Decision::NeedsHash);
    }

    #[test]
    fn channel_switch_is_a_noop_for_a_synced_install() {
        // A fully-synced install: every manifest entry has a matching index
        // record and local stat. After a channel switch (engine-only overlay),
        // the same manifest must classify every file UpToDate — no downloads.
        let manifest = vec![entry("aa", 10), {
            let mut e = entry("bb", 20);
            e.path = "q".into();
            e
        }];
        let index = {
            let mut m = BTreeMap::new();
            m.insert(
                "p".to_string(),
                IndexEntry {
                    md5: "aa".into(),
                    size: 10,
                    mtime: 1,
                },
            );
            m.insert(
                "q".to_string(),
                IndexEntry {
                    md5: "bb".into(),
                    size: 20,
                    mtime: 2,
                },
            );
            m
        };
        let stats = {
            let mut m = BTreeMap::new();
            m.insert("p".to_string(), LocalStat { size: 10, mtime: 1 });
            m.insert("q".to_string(), LocalStat { size: 20, mtime: 2 });
            m
        };
        for e in &manifest {
            let d = decide(e, index.get(&e.path), stats.get(&e.path), false);
            assert_eq!(d, Decision::UpToDate, "entry {} should be up to date", e.path);
        }
    }

    #[test]
    fn prune_set_lists_only_indexed_paths_missing_from_manifest() {
        let mut index = DataIndex::default();
        index.files.insert(
            "stays.png".into(),
            IndexEntry {
                md5: "a".into(),
                size: 1,
                mtime: 0,
            },
        );
        index.files.insert(
            "gone.png".into(),
            IndexEntry {
                md5: "b".into(),
                size: 1,
                mtime: 0,
            },
        );
        let manifest = vec![ManifestEntry {
            md5: "a".into(),
            size: 1,
            path: "stays.png".into(),
        }];
        assert_eq!(prune_set(&index, &manifest), vec!["gone.png".to_string()]);
    }

    #[test]
    fn prune_set_empty_when_manifest_covers_index() {
        let mut index = DataIndex::default();
        index.files.insert(
            "a.png".into(),
            IndexEntry {
                md5: "a".into(),
                size: 1,
                mtime: 0,
            },
        );
        let manifest = vec![ManifestEntry {
            md5: "a".into(),
            size: 1,
            path: "a.png".into(),
        }];
        assert!(prune_set(&index, &manifest).is_empty());
    }

    #[test]
    fn write_engine_manifest_lists_present_files_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        let manifest = vec![
            ManifestEntry { md5: "bb".into(), size: 2, path: "b.png".into() },
            ManifestEntry { md5: "aa".into(), size: 1, path: "a.png".into() },
            ManifestEntry { md5: "cc".into(), size: 3, path: "missing.png".into() },
        ];
        let mut index = DataIndex::default();
        index.files.insert("a.png".into(), IndexEntry { md5: "aa".into(), size: 1, mtime: 0 });
        index.files.insert("b.png".into(), IndexEntry { md5: "bb".into(), size: 2, mtime: 0 });
        // missing.png is in the manifest but not the index → excluded.
        write_engine_manifest(install, &manifest, &index).unwrap();
        let mf = std::fs::read_to_string(local_path(install, "manifest.mf")).unwrap();
        // Sorted by path, engine `hash size path` format, missing.png omitted.
        assert_eq!(mf, "aa 1 a.png\nbb 2 b.png\n");
    }
}
