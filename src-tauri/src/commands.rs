//! The command surface the webview calls. Every statement lives in `db.rs`;
//! this file only translates between the two sides and owns the threads.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};

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
}

impl AppState {
    pub fn new(db: SharedDb, player: PlayerCtl) -> Self {
        let player = Arc::new(player);
        let volume = db::get_settings(&conn(&db)).map(|s| s.volume).unwrap_or(0.7);
        player.set_volume(volume);
        Self { db, player, scan: ScanControl::default() }
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
        scan::run(db, app, targets, force, settings, control);
    });
    Ok(true)
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
    db::set_alias(&conn(&state.db), id, &alias).map_err(fail)
}

#[tauri::command]
pub fn set_notes(state: State<AppState>, ids: Vec<i64>, text: String, mode: String) -> Cmd<()> {
    db::set_notes(&conn(&state.db), &ids, &text, &mode).map_err(fail)
}

#[tauri::command]
pub fn update_tags(state: State<AppState>, ids: Vec<i64>, add: Vec<i64>, remove: Vec<i64>) -> Cmd<usize> {
    db::update_track_tags(&conn(&state.db), &ids, &add, &remove).map_err(fail)
}

#[tauri::command]
pub fn set_rating(state: State<AppState>, ids: Vec<i64>, rating: i64) -> Cmd<()> {
    db::set_rating(&conn(&state.db), &ids, rating).map_err(fail)
}

/// Drops library rows only; the audio files are never touched.
#[tauri::command]
pub fn remove_tracks(state: State<AppState>, ids: Vec<i64>) -> Cmd<usize> {
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
    db::remove_tracks(&conn(&state.db), &deleted_ids).map_err(fail)?;
    Ok(DeleteReport { deleted: deleted_ids.len(), failed })
}

#[tauri::command]
pub fn reveal(state: State<AppState>, id: i64) -> Cmd<()> {
    let path = db::track_path(&conn(&state.db), id).map_err(fail)?;
    open_in_explorer(&format!("/select,{path}"));
    Ok(())
}

#[tauri::command]
pub fn open_db_folder(state: State<AppState>) -> Cmd<()> {
    let path = db::db_path_of(&conn(&state.db)).ok_or_else(|| "数据库路径未知".to_string())?;
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(path);
    open_in_explorer(&dir.to_string_lossy());
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

fn open_in_explorer(arg: &str) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("explorer").creation_flags(0x08000000).arg(arg).spawn();
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
    open_in_explorer(&url);
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
