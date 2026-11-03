//! Headless checks for the parts the GUI cannot be exercised with: metadata
//! probing, waveform peaks, the query builder, and the promise that indexing
//! never writes into a user's audio file.
//!
//! Needs `test-library/` (regenerate with `node tools/make-test-audio.mjs`);
//! without it the probe cases report themselves as skipped.
use std::path::{Path, PathBuf};

use crate::db::{self, NewTrack};
use crate::model::{Filter, ManifestEntry};
use crate::probe;
use crate::scan;

fn library() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|root| root.join("test-library"))
        .expect("CARGO_MANIFEST_DIR has a parent")
}

/// (relative path, sample rate, channels, bit depth, expected duration in ms)
const WAVS: &[(&str, i64, i64, i64, i64)] = &[
    ("ui/click-soft.wav", 44100, 1, 16, 120),
    ("ui/click-hard.wav", 48000, 1, 24, 90),
    ("foley/footstep-gravel.wav", 44100, 2, 16, 600),
    ("foley/door-close.wav", 44100, 2, 16, 1400),
    ("ambience/rain-loop.wav", 22050, 2, 16, 4000),
    ("tones/sine-440.wav", 48000, 2, 32, 2500),
    ("tones/silence.wav", 44100, 1, 16, 1000),
    ("tones/clipped-square.wav", 44100, 1, 16, 500),
];

/// Size and mtime of every file under the library, to prove indexing is read-only.
fn fingerprint(dir: &Path) -> Vec<(String, u64, u64)> {
    let mut list = Vec::new();
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let meta = entry.metadata().expect("readable file");
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        list.push((entry.path().to_string_lossy().to_string(), meta.len(), mtime));
    }
    list.sort();
    list
}

/// The generated fixtures live outside the crate, so a fresh clone has none.
fn skip_if_no_library(test: &str) -> bool {
    let lib = library();
    if lib.is_dir() {
        false
    } else {
        eprintln!("{} skipped: run `node tools/make-test-audio.mjs` to create {}", test, lib.display());
        true
    }
}

#[test]
fn reads_container_properties_of_every_generated_wav() {
    if skip_if_no_library("reads_container_properties") {
        return;
    }
    let lib = library();
    for (rel, rate, channels, bits, duration_ms) in WAVS {
        let path = lib.join(rel);
        let meta = probe::read_meta(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(meta.sample_rate, Some(*rate), "{rel} sample rate");
        assert_eq!(meta.channels, Some(*channels), "{rel} channels");
        assert_eq!(meta.bit_depth, Some(*bits), "{rel} bit depth");
        let actual = meta.duration_ms.unwrap_or(-1);
        assert!((actual - duration_ms).abs() <= 30, "{rel}: {actual} ms, expected {duration_ms} ms");
        assert_eq!(meta.container.as_deref(), Some("Wav"), "{rel} container");
    }
}

#[test]
fn decodes_peaks_and_detects_silence_and_clipping() {
    if skip_if_no_library("decodes_peaks") {
        return;
    }
    let lib = library();
    for (rel, _, _, _, _) in WAVS {
        let wave = probe::read_wave(&lib.join(rel), 256).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(wave.peaks.len(), 512, "{rel} is one (min, max) pair per bucket");
        assert!(wave.peak_db <= 0.1, "{rel} peaks above full scale: {}", wave.peak_db);
    }

    let silence = probe::read_wave(&lib.join("tones/silence.wav"), 256).expect("silence still decodes");
    assert_eq!(silence.peak_db, -99.0, "digital silence floors at the sentinel value");
    assert!(!silence.clipped);

    let loud = probe::read_wave(&lib.join("tones/clipped-square.wav"), 256).expect("square decodes");
    assert!(loud.clipped, "a saturated square wave must be flagged");
    assert!(loud.peak_db > -0.5, "clipped peak db: {}", loud.peak_db);
    assert!(loud.rms_db > silence.rms_db);
}

#[test]
fn unreadable_files_are_reported_instead_of_panicking() {
    if skip_if_no_library("unreadable_files") {
        return;
    }
    let lib = library();
    for rel in ["bad/not-really-mp3.mp3", "bad/empty.wav"] {
        let path = lib.join(rel);
        assert!(probe::read_meta(&path).is_err(), "{rel} should not probe as audio");
        assert!(probe::read_wave(&path, 128).is_err(), "{rel} should not decode");
    }
}

#[test]
fn extension_filter_picks_audio_only() {
    assert!(scan::is_audio_name("Door.CLOSE.WAV"));
    assert!(scan::is_audio_name("loop.flac"));
    assert!(scan::is_audio_name("voice.opus"));
    assert!(scan::is_audio_name("game.wma"));
    assert!(!scan::is_audio_name("notes.txt"));
    assert!(!scan::is_audio_name("noextension"));
    assert!(!scan::is_audio_name("trailing-dot."));
}

fn temp_db(name: &str) -> db::SharedDb {
    let path = std::env::temp_dir().join(format!("sfx-manager-test-{name}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    db::open(&path).expect("open a temporary library")
}

fn track_of(path: &Path) -> NewTrack {
    let meta = probe::read_meta(path).expect("probe");
    let wave = probe::read_wave(path, 128).ok();
    let file = path.metadata().expect("file metadata");
    NewTrack {
        path: path.to_string_lossy().to_string(),
        name: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        ext: path.extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default(),
        dir: path.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
        root_id: None,
        size_bytes: file.len() as i64,
        mtime: 1_700_000_000,
        duration_ms: meta.duration_ms,
        sample_rate: meta.sample_rate,
        bit_depth: meta.bit_depth,
        channels: meta.channels,
        bitrate: meta.bitrate,
        codec: meta.codec,
        container: meta.container,
        peak_db: wave.as_ref().map(|w| w.peak_db),
        rms_db: wave.as_ref().map(|w| w.rms_db),
        decodeable: wave.is_some(),
        clipped: wave.as_ref().map(|w| w.clipped).unwrap_or(false),
        peaks: wave.as_ref().map(|w| db::peaks_to_bytes(&w.peaks)),
    }
}

fn total(conn: &rusqlite::Connection, text: &str) -> i64 {
    let filter = Filter { text: text.into(), ..Default::default() };
    db::query(conn, &filter).unwrap_or_else(|e| panic!("search {text:?}: {e}")).total
}

#[test]
fn an_empty_library_reports_zeroes_rather_than_failing() {
    // The first launch has no rows at all; every aggregate then reads back as
    // NULL, which the GUI showed as a startup error once.
    let db = temp_db("empty");
    let conn = db.lock().unwrap();
    let stats = db::stats(&conn).expect("stats on an empty library");
    assert_eq!(stats.track_count, 0);
    assert_eq!(
        (stats.untagged, stats.notes, stats.recent, stats.clipped, stats.undecodable),
        (0, 0, 0, 0, 0)
    );
    assert_eq!((stats.total_duration_ms, stats.total_size_bytes), (0, 0));
    assert!(stats.formats.is_empty() && stats.sample_rates.is_empty() && stats.durations.is_empty());
    assert_eq!(db::query(&conn, &Filter::default()).unwrap().total, 0);
    assert!(db::list_tags(&conn).unwrap().is_empty());
    assert!(db::export_rows(&conn).unwrap().is_empty());
    assert_eq!(db::get_settings(&conn).unwrap().peak_buckets, 512, "defaults are seeded");
}

#[test]
fn library_search_tags_and_scopes() {
    if skip_if_no_library("library_search") {
        return;
    }
    let lib = library();
    let before = fingerprint(&lib);
    let db = temp_db("query");
    let conn = db.lock().unwrap();
    let root = db::add_root(&conn, &lib.to_string_lossy()).expect("add root");
    assert!(root.path.to_lowercase().ends_with("test-library"), "root path: {}", root.path);

    for rel in ["ui/click-soft.wav", "foley/door-close.wav", "tones/clipped-square.wav"] {
        let mut track = track_of(&lib.join(rel));
        track.root_id = Some(root.id);
        db::upsert_track(&conn, &track, false).expect("index");
    }

    let all = db::query(&conn, &Filter::default()).expect("query all");
    assert_eq!(all.total, 3);
    let door = all.items.iter().find(|r| r.name == "door-close").expect("door row").id;

    // One term per whitespace-separated word, matched AND-wise against the
    // file name, its folder, alias, notes and tag names.
    assert_eq!(total(&conn, "door"), 1);
    assert_eq!(total(&conn, "foley"), 1);
    assert_eq!(total(&conn, "click"), 1);
    assert_eq!(total(&conn, "TEST-LIBRARY foley"), 1, "terms are case folded");
    assert_eq!(total(&conn, "foley click"), 0, "terms are ANDed");
    assert_eq!(total(&conn, "门"), 0);

    db::set_notes(&conn, &[door], "关门，带回响", "replace").unwrap();
    db::set_alias(&conn, door, "木门关闭").unwrap();
    assert_eq!(total(&conn, "关门"), 1);
    assert_eq!(total(&conn, "木门"), 1);
    assert_eq!(total(&conn, "回响"), 1);
    assert_eq!(total(&conn, "关门 foley"), 1, "CJK and ascii in one search");
    assert_eq!(total(&conn, "关门 tone"), 0, "still ANDed");

    let short = db::create_tag(&conn, "短音", None).unwrap();
    let metal = db::create_tag(&conn, "金属", None).unwrap();
    db::update_track_tags(&conn, &[door], &[short.id], &[]).unwrap();
    let others: Vec<i64> = all.items.iter().map(|r| r.id).filter(|id| *id != door).collect();
    db::update_track_tags(&conn, &others, &[metal.id], &[]).unwrap();

    let any = Filter { tag_ids: vec![short.id, metal.id], tag_match: "any".into(), ..Default::default() };
    let both = Filter { tag_ids: vec![short.id, metal.id], tag_match: "all".into(), ..Default::default() };
    assert_eq!(db::query(&conn, &any).unwrap().total, 3);
    assert_eq!(db::query(&conn, &both).unwrap().total, 0);
    assert_eq!(db::query(&conn, &Filter { scope: "untagged".into(), ..Default::default() }).unwrap().total, 0);
    assert_eq!(db::query(&conn, &Filter { scope: "notes".into(), ..Default::default() }).unwrap().total, 1);
    assert_eq!(db::query(&conn, &Filter { scope: "clipped".into(), ..Default::default() }).unwrap().total, 1);
    assert_eq!(db::query(&conn, &Filter { scope: "root".into(), root_id: Some(root.id), ..Default::default() })
        .unwrap()
        .total,
        3);

    // Renaming a tag must not leave the old spelling in the search column.
    db::update_tag(&conn, metal.id, Some("铁质"), None).unwrap();
    assert_eq!(total(&conn, "金属"), 0);
    assert_eq!(total(&conn, "铁质"), 2);
    assert_eq!(db::query(&conn, &Filter::default()).unwrap().items[0].tag_ids.len(), 1, "tags ride along the row");

    let stats = db::stats(&conn).unwrap();
    assert_eq!((stats.track_count, stats.root_count, stats.tag_count), (3, 1, 2));
    assert_eq!((stats.clipped, stats.undecodable, stats.untagged, stats.notes), (1, 0, 0, 1));
    assert!(stats.total_size_bytes > 0);
    assert!(stats.formats.iter().any(|f| f.value == "wav"));
    assert_eq!(db::list_tags(&conn).unwrap()[0].use_count, 2, "most used tag first");

    let detail = db::track_detail(&conn, door).unwrap();
    assert_eq!(detail.notes, "关门，带回响");
    assert_eq!(detail.alias.as_deref(), Some("木门关闭"));
    assert_eq!((detail.sample_rate, detail.channels), (Some(44100), Some(2)));
    assert!(detail.decodeable);

    // Dropping a root takes its rows with it (FK cascade), and nothing on disk.
    db::remove_root(&conn, root.id).unwrap();
    assert_eq!(db::query(&conn, &Filter::default()).unwrap().total, 0);
    assert_eq!(db::list_tags(&conn).unwrap().len(), 2, "the tag vocabulary outlives a removed root");
    drop(conn);
    drop(db);
    assert_eq!(fingerprint(&lib), before, "indexing must not touch the audio files");
}

#[test]
fn peaks_roundtrip_through_the_blob_column() {
    if skip_if_no_library("peaks_roundtrip") {
        return;
    }
    let db = temp_db("peaks");
    let conn = db.lock().unwrap();
    db::upsert_track(&conn, &track_of(&library().join("foley/door-close.wav")), false).unwrap();
    let id = db::query(&conn, &Filter::default()).unwrap().items[0].id;
    let peaks = db::track_peaks(&conn, id).unwrap();
    assert_eq!((peaks.buckets, peaks.values.len()), (128, 256));
    assert!(peaks.values.iter().all(|v| v.abs() <= 1.0), "normalised to full scale");
    assert!(peaks.values.iter().any(|v| *v < 0.0) && peaks.values.iter().any(|v| *v > 0.0));
}

#[test]
fn manifest_export_restores_onto_a_fresh_database() {
    if skip_if_no_library("manifest_export") {
        return;
    }
    let path = library().join("ui/click-soft.wav");
    let db = temp_db("manifest");
    let conn = db.lock().unwrap();
    db::upsert_track(&conn, &track_of(&path), false).unwrap();
    let id = db::find_track_by_path(&conn, &path.to_string_lossy()).unwrap().expect("row");
    db::set_alias(&conn, id, "轻点击").unwrap();
    db::set_notes(&conn, &[id], "下次换个更短的", "append").unwrap();
    let tag = db::create_tag(&conn, "UI", None).unwrap();
    db::update_track_tags(&conn, &[id], &[tag.id], &[]).unwrap();

    let rows = db::export_rows(&conn).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tags, vec!["UI".to_string()], "tags travel by name, not by id");
    assert!(rows[0].notes.contains("下次换个更短的"));

    // A second library that indexed the same file with unrelated ids.
    let other_db = temp_db("manifest-restore");
    let other = other_db.lock().unwrap();
    assert!(db::upsert_track(&other, &track_of(&path), false).unwrap());
    assert!(db::apply_manifest_entry(&other, &rows[0]).unwrap());
    let restored = db::query(&other, &Filter { text: "轻点击".into(), ..Default::default() }).unwrap();
    assert_eq!(restored.total, 1);
    assert_eq!(db::track_detail(&other, restored.items[0].id).unwrap().tag_ids.len(), 1);
    assert_eq!(db::list_tags(&other).unwrap().len(), 1);

    let missing = ManifestEntry {
        path: library().join("no-such-file.wav").to_string_lossy().to_string(),
        alias: None,
        notes: "orphan".into(),
        tags: vec![],
    };
    assert!(!db::apply_manifest_entry(&other, &missing).unwrap(), "an unknown path is skipped, never inserted");
}

fn placeholder_track() -> NewTrack {
    NewTrack {
        path: "D:/nowhere/bloop.wav".into(),
        name: "bloop".into(),
        ext: "wav".into(),
        dir: "D:/nowhere".into(),
        root_id: None,
        size_bytes: 1024,
        mtime: 1_700_000_000,
        duration_ms: Some(100),
        sample_rate: Some(44100),
        bit_depth: Some(16),
        channels: Some(1),
        bitrate: Some(1_411_000),
        codec: Some("PCM / ADPCM (WAV)".into()),
        container: Some("Wav".into()),
        peak_db: Some(-3.0),
        rms_db: Some(-12.0),
        decodeable: true,
        clipped: false,
        peaks: Some(db::peaks_to_bytes(&[-0.5, 0.5, -0.2, 0.2])),
    }
}

#[test]
fn second_index_of_an_untouched_file_writes_nothing() {
    let db = temp_db("idempotent");
    let conn = db.lock().unwrap();
    let mut track = placeholder_track();
    assert!(db::upsert_track(&conn, &track, false).unwrap(), "the first row is new");
    assert!(!db::upsert_track(&conn, &track, false).unwrap(), "same size and mtime is skipped");
    track.size_bytes += 1;
    assert!(!db::upsert_track(&conn, &track, false).unwrap(), "a changed file updates in place");
    let rows = db::query(&conn, &Filter::default()).unwrap();
    assert_eq!((rows.total, rows.items[0].size_bytes), (1, 1025));
}

#[test]
fn an_overlapping_root_does_not_take_ownership_of_a_file() {
    let base = std::env::temp_dir().join("sfx-manager-test-overlap");
    let outer_dir = base.join("outer");
    let inner_dir = outer_dir.join("deep");
    std::fs::create_dir_all(&inner_dir).expect("scratch roots");
    let db = temp_db("overlap");
    let conn = db.lock().unwrap();
    let outer = db::add_root(&conn, &outer_dir.to_string_lossy()).unwrap();
    let inner = db::add_root(&conn, &inner_dir.to_string_lossy()).unwrap();

    let mut track = placeholder_track();
    track.path = outer_dir.join("bloop.wav").to_string_lossy().to_string();
    track.dir = outer_dir.to_string_lossy().to_string();
    track.root_id = Some(outer.id);
    db::upsert_track(&conn, &track, false).unwrap();

    // The inner root walks over the same file and reports it as its own.
    track.root_id = Some(inner.id);
    track.size_bytes += 1;
    db::upsert_track(&conn, &track, false).unwrap();

    let roots = db::list_roots(&conn).unwrap();
    let count = |id: i64| roots.iter().find(|r| r.id == id).map(|r| r.count).unwrap_or(-1);
    assert_eq!((count(outer.id), count(inner.id)), (1, 0), "the first root keeps the row");
    db::remove_root(&conn, inner.id).unwrap();
    assert_eq!(db::query(&conn, &Filter::default()).unwrap().total, 1, "dropping the inner root keeps it");
    drop(conn);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn removing_a_track_untags_it_but_keeps_the_tag() {
    let db = temp_db("remove");
    let conn = db.lock().unwrap();
    db::upsert_track(&conn, &placeholder_track(), false).unwrap();
    let id = db::query(&conn, &Filter::default()).unwrap().items[0].id;
    let tag = db::create_tag(&conn, "待删", None).unwrap();
    db::update_track_tags(&conn, &[id], &[tag.id], &[]).unwrap();

    assert_eq!(db::remove_tracks(&conn, &[id]).unwrap(), 1);
    assert_eq!(db::remove_tracks(&conn, &[999_999]).unwrap(), 0, "an unknown id removes nothing");
    assert!(db::track_detail(&conn, id).is_err());
    assert_eq!(db::query(&conn, &Filter::default()).unwrap().total, 0);
    assert_eq!(db::list_tags(&conn).unwrap()[0].use_count, 0, "the link cascaded, the tag stays defined");

    // Untagging the last user of a tag drops the tag itself.
    db::upsert_track(&conn, &placeholder_track(), true).unwrap();
    let again = db::query(&conn, &Filter::default()).unwrap().items[0].id;
    db::update_track_tags(&conn, &[again], &[tag.id], &[]).unwrap();
    db::update_track_tags(&conn, &[again], &[], &[tag.id]).unwrap();
    assert!(db::list_tags(&conn).unwrap().is_empty());
}

#[test]
fn row_flags_describe_every_requested_track_in_one_query() {
    let db = temp_db("flags");
    let conn = db.lock().unwrap();
    let ids: Vec<i64> = (0..3)
        .map(|i| {
            let mut t = placeholder_track();
            t.path = format!("D:/nowhere/bloop-{i}.wav");
            t.name = format!("bloop-{i}");
            t.mtime = 1_700_000_000 + i;
            db::upsert_track(&conn, &t, false).unwrap();
            db::find_track_by_path(&conn, &t.path).unwrap().expect("row")
        })
        .collect();

    let tag = db::create_tag(&conn, "批量", None).unwrap();
    db::update_track_tags(&conn, &ids[0..2], &[tag.id], &[]).unwrap();
    db::set_notes(&conn, &[ids[1]], "  ", "replace").unwrap();
    db::set_notes(&conn, &[ids[0]], "金属撞击", "replace").unwrap();

    let flags = db::row_flags(&conn, &ids).unwrap();
    assert_eq!(flags.len(), ids.len(), "untagged rows are described too, so stale chips clear");
    assert_eq!(flags[&ids[0]].tag_ids, vec![tag.id]);
    assert!(flags[&ids[0]].has_notes);
    assert_eq!(flags[&ids[1]].tag_ids, vec![tag.id], "both rows of the slice were tagged");
    assert!(!flags[&ids[1]].has_notes, "whitespace is not a note");
    assert!(flags[&ids[2]].tag_ids.is_empty() && !flags[&ids[2]].has_notes, "never touched at all");
    assert!(db::row_flags(&conn, &[]).unwrap().is_empty(), "an empty selection asks nothing");
}

#[test]
fn facet_filters_match_the_values_the_sidebar_sends() {
    if skip_if_no_library("facet_filters") {
        return;
    }
    let lib = library();
    let db = temp_db("facets");
    let conn = db.lock().unwrap();
    let root = db::add_root(&conn, &lib.to_string_lossy()).expect("add root");
    for (rel, _, _, _, _) in WAVS {
        let mut track = track_of(&lib.join(rel));
        track.root_id = Some(root.id);
        db::upsert_track(&conn, &track, false).expect("index");
    }
    let stats = db::stats(&conn).unwrap();

    // The sidebar feeds each facet value straight back into the next query, so a
    // text facet must bind as text: a stray cast to integer makes it `t.ext = 0`
    // and the whole filter group silently matches nothing.
    let formats: Vec<String> = stats.formats.iter().map(|f| f.value.clone()).collect();
    let rates: Vec<i64> = stats.sample_rates.iter().map(|f| f.value.parse().unwrap()).collect();
    let durations: Vec<String> = stats.durations.iter().map(|f| f.value.clone()).collect();
    assert_eq!(db::query(&conn, &Filter { formats: formats.clone(), ..Default::default() }).unwrap().total, 8, "the ext facet round-trips");
    assert_eq!(db::query(&conn, &Filter { formats: vec!["flac".into()], ..Default::default() }).unwrap().total, 0);
    assert_eq!(db::query(&conn, &Filter { sample_rates: rates, ..Default::default() }).unwrap().total, 8);
    assert_eq!(db::query(&conn, &Filter { sample_rates: vec![44100], ..Default::default() }).unwrap().total, 5);
    assert_eq!(db::query(&conn, &Filter { channels: vec![1], ..Default::default() }).unwrap().total, 4);
    assert_eq!(db::query(&conn, &Filter { durations: vec!["lt1".into()], ..Default::default() }).unwrap().total, 4);
    assert_eq!(durations.len(), 3, "the fixture spans three duration buckets");
    let both = db::query(&conn, &Filter { channels: vec![2], durations: vec!["3to10".into()], ..Default::default() }).unwrap();
    assert_eq!(both.total, 1, "groups AND together: only rain-loop is stereo and 3-10s");
}

#[test]
fn a_bulk_edit_over_hundreds_of_rows_stays_fast() {
    let db = temp_db("bulk-speed");
    let conn = db.lock().unwrap();
    let ids: Vec<i64> = (0..400)
        .map(|i| {
            let mut t = placeholder_track();
            t.path = format!("D:/nowhere/bulk-{i}.wav");
            t.name = format!("bulk-{i}");
            t.mtime = 1_700_000_000 + i;
            db::upsert_track(&conn, &t, false).unwrap();
            db::find_track_by_path(&conn, &t.path).unwrap().expect("row")
        })
        .collect();
    let tag = db::create_tag(&conn, "批量", None).unwrap();

    let started = std::time::Instant::now();
    db::update_track_tags(&conn, &ids, &[tag.id], &[]).unwrap();
    db::set_notes(&conn, &ids, "金属撞击", "replace").unwrap();
    let elapsed = started.elapsed();
    println!("{} rows tagged and noted in {elapsed:?}", ids.len());
    assert!(elapsed < std::time::Duration::from_secs(2), "one bulk edit must not become 400 commits");

    let flags = db::row_flags(&conn, &ids).unwrap();
    assert_eq!(flags.len(), ids.len());
    assert!(flags.values().all(|f| f.has_notes && f.tag_ids == vec![tag.id]));
    let stats = db::stats(&conn).unwrap();
    assert_eq!((stats.track_count, stats.notes, stats.untagged), (400, 400, 0));
}

#[test]
fn saved_views_round_trip_and_carry_a_live_count() {
    let db = temp_db("views");
    let conn = db.lock().unwrap();
    for (name, path) in [("metal-hit", "D:/nowhere/metal-hit.wav"), ("soft-pad", "D:/nowhere/soft-pad.wav")] {
        let mut track = placeholder_track();
        track.name = name.into();
        track.path = path.into();
        db::upsert_track(&conn, &track, false).expect("index");
    }

    let metal = serde_json::json!({ "text": "metal" }).to_string();
    let view = db::save_view(&conn, "金属", &metal).expect("save");
    assert_eq!(view.count, 1, "a view reports how much it matches right now");

    // The name is the identity: saving it again replaces the filter instead of
    // piling up a second near-duplicate.
    db::save_view(&conn, "金属", &serde_json::json!({ "text": "soft" }).to_string()).expect("replace");
    let all = db::list_views(&conn).expect("list");
    assert_eq!(all.len(), 1, "re-saving a name does not add a row");
    assert_eq!(all[0].count, 1);

    db::save_view(&conn, "柔和", "{}").expect("second view");
    assert!(
        db::update_view(&conn, all[0].id, "柔和", "{}").is_err(),
        "renaming onto an existing name is refused, not a constraint crash"
    );
    let renamed = db::update_view(&conn, all[0].id, "改名后", &metal).expect("rename");
    assert_eq!(renamed.count, 1, "the count survives a rename");

    db::delete_view(&conn, all[0].id).expect("delete");
    assert_eq!(db::list_views(&conn).expect("list after delete").len(), 1);
}

#[test]
fn version_compare_orders_release_tags_the_way_the_update_prompt_needs() {
    use std::cmp::Ordering::*;
    use crate::update::compare;

    // Numeric components, not string order: "0.10.0" must beat "0.9.0".
    assert_eq!(compare("0.10.0", "0.9.0"), Greater);
    assert_eq!(compare("0.9.0", "0.9.0"), Equal);
    assert_eq!(compare("v0.9.1", "0.9.0"), Greater, "a leading v is cosmetic");
    assert_eq!(compare("0.9", "0.9.0"), Equal, "a missing patch is zero, not less");
    // A pre-release is a preview *of* its version, so it sorts below the release.
    assert_eq!(compare("1.0.0-beta", "1.0.0"), Less);
    assert_eq!(compare("1.0.0-beta", "0.9.9"), Greater);
}
