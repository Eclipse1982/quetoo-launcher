//! Parallel segmented (HTTP Range) downloader for large single files.
//!
//! The game-data path (`data.rs`) already parallelizes across *many* small
//! files. The engine archive, by contrast, is one large file fetched as a
//! single serial stream in `installer::download_asset`. This module ports the
//! UO "Unchained" launcher's segmented-download technique to Rust: split the
//! byte range into N contiguous segments, fetch them concurrently with
//! `Range:` requests, then reassemble — with a clean serial fallback whenever
//! the server doesn't advertise range support or the file is too small to
//! bother.
//!
//! The pure planning (`plan_segments`) and reassembly (`assemble_parts`) logic
//! is unit-tested; the async fetch is thin glue over those tested pieces plus
//! the existing serial path, mirroring how `data.rs` leaves its network glue
//! (`download_one`/`run_sync`) untested while testing the pure helpers.

use crate::error::{LauncherError, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Split `total` bytes into at most `max_segments` contiguous, non-overlapping
/// inclusive byte ranges suitable for `Range: bytes=start-end`.
///
/// - `total == 0` → no segments (`[]`).
/// - `total <= min_segment` → a single segment covering everything.
/// - otherwise the segment count is `min(max_segments, ceil(total/min_segment))`
///   and the bytes are split as evenly as possible, with any remainder placed
///   on the leading segments (so no segment is ever empty).
///
/// `max_segments == 0` is treated as 1 and `min_segment == 0` as 1, so callers
/// can't trigger a divide-by-zero or an empty plan for a non-empty file.
pub fn plan_segments(total: u64, max_segments: usize, min_segment: u64) -> Vec<(u64, u64)> {
    if total == 0 {
        return Vec::new();
    }
    let min_segment = min_segment.max(1);
    let max_segments = (max_segments.max(1)) as u64;
    // Candidate count keeps every segment >= min_segment; cap at max_segments.
    let n = total.div_ceil(min_segment).clamp(1, max_segments);
    let base = total / n;
    let rem = total % n;
    let mut segs = Vec::with_capacity(n as usize);
    let mut start = 0u64;
    for i in 0..n {
        // Spread the remainder across the leading segments so none is empty.
        let len = base + u64::from(i < rem);
        let end = start + len - 1;
        segs.push((start, end));
        start = end + 1;
    }
    segs
}

/// Concatenate `parts` (in order) into `dest`, returning the total bytes
/// written. Writes to a sibling temp file and renames into place so a failure
/// never leaves a half-written `dest`. Creates `dest`'s parent directories.
pub fn assemble_parts(parts: &[PathBuf], dest: &Path) -> Result<u64> {
    use std::io::Write;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = dest.file_name().and_then(|s| s.to_str()).unwrap_or("download");
    let tmp = dest.with_file_name(format!(".{file_name}.asm"));

    // Assemble into a temp file; only rename into place on full success so a
    // mid-assembly failure never leaves a partial `dest`.
    let result = (|| -> Result<u64> {
        let mut out = std::fs::File::create(&tmp)?;
        let mut total = 0u64;
        for part in parts {
            let mut f = std::fs::File::open(part)?;
            total += std::io::copy(&mut f, &mut out)?;
        }
        out.flush()?;
        Ok(total)
    })();

    match result {
        Ok(total) => {
            std::fs::rename(&tmp, dest)?;
            Ok(total)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Max concurrent range segments for one file.
pub const MAX_SEGMENTS: usize = 8;
/// Smallest segment worth issuing a separate request for (1 MiB).
pub const MIN_SEGMENT: u64 = 1 << 20;
/// Files smaller than this download serially — segmenting tiny files only adds
/// round-trips (8 MiB).
pub const MIN_PARALLEL_SIZE: u64 = 8 << 20;

/// Decide whether (and how) to download a file of `total` bytes in parallel.
/// Returns the segment plan when the file is large enough to benefit and splits
/// into at least two segments; otherwise `None` (download serially).
pub fn parallel_plan(total: u64) -> Option<Vec<(u64, u64)>> {
    if total < MIN_PARALLEL_SIZE {
        return None;
    }
    let segs = plan_segments(total, MAX_SEGMENTS, MIN_SEGMENT);
    (segs.len() >= 2).then_some(segs)
}

// ── Async fetch glue ────────────────────────────────────────────────────────
//
// Untested here (no HTTP test server in the suite, matching `data.rs` /
// `installer.rs`). It is built only from the unit-tested `plan_segments` /
// `parallel_plan` / `assemble_parts`, and `installer::download_asset` always
// falls back to the existing serial stream if any of this returns an error, so
// a flaky CDN can never break a download — only make it slower.

/// Cheap probe: does the server honor HTTP Range for `url`? A compliant origin
/// answers a 1-byte range request with `206 Partial Content`.
pub async fn supports_range(client: &reqwest::Client, url: &str) -> bool {
    match client
        .get(url)
        .header(reqwest::header::USER_AGENT, "quetoo-launcher")
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
    {
        Ok(resp) => resp.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        Err(_) => false,
    }
}

/// Download `url` into `dest` as `segments.len()` concurrent Range requests,
/// each to its own `.partN` temp, then reassemble. `progress(done, total)` is
/// invoked (throttled to whole-percent changes) as bytes arrive. Errors leave
/// no `dest` behind and let the caller fall back to a serial download.
pub async fn fetch_parallel<F>(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    segments: &[(u64, u64)],
    progress: F,
) -> Result<()>
where
    F: Fn(u64, u64) + Send + Sync + 'static,
{
    use futures_util::stream::{self, StreamExt};

    let total: u64 = segments.iter().map(|(s, e)| e - s + 1).sum();
    let file_name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download")
        .to_string();
    let concurrency = segments.len().clamp(1, MAX_SEGMENTS);

    // Shared, throttled progress reporter. Concurrent segments add their chunk
    // sizes to one cumulative counter; we emit only when the whole percent
    // changes to avoid flooding the UI event channel.
    let cumulative = Arc::new(AtomicU64::new(0));
    let last_pct = Arc::new(AtomicU64::new(u64::MAX));
    let report: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |n: u64| {
        let done = cumulative.fetch_add(n, Ordering::Relaxed) + n;
        let pct = if total == 0 { 100 } else { done.min(total) * 100 / total };
        if last_pct.swap(pct, Ordering::Relaxed) != pct {
            progress(done, total);
        }
    });

    let results: Vec<Result<(usize, PathBuf)>> = stream::iter(
        segments.iter().copied().enumerate().map(|(i, (start, end))| {
            let client = client.clone();
            let url = url.to_string();
            let part_path = dest.with_file_name(format!(".{file_name}.part{i}"));
            let report = report.clone();
            async move {
                download_range(&client, &url, start, end, &part_path, report.as_ref())
                    .await
                    .map(|()| (i, part_path))
            }
        }),
    )
    .buffer_unordered(concurrency)
    .collect()
    .await;

    // Reorder parts by segment index; collect the first error if any.
    let mut parts: Vec<Option<PathBuf>> = (0..segments.len()).map(|_| None).collect();
    let mut first_err: Option<LauncherError> = None;
    for r in results {
        match r {
            Ok((i, p)) => parts[i] = Some(p),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    let cleanup = |parts: &[Option<PathBuf>]| {
        for p in parts.iter().flatten() {
            let _ = std::fs::remove_file(p);
        }
    };
    if let Some(e) = first_err {
        cleanup(&parts);
        return Err(e);
    }

    let ordered: Vec<PathBuf> = parts.into_iter().map(|p| p.expect("no gaps")).collect();
    let assembled = assemble_parts(&ordered, dest);
    for p in &ordered {
        let _ = std::fs::remove_file(p);
    }
    let written = assembled?;
    if written != total {
        let _ = std::fs::remove_file(dest);
        return Err(LauncherError::Network(format!(
            "assembled {written} bytes, expected {total}"
        )));
    }
    Ok(())
}

/// Fetch one byte range to `part_path` with up to 3 attempts. Each attempt
/// recreates the part file, so a retry never appends to a partial segment.
async fn download_range(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
    part_path: &Path,
    report: &(dyn Fn(u64) + Send + Sync),
) -> Result<()> {
    let expected = end - start + 1;
    let mut last_err: Option<LauncherError> = None;
    for attempt in 0..3u32 {
        match try_range(client, url, start, end, part_path, report).await {
            Ok(got) if got == expected => return Ok(()),
            Ok(got) => {
                last_err = Some(LauncherError::Network(format!(
                    "segment {start}-{end}: got {got} bytes, expected {expected}"
                )));
            }
            Err(e) => last_err = Some(e),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }
    Err(last_err.unwrap_or_else(|| LauncherError::Network("segment download failed".into())))
}

async fn try_range(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
    part_path: &Path,
    report: &(dyn Fn(u64) + Send + Sync),
) -> Result<u64> {
    use futures_util::StreamExt;
    use std::io::Write;

    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "quetoo-launcher")
        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
        .send()
        .await
        .map_err(|e| LauncherError::Network(e.to_string()))?;
    // A 200 here means the origin ignored Range and would stream the whole
    // file into one segment — unusable. Demand 206 Partial Content.
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(LauncherError::Network(format!(
            "range {start}-{end} not honored: HTTP {}",
            resp.status()
        )));
    }
    let mut file = std::fs::File::create(part_path)?;
    let mut got = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LauncherError::Network(e.to_string()))?;
        file.write_all(&chunk)?;
        got += chunk.len() as u64;
        report(chunk.len() as u64);
    }
    file.flush()?;
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_zero_total_is_empty() {
        assert_eq!(plan_segments(0, 8, 1024), Vec::<(u64, u64)>::new());
    }

    #[test]
    fn plan_small_total_single_segment() {
        // total at/below min_segment → one segment covering everything.
        assert_eq!(plan_segments(1000, 8, 4096), vec![(0, 999)]);
        assert_eq!(plan_segments(4096, 8, 4096), vec![(0, 4095)]);
    }

    #[test]
    fn plan_exact_even_split() {
        // 10000/1000 = 10 candidate segments, capped at 4 → 2500 each.
        assert_eq!(
            plan_segments(10_000, 4, 1000),
            vec![(0, 2499), (2500, 4999), (5000, 7499), (7500, 9999)]
        );
    }

    #[test]
    fn plan_distributes_remainder_to_leading_segments() {
        // total 10, min 1 → 10 candidate, capped 4 → base 2, rem 2 → 3,3,2,2.
        assert_eq!(plan_segments(10, 4, 1), vec![(0, 2), (3, 5), (6, 7), (8, 9)]);
    }

    #[test]
    fn plan_caps_at_max_segments() {
        let segs = plan_segments(1_000_000, 8, 1024);
        assert_eq!(segs.len(), 8);
        assert_eq!(segs.first().unwrap().0, 0);
        assert_eq!(segs.last().unwrap().1, 999_999);
    }

    #[test]
    fn plan_segments_are_contiguous_and_cover_total() {
        for total in [1u64, 2, 3, 7, 100, 4096, 4097, 123_456, 999_983] {
            let segs = plan_segments(total, 8, 1024);
            assert!(!segs.is_empty(), "total={total} produced no segments");
            assert_eq!(segs.first().unwrap().0, 0, "first start != 0 (total={total})");
            assert_eq!(segs.last().unwrap().1, total - 1, "last end != total-1 (total={total})");
            for w in segs.windows(2) {
                assert_eq!(w[1].0, w[0].1 + 1, "gap/overlap (total={total}): {segs:?}");
            }
            for (s, e) in &segs {
                assert!(s <= e, "empty segment (total={total}): {segs:?}");
            }
        }
    }

    #[test]
    fn plan_guards_zero_min_segment() {
        // min_segment 0 must be treated as 1 (no divide-by-zero) and still cover all.
        let segs = plan_segments(10, 4, 0);
        assert_eq!(segs.first().unwrap().0, 0);
        assert_eq!(segs.last().unwrap().1, 9);
        assert!(!segs.is_empty());
    }

    #[test]
    fn plan_guards_zero_max_segments() {
        // max_segments 0 → at least one segment covering everything.
        assert_eq!(plan_segments(100, 0, 1024), vec![(0, 99)]);
    }

    // ── assemble_parts ──────────────────────────────────────────────────────

    #[test]
    fn assemble_concatenates_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let p0 = dir.path().join("a.part0");
        let p1 = dir.path().join("a.part1");
        let p2 = dir.path().join("a.part2");
        std::fs::write(&p0, b"aaa").unwrap();
        std::fs::write(&p1, b"bb").unwrap();
        std::fs::write(&p2, b"cccc").unwrap();
        let dest = dir.path().join("out.bin");
        let n = assemble_parts(&[p0, p1, p2], &dest).unwrap();
        assert_eq!(n, 9);
        assert_eq!(std::fs::read(&dest).unwrap(), b"aaabbcccc");
    }

    #[test]
    fn assemble_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let p0 = dir.path().join("x.part0");
        std::fs::write(&p0, b"data").unwrap();
        let dest = dir.path().join("nested").join("deep").join("out.bin");
        let n = assemble_parts(&[p0], &dest).unwrap();
        assert_eq!(n, 4);
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    }

    #[test]
    fn assemble_missing_part_errors_and_leaves_no_dest() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let missing = dir.path().join("nope.part0");
        let r = assemble_parts(&[missing], &dest);
        assert!(r.is_err(), "missing part must error");
        assert!(!dest.exists(), "dest must not be left behind on failure");
    }

    #[test]
    fn assemble_no_parts_writes_empty_dest() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let n = assemble_parts(&[], &dest).unwrap();
        assert_eq!(n, 0);
        assert_eq!(std::fs::read(&dest).unwrap(), b"");
    }

    // ── parallel_plan ───────────────────────────────────────────────────────

    #[test]
    fn parallel_plan_none_when_small() {
        assert!(parallel_plan(0).is_none());
        assert!(parallel_plan(MIN_PARALLEL_SIZE - 1).is_none());
    }

    #[test]
    fn parallel_plan_at_threshold_is_some_with_multiple_segments() {
        let segs = parallel_plan(MIN_PARALLEL_SIZE).unwrap();
        assert!(segs.len() >= 2, "threshold file must split into >= 2 segments");
    }

    #[test]
    fn parallel_plan_large_file_covers_total_contiguously() {
        let total = 50 * (1 << 20); // 50 MiB
        let segs = parallel_plan(total).unwrap();
        assert!(segs.len() >= 2);
        assert!(segs.len() <= MAX_SEGMENTS);
        assert_eq!(segs.first().unwrap().0, 0);
        assert_eq!(segs.last().unwrap().1, total - 1);
        for w in segs.windows(2) {
            assert_eq!(w[1].0, w[0].1 + 1);
        }
    }
}
