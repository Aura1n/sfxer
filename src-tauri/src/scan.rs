use std::collections::{HashMap, HashSet};
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::db::{self, NewTrack, SharedDb};
use crate::model::{ScanProgress, ScanSummary, Settings};
use crate::probe;

/// Everything we can at least identify and index. Formats outside the decoders'
/// reach still land in the library; they simply carry no waveform.
pub const AUDIO_EXTS: &[&str] = &[
    "mp1", "mp2", "mp3", "mpga", "wav", "wave", "bwf", "w64", "rf64", "bw64", "flac", "ogg", "oga",
    "opus", "spx", "m4a", "m4b", "m4p", "m4r", "mp4", "aac", "adts", "aif", "aiff", "aifc", "au",
    "snd", "caf", "wma", "ape", "mpc", "wv", "tta", "tak", "mka", "mkv", "webm", "dsf", "dff",
    "3gp", "amr", "ac3", "eac3", "dts", "aob",
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

fn collect(root: &Path, settings: &Settings, control: &ScanControl, app: &AppHandle) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut last_emit = Instant::now();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && skip_dir(e.path(), settings.scan_hidden)))
    {
        if control.abort.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_audio_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        out.push(entry.path().to_path_buf());
        if out.len() % 200 == 0 && last_emit.elapsed() >= Duration::from_millis(160) {
            last_emit = Instant::now();
            emit(app, "查找文件", out.len() as i64, 0, &entry.file_name().to_string_lossy());
        }
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

/// One finished file, sent from a probe worker to the single database writer.
struct ProbeMsg {
    path: String,
    skipped: bool,
    result: ProbeResult,
}

enum ProbeResult {
    Track { track: NewTrack, existed: bool },
    Meta { id: i64, meta: probe::Meta },
    Skip,
    Error,
}

type Fingerprints = HashMap<String, (i64, i64, i64, bool)>;

/// Pure per-file work: stat, compare with the library, probe, decode. Runs on
/// several threads at once, so it never touches the database.
fn probe_file(
    path: &Path,
    root_id: i64,
    indexed: &Fingerprints,
    force: bool,
    settings: &Settings,
    buckets: usize,
) -> ProbeMsg {
    let db_path = db::strip_verbatim(&path.to_string_lossy());
    let Ok(meta) = path.metadata() else {
        return ProbeMsg { path: db_path, skipped: false, result: ProbeResult::Error };
    };
    if meta.len() < settings.min_size_bytes.max(0) as u64 {
        return ProbeMsg { path: db_path, skipped: true, result: ProbeResult::Skip };
    }
    let mtime = mtime_secs(&meta);
    let existing = indexed.get(&db_path.to_lowercase());

    if let Some((id, old_size, old_mtime, needs_meta)) = existing {
        if !force && *old_size == meta.len() as i64 && *old_mtime == mtime {
            if !*needs_meta {
                return ProbeMsg { path: db_path, skipped: true, result: ProbeResult::Skip };
            }
            // The file itself is unchanged, but an older build could not read
            // its properties. Repair those without decoding the audio again.
            return match probe::read_meta(path) {
                Ok(meta) if meta.duration_ms.is_some() || meta.sample_rate.is_some() => {
                    ProbeMsg {
                        path: db_path,
                        skipped: true,
                        result: ProbeResult::Meta { id: *id, meta },
                    }
                }
                _ => ProbeMsg { path: db_path, skipped: true, result: ProbeResult::Skip },
            };
        }
    }

    // A file we cannot describe is still a file the user owns: it enters the
    // library with blank metadata rather than vanishing from the results.
    let meta_info = probe::read_meta(path).ok();
    let wave = if settings.gen_peaks { probe::read_wave(path, buckets).ok() } else { None };
    let (name, ext, dir) = split_path(&db_path);

    let track = NewTrack {
        path: db_path.clone(),
        name,
        ext,
        dir,
        root_id: Some(root_id),
        size_bytes: meta.len() as i64,
        mtime,
        duration_ms: meta_info.as_ref().and_then(|m| m.duration_ms),
        sample_rate: meta_info.as_ref().and_then(|m| m.sample_rate),
        bit_depth: meta_info.as_ref().and_then(|m| m.bit_depth),
        channels: meta_info.as_ref().and_then(|m| m.channels),
        bitrate: meta_info.as_ref().and_then(|m| m.bitrate),
        codec: meta_info.as_ref().and_then(|m| m.codec.clone()),
        container: meta_info.as_ref().and_then(|m| m.container.clone()),
        peak_db: wave.as_ref().map(|w| w.peak_db),
        rms_db: wave.as_ref().map(|w| w.rms_db),
        decodeable: wave.is_some(),
        clipped: wave.as_ref().map(|w| w.clipped).unwrap_or(false),
        peaks: wave.as_ref().map(|w| db::peaks_to_bytes(&w.peaks)),
    };

    ProbeMsg {
        path: db_path,
        skipped: false,
        result: ProbeResult::Track { track, existed: existing.is_some() },
    }
}

/// Decoding is CPU-bound, so a handful of workers saturate a modern laptop
/// without starving the UI thread or thrashing a spinning disk.
fn worker_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8)
}

const WRITE_BATCH: usize = 256;

fn flush_batch(
    db: &SharedDb,
    batch: &mut Vec<NewTrack>,
    meta_batch: &mut Vec<(i64, probe::Meta)>,
    force: bool,
) {
    if batch.is_empty() && meta_batch.is_empty() {
        return;
    }
    if let Ok(conn) = db.lock() {
        // One transaction per batch instead of one autocommit per row: 30k files
        // otherwise pay 30k journal syncs.
        match conn.unchecked_transaction() {
            Ok(tx) => {
                for t in batch.iter() {
                    let _ = db::upsert_track(&tx, t, force);
                }
                for (id, meta) in meta_batch.iter() {
                    let _ = db::update_meta(&tx, *id, meta);
                }
                let _ = tx.commit();
            }
            Err(_) => {
                for t in batch.iter() {
                    let _ = db::upsert_track(&conn, t, force);
                }
                for (id, meta) in meta_batch.iter() {
                    let _ = db::update_meta(&conn, *id, meta);
                }
            }
        }
    }
    batch.clear();
    meta_batch.clear();
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
            ScanProgress { active: true, phase: "查找文件".into(), done: 0, total: 0, current: root_path.display().to_string() },
        );

        let candidates = collect(root_path, &settings, &control, &app);
        if control.abort.load(Ordering::SeqCst) {
            break;
        }
        let total = candidates.len() as i64;
        // Shows "0 / N" right away instead of waiting for the first file to
        // finish probing, which can take a moment on a cold disk.
        emit(&app, "解析", 0, total, "");

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

        let seen: HashSet<String> = candidates
            .iter()
            .map(|p| db::strip_verbatim(&p.to_string_lossy()).to_lowercase())
            .collect();

        let cursor = AtomicUsize::new(0);
        let (tx, rx) = mpsc::sync_channel::<ProbeMsg>(512);

        std::thread::scope(|scope| {
            for _ in 0..worker_count() {
                let tx = tx.clone();
                let candidates = &candidates;
                let indexed = &indexed;
                let settings = &settings;
                let control = &control;
                let cursor = &cursor;
                scope.spawn(move || loop {
                    if control.abort.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    if i >= candidates.len() {
                        break;
                    }
                    let msg = probe_file(&candidates[i], *root_id, indexed, force, settings, buckets);
                    if tx.send(msg).is_err() {
                        break;
                    }
                });
            }
            drop(tx);

            let mut batch: Vec<NewTrack> = Vec::with_capacity(WRITE_BATCH);
            let mut meta_batch: Vec<(i64, probe::Meta)> = Vec::new();
            let mut phase = "解析";
            let mut current = String::new();
            // `summary.scanned` accumulates across roots; the bar needs the
            // count for this root only.
            let mut root_scanned = 0i64;
            let mut last_emit = Instant::now();

            loop {
                match rx.recv_timeout(Duration::from_millis(220)) {
                    Ok(msg) => {
                        summary.scanned += 1;
                        root_scanned += 1;
                        current = msg.path;
                        phase = if msg.skipped { "检查" } else { "解析" };
                        match msg.result {
                            ProbeResult::Track { track, existed } => {
                                if existed {
                                    summary.updated += 1;
                                } else {
                                    summary.added += 1;
                                }
                                batch.push(track);
                                if batch.len() >= WRITE_BATCH {
                                    flush_batch(&db, &mut batch, &mut meta_batch, force);
                                }
                            }
                            ProbeResult::Meta { id, meta } => {
                                summary.updated += 1;
                                meta_batch.push((id, meta));
                            }
                            ProbeResult::Skip => {}
                            ProbeResult::Error => summary.errors += 1,
                        }
                        // Emitting only after a lull would freeze the bar while
                        // small files stream in non-stop, so pace it by time.
                        if last_emit.elapsed() >= Duration::from_millis(160) {
                            flush_batch(&db, &mut batch, &mut meta_batch, force);
                            emit(&app, phase, root_scanned, total, &current);
                            last_emit = Instant::now();
                        }
                    }
                    // A long file can take longer than the emit interval, so the
                    // timeout also keeps the progress line moving.
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        flush_batch(&db, &mut batch, &mut meta_batch, force);
                        emit(&app, phase, root_scanned, total, &current);
                        last_emit = Instant::now();
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            flush_batch(&db, &mut batch, &mut meta_batch, force);
        });

        // Anything recorded under this root that the walk did not see and that
        // no longer exists on disk leaves the library.
        if !control.abort.load(Ordering::SeqCst) {
            if let Ok(conn) = db.lock() {
                for (path, (id, _, _, _)) in &fingerprints {
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
