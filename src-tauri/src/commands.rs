//! The command surface the webview calls. Every statement lives in `db.rs`;
//! this file only translates between the two sides and owns the threads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::db::{self, SharedDb};
use crate::model::*;
use crate::player::PlayerCtl;
use crate::scan::{self, ScanControl};
use crate::update;

/// Tauri turns `Err` into a rejected promise carrying this string.
type Cmd<T> = Result<T, String>;

fn fail<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn conn(db: &SharedDb) -> MutexGuard<'_, Connection> {
    db.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct AppState {
    pub db: SharedDb,
    pub player: Arc<PlayerCtl>,
    pub scan: ScanControl,
    pub undo: Mutex<Vec<UndoEntry>>,
}

/// One reversible user action. Kept in memory only: undo history starts fresh
/// each launch, and the cap keeps a mass delete from pinning the whole library.
pub enum UndoEntry {
    Fields { label: String, snaps: Vec<TrackSnap> },
    Removed { label: String, rows: Vec<db::RemovedTrack> },
    Files { label: String, rows: Vec<db::RemovedTrack> },
    IgnoredDup { sig: String },
}

const UNDO_CAP: usize = 30;

fn push_undo(state: &AppState, entry: UndoEntry) {
    let mut stack = state.undo.lock().unwrap_or_else(|p| p.into_inner());
    stack.push(entry);
    if stack.len() > UNDO_CAP {
        stack.remove(0);
    }
}

impl AppState {
    pub fn new(db: SharedDb, player: PlayerCtl) -> Self {
        let player = Arc::new(player);
        let volume = db::get_settings(&conn(&db)).map(|s| s.volume).unwrap_or(0.7);
        player.set_volume(volume);
        Self { db, player, scan: ScanControl::default(), undo: Mutex::new(Vec::new()) }
    }
}

/// Spawns the walker on its own thread; progress arrives as `scan://` events.
fn spawn_scan(app: AppHandle, state: &AppState, root_id: Option<i64>, force: bool) -> Cmd<bool> {
    if state.scan.running.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let guard = conn(&state.db);
    let settings = db::get_settings(&guard).map_err(fail)?;
    let targets: Vec<(i64, PathBuf)> = db::list_roots(&guard)
        .map_err(fail)?
        .into_iter()
        .filter(|r| root_id.map(|id| r.id == id).unwrap_or(true))
        .map(|r| (r.id, PathBuf::from(&r.path)))
        .collect();
    drop(guard);
    if targets.is_empty() {
        return Ok(false);
    }
    state.scan.running.store(true, Ordering::SeqCst);
    let control = state.scan.clone();
    std::thread::spawn(move || {
        let db = app.state::<AppState>().db.clone();
        scan::run(db, app, targets, force, settings, control, Vec::new());
    });
    Ok(true)
}

#[derive(serde::Serialize)]
pub struct DropReport {
    pub files: usize,
    pub folders: usize,
    pub skipped: usize,
    pub started: bool,
}

/// Drag & drop entry: folders become roots, loose audio files are probed on
/// their own so dropping one file does not import its whole directory.
#[tauri::command]
pub fn import_paths(app: AppHandle, state: State<AppState>, paths: Vec<String>) -> Cmd<DropReport> {
    if state.scan.running.load(Ordering::SeqCst) {
        return Err("正在导入，请稍候再试".into());
    }
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut skipped = 0usize;
    for p in &paths {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            folders.push(p.clone());
        } else if pb.is_file()
            && scan::is_audio_name(&pb.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        {
            files.push(pb);
        } else {
            skipped += 1;
        }
    }
    let mut targets = Vec::new();
    {
        let guard = conn(&state.db);
        for f in &folders {
            let root = db::add_root(&guard, f).map_err(fail)?;
            targets.push((root.id, PathBuf::from(&root.path)));
        }
    }
    let report = DropReport { files: files.len(), folders: folders.len(), skipped, started: false };
    if files.is_empty() && targets.is_empty() {
        return Ok(report);
    }
    let settings = db::get_settings(&conn(&state.db)).map_err(fail)?;
    state.scan.running.store(true, Ordering::SeqCst);
    let control = state.scan.clone();
    let db_arc = state.db.clone();
    std::thread::spawn(move || {
        scan::run(db_arc, app, targets, false, settings, control, files);
    });
    Ok(DropReport { started: true, ..report })
}

/* ---------------------------------------------------------------- library */

#[tauri::command]
pub async fn pick_root() -> Cmd<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new().set_title("选择音频文件夹").pick_folder()
    })
    .await
    .map_err(fail)?;
    Ok(picked.map(|p| p.to_string_lossy().to_string()))
}

/// Native multi-select picker for loose audio files. The files stay where they
/// are on disk; only the library gains rows for them.
#[tauri::command]
pub async fn pick_audio_files() -> Cmd<Vec<String>> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("选择音频文件")
            .add_filter("音频", scan::AUDIO_EXTS)
            .pick_files()
    })
    .await
    .map_err(fail)?;
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

#[tauri::command]
pub async fn pick_save_json() -> Cmd<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("导出标签与备注")
            .set_file_name("sfxer-backup.json")
            .add_filter("JSON", &["json"])
            .save_file()
    })
    .await
    .map_err(fail)?;
    Ok(picked.map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
pub async fn pick_open_json() -> Cmd<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new().set_title("选择备份文件").add_filter("JSON", &["json"]).pick_file()
    })
    .await
    .map_err(fail)?;
    Ok(picked.map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
pub fn list_roots(state: State<AppState>) -> Cmd<Vec<Root>> {
    db::list_roots(&conn(&state.db)).map_err(fail)
}

#[tauri::command]
pub fn add_root(app: AppHandle, state: State<AppState>, path: String) -> Cmd<Root> {
    let root = db::add_root(&conn(&state.db), &path).map_err(fail)?;
    let _ = spawn_scan(app, &state, Some(root.id), false);
    Ok(root)
}

#[tauri::command]
pub fn remove_root(state: State<AppState>, id: i64) -> Cmd<()> {
    db::remove_root(&conn(&state.db), id).map_err(fail)
}

#[tauri::command]
pub fn scan(app: AppHandle, state: State<AppState>, root_id: Option<i64>, force: bool) -> Cmd<bool> {
    spawn_scan(app, &state, root_id, force)
}

#[tauri::command]
pub fn abort_scan(state: State<AppState>) -> Cmd<()> {
    state.scan.abort.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn stats(state: State<AppState>) -> Cmd<Stats> {
    db::stats(&conn(&state.db)).map_err(fail)
}

#[tauri::command]
pub fn find_duplicates(state: State<AppState>) -> Cmd<Vec<DupGroup>> {
    db::duplicates(&conn(&state.db)).map_err(fail)
}

/// Auditioning showed distinct takes, so stop reporting this group as copies.
#[tauri::command]
pub fn ignore_duplicates(state: State<AppState>, sig: String) -> Cmd<()> {
    db::ignore_dup(&conn(&state.db), &sig).map_err(fail)?;
    push_undo(&state, UndoEntry::IgnoredDup { sig });
    Ok(())
}

#[tauri::command]
pub fn rebuild_peaks(app: AppHandle, state: State<AppState>) -> Cmd<bool> {
    if state.scan.running.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let guard = conn(&state.db);
    let buckets = db::get_settings(&guard).map_err(fail)?.peak_buckets.max(32) as usize;
    drop(guard);
    state.scan.running.store(true, Ordering::SeqCst);
    let control = state.scan.clone();
    let db_arc = state.db.clone();
    std::thread::spawn(move || scan::rebuild(db_arc, app, buckets, control));
    Ok(true)
}

/// Re-reads only the files the library marked undecodable: after a decoder
/// upgrade, or after ffmpeg was installed, this is all that needs repeating.
#[tauri::command]
pub fn retry_undecodable(app: AppHandle, state: State<AppState>) -> Cmd<bool> {
    if state.scan.running.load(Ordering::SeqCst) {
        return Ok(false);
    }
    let guard = conn(&state.db);
    let buckets = db::get_settings(&guard).map_err(fail)?.peak_buckets.max(32) as usize;
    drop(guard);
    state.scan.running.store(true, Ordering::SeqCst);
    let control = state.scan.clone();
    let db_arc = state.db.clone();
    std::thread::spawn(move || scan::retry_failed(db_arc, app, buckets, control));
    Ok(true)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecoderInfo {
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
}

/// Which optional external decoders were found, for the settings page.
#[tauri::command]
pub fn decoder_info() -> DecoderInfo {
    let (ffmpeg, ffprobe) = crate::external::status();
    DecoderInfo {
        ffmpeg: ffmpeg.map(|p| p.display().to_string()),
        ffprobe: ffprobe.map(|p| p.display().to_string()),
    }
}

/* ----------------------------------------------------------------- tracks */

#[tauri::command]
pub fn query(state: State<AppState>, filter: Filter) -> Cmd<QueryResult> {
    db::query(&conn(&state.db), &filter).map_err(fail)
}

#[tauri::command]
pub fn track_detail(state: State<AppState>, id: i64) -> Cmd<TrackDetail> {
    db::track_detail(&conn(&state.db), id).map_err(fail)
}

#[tauri::command]
pub fn track_peaks(state: State<AppState>, id: i64) -> Cmd<Peaks> {
    db::track_peaks(&conn(&state.db), id).map_err(fail)
}

/// Batch sibling of `track_detail` used after bulk tag/note edits.
#[tauri::command]
pub fn rows_flags(state: State<AppState>, ids: Vec<i64>) -> Cmd<HashMap<i64, RowFlags>> {
    db::row_flags(&conn(&state.db), &ids).map_err(fail)
}

#[tauri::command]
pub fn set_alias(state: State<AppState>, id: i64, alias: String) -> Cmd<()> {
    let snaps = db::snapshot_tracks(&conn(&state.db), &[id]).map_err(fail)?;
    push_undo(&state, UndoEntry::Fields { label: "显示名".into(), snaps });
    db::set_alias(&conn(&state.db), id, &alias).map_err(fail)
}

#[tauri::command]
pub fn set_notes(state: State<AppState>, ids: Vec<i64>, text: String, mode: String) -> Cmd<()> {
    let snaps = db::snapshot_tracks(&conn(&state.db), &ids).map_err(fail)?;
    push_undo(&state, UndoEntry::Fields { label: "备注".into(), snaps });
    db::set_notes(&conn(&state.db), &ids, &text, &mode).map_err(fail)
}

#[tauri::command]
pub fn update_tags(state: State<AppState>, ids: Vec<i64>, add: Vec<i64>, remove: Vec<i64>) -> Cmd<usize> {
    let snaps = db::snapshot_tracks(&conn(&state.db), &ids).map_err(fail)?;
    push_undo(&state, UndoEntry::Fields { label: "标签".into(), snaps });
    db::update_track_tags(&conn(&state.db), &ids, &add, &remove).map_err(fail)
}

#[tauri::command]
pub fn set_rating(state: State<AppState>, ids: Vec<i64>, rating: i64) -> Cmd<()> {
    let snaps = db::snapshot_tracks(&conn(&state.db), &ids).map_err(fail)?;
    push_undo(&state, UndoEntry::Fields { label: "评分".into(), snaps });
    db::set_rating(&conn(&state.db), &ids, rating).map_err(fail)
}

/// Drops library rows only; the audio files are never touched.
#[tauri::command]
pub fn remove_tracks(state: State<AppState>, ids: Vec<i64>) -> Cmd<usize> {
    let rows = db::capture_tracks(&conn(&state.db), &ids).map_err(fail)?;
    push_undo(&state, UndoEntry::Removed { label: "移出库".into(), rows });
    db::remove_tracks(&conn(&state.db), &ids).map_err(fail)
}

#[derive(serde::Serialize)]
pub struct DeleteReport {
    pub deleted: usize,
    pub failed: Vec<String>,
}

/// Sends the files themselves to the Recycle Bin and then drops their rows.
/// Playback stops first so the decoder releases the file handle; a file that is
/// already gone still counts, and failures leave their rows in place.
#[tauri::command]
pub fn delete_files(state: State<AppState>, ids: Vec<i64>) -> Cmd<DeleteReport> {
    if let Some(current) = state.player.status().track_id {
        if ids.contains(&current) {
            state.player.stop();
        }
    }
    let targets: Vec<(i64, PathBuf)> = {
        let guard = conn(&state.db);
        ids.iter()
            .filter_map(|id| db::track_path(&guard, *id).ok().map(|p| (*id, PathBuf::from(p))))
            .collect()
    };
    let mut deleted_ids = Vec::with_capacity(targets.len());
    let mut failed = Vec::new();
    for (id, path) in targets {
        if !path.exists() {
            deleted_ids.push(id);
            continue;
        }
        match trash::delete(&path) {
            Ok(()) => deleted_ids.push(id),
            Err(e) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                failed.push(format!("{name}：{e}"));
            }
        }
    }
    let rows = db::capture_tracks(&conn(&state.db), &deleted_ids).map_err(fail)?;
    if !rows.is_empty() {
        push_undo(&state, UndoEntry::Files { label: "删除文件".into(), rows });
    }
    db::remove_tracks(&conn(&state.db), &deleted_ids).map_err(fail)?;
    Ok(DeleteReport { deleted: deleted_ids.len(), failed })
}

/// The shell's restore verb is the only route back from the Recycle Bin. Verb
/// names are localized, so match the common words and fall back to the first
/// verb, which on Recycle Bin items is always Restore.
fn restore_from_trash(paths: &[String]) -> Cmd<()> {
    use std::os::windows::process::CommandExt;
    if paths.is_empty() {
        return Ok(());
    }
    let mut script = String::from(
        "$ErrorActionPreference = 'Stop'\n\
         $shell = New-Object -ComObject Shell.Application\n\
         $items = @($shell.Namespace(0xA).Items())\n\
         $targets = @(\n",
    );
    for p in paths {
        script.push_str(&format!("'{}'\n", p.replace('\'', "''")));
    }
    script.push_str(
        ")\n\
         foreach ($t in $targets) {\n\
           if (Test-Path -LiteralPath $t) { continue }\n\
           $dir = Split-Path -Parent $t\n\
           $leaf = Split-Path -Leaf $t\n\
           foreach ($item in $items) {\n\
             if ($item.Name -eq $leaf -and $item.ExtendedProperty('System.Recycle.DeletedFrom') -eq $dir) {\n\
               $verbs = @($item.Verbs())\n\
               $pick = $null\n\
               foreach ($v in $verbs) { if ($v.Name -match 'restore|还原|復元|복원|wiederherstellen|restaurer|restaurar|ripristina') { $pick = $v; break } }\n\
               if (-not $pick -and $verbs.Count -gt 0) { $pick = $verbs[0] }\n\
               if ($pick) { $pick.DoIt() }\n\
               break\n\
             }\n\
           }\n\
         }\n",
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x08000000)
        .output()
        .map_err(fail)?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    // DoIt returns before the move finishes; give the files a moment to land.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if paths.iter().all(|p| std::path::Path::new(p).exists()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(())
}

/// Reverses the newest journaled action. A file delete goes back through the
/// Recycle Bin first; rows whose file did not return stay on the stack.
#[tauri::command]
pub fn undo(state: State<AppState>) -> Cmd<String> {
    let entry = {
        let mut stack = state.undo.lock().unwrap_or_else(|p| p.into_inner());
        stack.pop()
    };
    let Some(entry) = entry else { return Err("没有可撤销的操作".into()) };
    match entry {
        UndoEntry::Fields { label, snaps } => {
            let n = db::restore_snapshots(&conn(&state.db), &snaps).map_err(fail)?;
            Ok(format!("{label}（{n} 个条目）"))
        }
        UndoEntry::Removed { label, rows } => {
            let n = db::restore_tracks(&conn(&state.db), &rows).map_err(fail)?;
            Ok(format!("{label}（{n} 个条目）"))
        }
        UndoEntry::Files { label, rows } => {
            let paths: Vec<String> = rows.iter().map(|r| r.path.clone()).collect();
            if let Err(e) = restore_from_trash(&paths) {
                push_undo(&state, UndoEntry::Files { label, rows });
                return Err(e);
            }
            let (back, pending): (Vec<_>, Vec<_>) =
                rows.into_iter().partition(|r| std::path::Path::new(&r.path).exists());
            let n = db::restore_tracks(&conn(&state.db), &back).map_err(fail)?;
            if pending.is_empty() {
                Ok(format!("{label}（{n} 个文件）"))
            } else {
                let left = pending.len();
                push_undo(&state, UndoEntry::Files { label: label.clone(), rows: pending });
                Ok(format!("{label}（还原 {n} 个文件，{left} 个未能还原）"))
            }
        }
        UndoEntry::IgnoredDup { sig } => {
            db::unignore_dup(&conn(&state.db), &sig).map_err(fail)?;
            Ok("不是重复标记".into())
        }
    }
}

#[tauri::command]
pub fn reveal(state: State<AppState>, id: i64) -> Cmd<()> {
    let path = db::track_path(&conn(&state.db), id).map_err(fail)?;
    let native = db::strip_verbatim(&path).replace('/', "\\");
    let target = Path::new(&native);
    if target.is_file() {
        explorer_raw(&format!("/select,\"{native}\""));
        return Ok(());
    }
    // The library can outlive a rename or move. Landing in the containing
    // folder is still useful; landing in Documents is not.
    match target.parent().filter(|p| p.is_dir()) {
        Some(dir) => {
            explorer_raw(&format!("\"{}\"", dir.to_string_lossy()));
            Ok(())
        }
        None => Err("文件已不在原位置".into()),
    }
}

#[tauri::command]
pub fn open_db_folder(state: State<AppState>) -> Cmd<()> {
    let path = db::db_path_of(&conn(&state.db)).ok_or_else(|| "数据库路径未知".to_string())?;
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(path);
    explorer_raw(&format!("\"{}\"", dir.to_string_lossy()));
    Ok(())
}

/// The webview writes this to the clipboard itself, so no clipboard dependency
/// has to ship with the app.
#[tauri::command]
pub fn copy_paths(state: State<AppState>, ids: Vec<i64>) -> Cmd<String> {
    let guard = conn(&state.db);
    let mut out = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Ok(p) = db::track_path(&guard, *id) {
            out.push(p);
        }
    }
    Ok(out.join("\r\n"))
}

/// Explorer parses its own command line rather than using the C runtime rules:
/// `/select,<path>` only highlights the file when the path is quoted, otherwise
/// it silently falls back to Documents or a drive root. `Command::arg` let the
/// std quoting rules escape the switch as one token, so it went wrong for any
/// path holding a space or comma — most of them, once folders have Chinese
/// names. `raw_arg` hands Explorer the exact string it expects.
fn explorer_raw(raw: &str) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("explorer").creation_flags(0x08000000).raw_arg(raw).spawn();
}

/* ------------------------------------------------------------------- tags */

#[tauri::command]
pub fn tags_list(state: State<AppState>) -> Cmd<Vec<Tag>> {
    db::list_tags(&conn(&state.db)).map_err(fail)
}

#[tauri::command]
pub fn tag_create(state: State<AppState>, name: String, color: Option<String>) -> Cmd<Tag> {
    db::create_tag(&conn(&state.db), &name, color.as_deref()).map_err(fail)
}

#[tauri::command]
pub fn tag_update(state: State<AppState>, id: i64, name: Option<String>, color: Option<String>) -> Cmd<()> {
    db::update_tag(&conn(&state.db), id, name.as_deref(), color.as_deref()).map_err(fail)
}

#[tauri::command]
pub fn tag_delete(state: State<AppState>, id: i64) -> Cmd<()> {
    db::delete_tag(&conn(&state.db), id).map_err(fail)
}

/* ------------------------------------------------------------------ views */

#[tauri::command]
pub fn views_list(state: State<AppState>) -> Cmd<Vec<SavedView>> {
    db::list_views(&conn(&state.db)).map_err(fail)
}

#[tauri::command]
pub fn view_save(state: State<AppState>, name: String, filter: Value) -> Cmd<SavedView> {
    let name = name.trim();
    if name.is_empty() {
        return Err("视图需要名字".into());
    }
    db::save_view(&conn(&state.db), name, &filter.to_string()).map_err(fail)
}

#[tauri::command]
pub fn view_update(state: State<AppState>, id: i64, name: String, filter: Value) -> Cmd<SavedView> {
    let name = name.trim();
    if name.is_empty() {
        return Err("视图需要名字".into());
    }
    db::update_view(&conn(&state.db), id, name, &filter.to_string()).map_err(fail)
}

#[tauri::command]
pub fn view_delete(state: State<AppState>, id: i64) -> Cmd<()> {
    db::delete_view(&conn(&state.db), id).map_err(fail)
}

/* ----------------------------------------------------------------- player */

#[tauri::command]
pub fn player_play(state: State<AppState>, id: i64, start_ms: Option<i64>, loop_on: Option<bool>) -> Cmd<PlayerStatus> {
    let (path, duration) = db::track_play_target(&conn(&state.db), id).map_err(fail)?;
    if let Some(on) = loop_on {
        state.player.set_loop(on);
    }
    state.player.play(&path, id, duration).map_err(fail)?;
    if let Some(ms) = start_ms.filter(|m| *m > 0) {
        state.player.seek(ms).map_err(fail)?;
    }
    let _ = db::note_play(&conn(&state.db), id);
    Ok(state.player.status())
}

/// Check-listen from the duplicate dialog. Deliberately skips `note_play`: an
/// audition is not a real play and must not pad the most-played ranking.
#[tauri::command]
pub fn player_preview(state: State<AppState>, id: i64) -> Cmd<PlayerStatus> {
    let (path, duration) = db::track_play_target(&conn(&state.db), id).map_err(fail)?;
    state.player.play(&path, id, duration).map_err(fail)?;
    Ok(state.player.status())
}

#[tauri::command]
pub fn player_toggle(state: State<AppState>) -> Cmd<bool> {
    Ok(state.player.toggle())
}

#[tauri::command]
pub fn player_stop(state: State<AppState>) -> Cmd<()> {
    state.player.stop();
    Ok(())
}

#[tauri::command]
pub fn player_seek(state: State<AppState>, ms: i64) -> Cmd<()> {
    state.player.seek(ms).map_err(fail)
}

#[tauri::command]
pub fn player_set_volume(state: State<AppState>, value: f32) -> Cmd<f32> {
    state.player.set_volume(value);
    let _ = db::set_setting(&conn(&state.db), "volume", &value.clamp(0.0, 1.0).to_string());
    Ok(state.player.status().volume)
}

#[tauri::command]
pub fn player_set_loop(state: State<AppState>, on: bool) -> Cmd<bool> {
    state.player.set_loop(on);
    Ok(on)
}

#[tauri::command]
pub fn player_status(state: State<AppState>) -> Cmd<PlayerStatus> {
    Ok(state.player.status())
}

/* --------------------------------------------------------------- settings */

const SETTING_KEYS: [&str; 7] =
    ["genPeaks", "peakBuckets", "theme", "scanHidden", "minSizeBytes", "volume", "autoUpdate"];

#[tauri::command]
pub fn settings(state: State<AppState>, patch: Option<HashMap<String, Value>>) -> Cmd<Settings> {
    let guard = conn(&state.db);
    for (key, value) in patch.unwrap_or_default() {
        if !SETTING_KEYS.contains(&key.as_str()) {
            continue;
        }
        let text = match value {
            Value::Bool(b) => b.to_string(),
            Value::String(s) => s,
            other => other.to_string(),
        };
        db::set_setting(&guard, &key, &text).map_err(fail)?;
    }
    db::get_settings(&guard).map_err(fail)
}

#[tauri::command]
pub fn app_info(state: State<AppState>) -> Cmd<AppInfo> {
    let guard = conn(&state.db);
    let db_path = db::db_path_of(&guard).map(|p| p.display().to_string()).unwrap_or_default();
    Ok(AppInfo {
        db_path,
        version: env!("CARGO_PKG_VERSION").to_string(),
        product: "sfxer".into(),
        repo: update::REPO.into(),
        repo_url: update::repo_url(),
        audio_extensions: scan::AUDIO_EXTS.iter().map(|s| s.to_string()).collect(),
    })
}

/* ------------------------------------------------------------------ updates */

#[tauri::command]
pub async fn check_update() -> Cmd<update::Release> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    tauri::async_runtime::spawn_blocking(move || update::latest(&current))
        .await
        .map_err(fail)?
        .map_err(fail)
}

#[tauri::command]
pub fn open_link(url: String) -> Cmd<()> {
    // The only links the UI offers are GitHub ones, and the release address
    // arrives from the network, so nothing else may reach the shell.
    if !url.starts_with("https://github.com/") {
        return Err("只能打开 GitHub 链接".into());
    }
    use std::os::windows::process::CommandExt;
    let _ = Command::new("explorer").creation_flags(0x08000000).arg(&url).spawn();
    Ok(())
}

/* ----------------------------------------------------------- manifest i/o */

#[tauri::command]
pub fn export_manifest(state: State<AppState>, path: String) -> Cmd<ExportResult> {
    let rows = db::export_rows(&conn(&state.db)).map_err(fail)?;
    let count = rows.len() as i64;
    let doc = serde_json::json!({
        "app": "sfxer",
        "version": 1,
        "exportedAt": chrono::Local::now().timestamp(),
        "tracks": rows,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(fail)?;
    std::fs::write(&path, text).map_err(fail)?;
    Ok(ExportResult { count, path })
}

#[tauri::command]
pub fn import_manifest(state: State<AppState>, path: String) -> Cmd<ImportResult> {
    let text = std::fs::read_to_string(path).map_err(fail)?;
    let doc: Value =
        serde_json::from_str(&text).map_err(|e| format!("备份文件无法解析：{e}"))?;
    let entries: Vec<ManifestEntry> = doc
        .get("tracks")
        .cloned()
        .and_then(|t| serde_json::from_value(t).ok())
        .unwrap_or_default();
    if entries.is_empty() {
        return Err("备份文件里没有可导入的记录".into());
    }
    let mut guard = conn(&state.db);
    let tx = guard.transaction().map_err(fail)?;
    let (mut updated, mut skipped) = (0i64, 0i64);
    for entry in &entries {
        if db::apply_manifest_entry(&tx, entry).map_err(fail)? {
            updated += 1;
        } else {
            skipped += 1;
        }
    }
    tx.commit().map_err(fail)?;
    Ok(ImportResult { updated, skipped })
}
