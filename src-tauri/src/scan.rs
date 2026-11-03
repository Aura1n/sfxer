use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::db::{self, NewTrack, SharedDb};
use crate::model::{ScanProgress, ScanSummary, Settings};
use crate::probe;

/// Everything we can at least identify and index. Formats outside symphonia's
/// reach (WMA) still land in the library; they simply carry no waveform.
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "mp2", "mp1", "wav", "wave", "bwf", "flac", "ogg", "oga", "opus", "m4a", "m4b", "mp4", "aac", "aif",
    "aiff", "caf", "wma", "ape", "mpc", "wv", "mka", "mkv", "dsf", "3gp", "amr", "ac3", "eac3", "aob",
];

pub fn is_audio_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => !ext.is_empty() && AUDIO_EXTS.contains(&ext.to_lowercase().as_str()),
        None => false,
    }
}

#[derive(Clone)]
pub struct ScanControl {
    pub abort: Arc<AtomicBool>,
    pub running: Arc<AtomicBool>,
}

impl Default for ScanControl {
    fn default() -> Self {
        Self { abort: Arc::new(AtomicBool::new(false)), running: Arc::new(AtomicBool::new(false)) }
    }
}

/// Names we never descend into: version-control and editor side artifacts.
fn skip_dir(entry: &Path, allow_hidden: bool) -> bool {
    let name = entry.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name.eq_ignore_ascii_case("node_modules") || name.eq_ignore_ascii_case("$RECYCLE.BIN") {
        return true;
    }
    if !allow_hidden && (name.starts_with('.') || name.starts_with('_')) {
        return true;
    }
    false
}

fn collect(root: &Path, settings: &Settings) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && skip_dir(e.path(), settings.scan_hidden)))
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_audio_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        out.push(entry.path().to_path_buf());
    }
    out
}

fn split_path(path: &str) -> (String, String, String) {
    let p = Path::new(path);
    let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    let ext = p.extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();
    (name, ext, dir)
}

fn mtime_secs(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Walks each root, probing only what changed. Runs on its own thread; `app`
/// exists purely to publish progress.
pub fn run(
    db: SharedDb,
    app: AppHandle,
    targets: Vec<(i64, PathBuf)>,
    force: bool,
    settings: Settings,
    control: ScanControl,
) -> ScanSummary {
    let started = Instant::now();
    control.running.store(true, Ordering::SeqCst);
    control.abort.store(false, Ordering::SeqCst);

    let mut summary = ScanSummary { scanned: 0, added: 0, updated: 0, removed: 0, errors: 0, seconds: 0.0 };
    let buckets = settings.peak_buckets.max(32) as usize;

    for (root_id, root_path) in &targets {
        if control.abort.load(Ordering::SeqCst) {
            break;
        }
        let _ = app.emit(
            "scan://progress",
            ScanProgress { active: true, phase: "列举文件".into(), done: 0, total: 0, current: root_path.display().to_string() },
        );

        let candidates = collect(root_path, &settings);
        let total = candidates.len() as i64;

        // `fingerprints` is this root's own rows and drives the pruning below;
        // `indexed` covers the whole library, because a file that an overlapping
        // root already owns keeps its original root and would otherwise be
        // re-probed on every scan.
        let (fingerprints, indexed) = match db.lock() {
            Ok(conn) => (
                db::fingerprint_map(&conn, Some(*root_id)).unwrap_or_default(),
                db::fingerprint_map(&conn, None).unwrap_or_default(),
            ),
            Err(_) => break,
        };

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut done = 0i64;
        let mut last_emit = Instant::now();

        for path in &candidates {
            if control.abort.load(Ordering::SeqCst) {
                break;
            }
            done += 1;
            summary.scanned += 1;

            let db_path = db::strip_verbatim(&path.to_string_lossy());
            let key = db_path.to_lowercase();
            seen.insert(key.clone());

            let Ok(meta) = path.metadata() else {
                summary.errors += 1;
                continue;
            };
            if meta.len() < settings.min_size_bytes.max(0) as u64 {
                continue;
            }
            let mtime = mtime_secs(&meta);

            if let Some((_, old_size, old_mtime)) = indexed.get(&key) {
                if !force && *old_size == meta.len() as i64 && *old_mtime == mtime {
                    if last_emit.elapsed().as_millis() > 220 {
                        emit(&app, "检查", done, total, &db_path);
                        last_emit = Instant::now();
                    }
                    continue;
                }
                summary.updated += 1;
            } else {
                summary.added += 1;
            }

            let meta_info = match probe::read_meta(path) {
                Ok(m) => m,
                Err(_) => {
                    summary.errors += 1;
                    continue;
                }
            };

            let wave = if settings.gen_peaks { probe::read_wave(path, buckets).ok() } else { None };
            let (name, ext, dir) = split_path(&db_path);

            let track = NewTrack {
                path: db_path.clone(),
                name,
                ext,
                dir,
                root_id: Some(*root_id),
                size_bytes: meta.len() as i64,
                mtime,
                duration_ms: meta_info.duration_ms,
                sample_rate: meta_info.sample_rate,
                bit_depth: meta_info.bit_depth,
                channels: meta_info.channels,
                bitrate: meta_info.bitrate,
                codec: meta_info.codec,
                container: meta_info.container,
                peak_db: wave.as_ref().map(|w| w.peak_db),
                rms_db: wave.as_ref().map(|w| w.rms_db),
                decodeable: wave.is_some(),
                clipped: wave.as_ref().map(|w| w.clipped).unwrap_or(false),
                peaks: wave.as_ref().map(|w| db::peaks_to_bytes(&w.peaks)),
            };

            if let Ok(conn) = db.lock() {
                let _ = db::upsert_track(&conn, &track, force);
            }

            if last_emit.elapsed().as_millis() > 220 {
                emit(&app, "解析", done, total, &db_path);
                last_emit = Instant::now();
            }
        }

        // Anything recorded under this root that the walk did not see and that
        // no longer exists on disk leaves the library.
        if !control.abort.load(Ordering::SeqCst) {
            if let Ok(conn) = db.lock() {
                for (path, (id, _, _)) in &fingerprints {
                    if !seen.contains(&path.to_lowercase()) && !Path::new(path).exists() {
                        if db::delete_track(&conn, *id).is_ok() {
                            summary.removed += 1;
                        }
                    }
                }
                let _ = db::touch_scan(&conn, *root_id);
            }
        }
    }

    summary.seconds = started.elapsed().as_secs_f64();
    control.running.store(false, Ordering::SeqCst);
    let _ = app.emit("scan://done", summary.clone());
    summary
}

fn emit(app: &AppHandle, phase: &str, done: i64, total: i64, current: &str) {
    let _ = app.emit(
        "scan://progress",
        ScanProgress {
            active: true,
            phase: phase.into(),
            done,
            total,
            current: current.rsplit(['\\', '/']).next().unwrap_or("").to_string(),
        },
    );
}

/// Recomputes waveforms for files already in the library.
pub fn rebuild(db: SharedDb, app: AppHandle, buckets: usize, control: ScanControl) {
    control.running.store(true, Ordering::SeqCst);
    control.abort.store(false, Ordering::SeqCst);
    let list = match db.lock() {
        Ok(conn) => db::all_tracks_for_rebuild(&conn).unwrap_or_default(),
        Err(_) => {
            control.running.store(false, Ordering::SeqCst);
            return;
        }
    };
    let total = list.len() as i64;
    for (i, (id, path)) in list.iter().enumerate() {
        if control.abort.load(Ordering::SeqCst) {
            break;
        }
        let p = Path::new(path);
        match probe::read_wave(p, buckets.max(32)) {
            Ok(w) => {
                if let Ok(conn) = db.lock() {
                    let _ = db::set_wave(&conn, *id, &db::peaks_to_bytes(&w.peaks), w.peak_db, w.rms_db, w.clipped);
                }
            }
            Err(_) => {
                if let Ok(conn) = db.lock() {
                    let _ = db::mark_undecodable(&conn, *id);
                }
            }
        }
        if i % 40 == 0 {
            emit(&app, "重建波形", i as i64, total, path);
        }
    }
    control.running.store(false, Ordering::SeqCst);
    let _ = app.emit(
        "scan://done",
        ScanSummary { scanned: total, added: 0, updated: total, removed: 0, errors: 0, seconds: 0.0 },
    );
}
