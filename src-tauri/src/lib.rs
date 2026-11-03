mod commands;
mod db;
mod error;
mod model;
mod player;
mod probe;
mod scan;
mod update;
#[cfg(test)]
mod tests;

use std::sync::atomic::Ordering;

use tauri::{Manager, RunEvent};

use crate::commands::AppState;
use crate::player::PlayerCtl;

/// The library lived under the pre-rename identifier; copying it across once
/// keeps an existing set of tags, notes and display names from looking wiped.
fn migrate_legacy_db(target: &std::path::Path) {
    if target.exists() {
        return;
    }
    let Some(appdata) = std::env::var_os("APPDATA") else { return };
    let legacy_dir = std::path::Path::new(&appdata).join("dev.zara.sfxmanager");
    if !legacy_dir.join("sfx.db").is_file() {
        return;
    }
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // The write-ahead log goes with it: after an unclean exit the newest
    // transactions live only there.
    for suffix in ["", "-wal"] {
        let from = legacy_dir.join(format!("sfx.db{suffix}"));
        if from.is_file() {
            let _ = std::fs::copy(&from, target.with_file_name(format!("sfx.db{suffix}")));
        }
    }
}

pub fn run() {
    let builder = tauri::Builder::default()
        .setup(|app| {
            let db_path = app.path().resolve("sfx.db", tauri::path::BaseDirectory::AppData)?;
            if let Some(dir) = db_path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            migrate_legacy_db(&db_path);
            let library = db::open(&db_path)?;
            app.manage(AppState::new(library, PlayerCtl::new()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pick_root,
            commands::pick_save_json,
            commands::pick_open_json,
            commands::list_roots,
            commands::add_root,
            commands::remove_root,
            commands::scan,
            commands::abort_scan,
            commands::stats,
            commands::rebuild_peaks,
            commands::query,
            commands::track_detail,
            commands::track_peaks,
            commands::rows_flags,
            commands::set_alias,
            commands::set_notes,
            commands::update_tags,
            commands::remove_tracks,
            commands::reveal,
            commands::open_db_folder,
            commands::copy_paths,
            commands::tags_list,
            commands::tag_create,
            commands::tag_update,
            commands::tag_delete,
            commands::views_list,
            commands::view_save,
            commands::view_update,
            commands::view_delete,
            commands::player_play,
            commands::player_toggle,
            commands::player_stop,
            commands::player_seek,
            commands::player_set_volume,
            commands::player_set_loop,
            commands::player_status,
            commands::settings,
            commands::app_info,
            commands::check_update,
            commands::open_link,
            commands::export_manifest,
            commands::import_manifest
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("sfxer 无法启动");

    app.run(|handle, event| {
        if matches!(event, RunEvent::ExitRequested { .. }) {
            handle.state::<AppState>().scan.abort.store(true, Ordering::SeqCst);
        }
    });
}
