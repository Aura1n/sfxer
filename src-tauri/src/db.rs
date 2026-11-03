use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};
use crate::model::*;

pub type SharedDb = Arc<Mutex<Connection>>;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings(
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS roots(
  id           INTEGER PRIMARY KEY,
  path         TEXT NOT NULL UNIQUE COLLATE NOCASE,
  label        TEXT NOT NULL,
  added_at     INTEGER NOT NULL,
  last_scan_at INTEGER
);
CREATE TABLE IF NOT EXISTS tracks(
  id             INTEGER PRIMARY KEY,
  root_id        INTEGER REFERENCES roots(id) ON DELETE CASCADE,
  path           TEXT NOT NULL UNIQUE COLLATE NOCASE,
  name           TEXT NOT NULL,
  alias          TEXT,
  ext            TEXT NOT NULL,
  dir            TEXT NOT NULL,
  size_bytes     INTEGER NOT NULL,
  mtime          INTEGER NOT NULL,
  duration_ms    INTEGER,
  sample_rate    INTEGER,
  bit_depth      INTEGER,
  channels       INTEGER,
  bitrate        INTEGER,
  codec          TEXT,
  container      TEXT,
  peak_db        REAL,
  rms_db         REAL,
  decodeable     INTEGER NOT NULL DEFAULT 1,
  clipped        INTEGER NOT NULL DEFAULT 0,
  peaks          BLOB,
  notes          TEXT NOT NULL DEFAULT '',
  search_text    TEXT NOT NULL DEFAULT '',
  added_at       INTEGER NOT NULL,
  play_count     INTEGER NOT NULL DEFAULT 0,
  last_played_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_tracks_dir      ON tracks(dir);
CREATE INDEX IF NOT EXISTS idx_tracks_ext      ON tracks(ext);
CREATE INDEX IF NOT EXISTS idx_tracks_added    ON tracks(added_at);
CREATE INDEX IF NOT EXISTS idx_tracks_duration ON tracks(duration_ms);
CREATE INDEX IF NOT EXISTS idx_tracks_root     ON tracks(root_id);
CREATE TABLE IF NOT EXISTS tags(
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
  color      TEXT,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS track_tags(
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  tag_id   INTEGER NOT NULL REFERENCES tags(id)   ON DELETE CASCADE,
  added_at INTEGER NOT NULL,
  PRIMARY KEY(track_id, tag_id)
);
CREATE INDEX IF NOT EXISTS idx_track_tags_tag ON track_tags(tag_id);
CREATE TABLE IF NOT EXISTS views(
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
  filter     TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
"#;

pub fn open(db_path: &Path) -> AppResult<SharedDb> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 10_000)?;
    conn.execute_batch(SCHEMA)?;
    seed_settings(&conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

fn seed_settings(conn: &Connection) -> AppResult<()> {
    let defaults = Settings::default();
    let mut stmt = conn.prepare("INSERT OR IGNORE INTO settings(key, value) VALUES (?1, ?2)")?;
    for (k, v) in [
        ("genPeaks", defaults.gen_peaks.to_string()),
        ("peakBuckets", defaults.peak_buckets.to_string()),
        ("theme", defaults.theme.clone()),
        ("scanHidden", defaults.scan_hidden.to_string()),
        ("minSizeBytes", defaults.min_size_bytes.to_string()),
        ("volume", defaults.volume.to_string()),
        ("autoUpdate", defaults.auto_update.to_string()),
    ] {
        stmt.execute(params![k, v])?;
    }
    Ok(())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `\\?\D:\dir` is unreadable in the UI and breaks directory prefix matching.
pub fn strip_verbatim(s: &str) -> String {
    s.trim_start_matches(r"\\?\").to_string()
}

pub fn folder_label(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    match Path::new(trimmed).file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => trimmed.to_string(),
    }
}

fn canonical(path: &str) -> String {
    let p = Path::new(path);
    let resolved = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    strip_verbatim(&resolved.to_string_lossy())
}

/* -------------------------------------------------------------------- roots */

pub fn list_roots(conn: &Connection) -> AppResult<Vec<Root>> {
    let mut stmt = conn.prepare(
        r#"SELECT r.id, r.path, r.label, r.added_at, r.last_scan_at,
                  (SELECT COUNT(*) FROM tracks t WHERE t.root_id = r.id)
           FROM roots r ORDER BY r.added_at"#,
    )?;
    let out = stmt
        .query_map([], |row| {
            Ok(Root {
                id: row.get(0)?,
                path: row.get(1)?,
                label: row.get(2)?,
                added_at: row.get(3)?,
                last_scan_at: row.get(4)?,
                count: row.get(5)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(out)
}

pub fn add_root(conn: &Connection, path: &str) -> AppResult<Root> {
    let full = canonical(path);
    if !Path::new(&full).is_dir() {
        return Err(AppError::Other(format!("不是一个文件夹：{full}")));
    }
    let existing: Option<(i64, i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, added_at, last_scan_at FROM roots WHERE path = ?1",
            params![full],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let id = match existing {
        Some((id, _, _)) => id,
        None => {
            conn.prepare("INSERT INTO roots(path, label, added_at) VALUES (?1, ?2, ?3)")?
                .execute(params![full, folder_label(&full), now_secs()])?;
            conn.last_insert_rowid()
        }
    };
    let (added_at, last_scan_at) = conn.query_row(
        "SELECT added_at, last_scan_at FROM roots WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Root {
        id,
        label: folder_label(&full),
        path: full,
        added_at,
        last_scan_at,
        count: count_root(conn, id)?,
    })
}

fn count_root(conn: &Connection, root_id: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE root_id = ?1",
        params![root_id],
        |r| r.get(0),
    )?)
}

pub fn remove_root(conn: &Connection, id: i64) -> AppResult<()> {
    conn.prepare("DELETE FROM roots WHERE id = ?1")?.execute(params![id])?;
    Ok(())
}

pub fn touch_scan(conn: &Connection, id: i64) -> AppResult<()> {
    conn.prepare("UPDATE roots SET last_scan_at = ?2 WHERE id = ?1")?
        .execute(params![id, now_secs()])?;
    Ok(())
}

/* ------------------------------------------------------------------- tracks */

pub struct NewTrack {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub dir: String,
    pub root_id: Option<i64>,
    pub size_bytes: i64,
    pub mtime: i64,
    pub duration_ms: Option<i64>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub channels: Option<i64>,
    pub bitrate: Option<i64>,
    pub codec: Option<String>,
    pub container: Option<String>,
    pub peak_db: Option<f64>,
    pub rms_db: Option<f64>,
    pub decodeable: bool,
    pub clipped: bool,
    pub peaks: Option<Vec<u8>>,
}

/// Registers a probed file. Unchanged size+mtime is skipped unless `force`, so a
/// re-scan of an untouched library writes nothing.
pub fn upsert_track(conn: &Connection, t: &NewTrack, force: bool) -> AppResult<bool> {
    let existing: Option<(i64, i64, i64)> = conn
        .query_row(
            "SELECT id, size_bytes, mtime FROM tracks WHERE path = ?1",
            params![t.path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;

    if let Some((id, size, mtime)) = existing {
        if !force && size == t.size_bytes && mtime == t.mtime {
            return Ok(false);
        }
        // root_id stays out of the update: when two roots overlap, the file keeps
        // belonging to the root that found it first, so removing the later root
        // cannot cascade rows the earlier one still covers on disk.
        conn.prepare(
            r#"UPDATE tracks SET name=?2, ext=?3, dir=?4, size_bytes=?5, mtime=?6,
                     duration_ms=?7, sample_rate=?8, bit_depth=?9, channels=?10, bitrate=?11,
                     codec=?12, container=?13, peak_db=?14, rms_db=?15, decodeable=?16,
                     clipped=?17, peaks=COALESCE(?18, peaks)
               WHERE id=?1"#,
        )?
        .execute(params![
            id,
            t.name,
            t.ext,
            t.dir,
            t.size_bytes,
            t.mtime,
            t.duration_ms,
            t.sample_rate,
            t.bit_depth,
            t.channels,
            t.bitrate,
            t.codec,
            t.container,
            t.peak_db,
            t.rms_db,
            t.decodeable as i64,
            t.clipped as i64,
            t.peaks
        ])?;
        reindex_one(conn, id)?;
        return Ok(false);
    }

    conn.prepare(
        r#"INSERT INTO tracks(path, name, ext, dir, root_id, size_bytes, mtime, duration_ms,
              sample_rate, bit_depth, channels, bitrate, codec, container, peak_db, rms_db,
              decodeable, clipped, peaks, added_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)"#,
    )?
    .execute(params![
        t.path,
        t.name,
        t.ext,
        t.dir,
        t.root_id,
        t.size_bytes,
        t.mtime,
        t.duration_ms,
        t.sample_rate,
        t.bit_depth,
        t.channels,
        t.bitrate,
        t.codec,
        t.container,
        t.peak_db,
        t.rms_db,
        t.decodeable as i64,
        t.clipped as i64,
        t.peaks,
        now_secs()
    ])?;
    let id = conn.last_insert_rowid();
    reindex_one(conn, id)?;
    Ok(true)
}

/// search_text is what LIKE reads: alias, file name, folder, notes, tag names.
/// Keeping it denormalised means one scan per query and no index to maintain
/// in sync with the tag table.
pub fn reindex_one(conn: &Connection, track_id: i64) -> AppResult<()> {
    let text: Option<String> = conn
        .query_row(
            r#"SELECT lower(
                 COALESCE(t.alias,'') || ' ' || t.name || ' ' || t.dir || ' ' || t.notes || ' ' ||
                 COALESCE((SELECT group_concat(g.name, ' ')
                             FROM track_tags tt JOIN tags g ON g.id = tt.tag_id
                            WHERE tt.track_id = t.id), '')
               )
               FROM tracks t WHERE t.id = ?1"#,
            params![track_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(text) = text {
        conn.prepare("UPDATE tracks SET search_text = ?2 WHERE id = ?1")?
            .execute(params![track_id, text])?;
    }
    Ok(())
}

pub fn reindex_many(conn: &Connection, ids: &[i64]) -> AppResult<()> {
    for id in ids {
        reindex_one(conn, *id)?;
    }
    Ok(())
}

/* -------------------------------------------------------------------- query */

const DURATION_BUCKETS: [(&str, i64, i64); 5] = [
    ("lt1", 0, 1_000),
    ("1to3", 1_000, 3_000),
    ("3to10", 3_000, 10_000),
    ("10to60", 10_000, 60_000),
    ("gt60", 60_000, i64::MAX),
];

fn sort_sql(sort: &str, dir: &str) -> String {
    let expr = match sort {
        "duration" => "t.duration_ms",
        "sample_rate" => "t.sample_rate",
        "bit_depth" => "t.bit_depth",
        "channels" => "t.channels",
        "size" => "t.size_bytes",
        "peak_db" => "t.peak_db",
        "added" => "t.added_at",
        "modified" => "t.mtime",
        "plays" => "t.play_count",
        "path" => "t.dir || t.name",
        "ext" => "t.ext",
        _ => "COALESCE(t.alias, t.name)",
    };
    let order = if dir == "desc" { "DESC" } else { "ASC" };
    format!("{expr} {order} NULLS LAST")
}

/// Positional placeholders are numbered explicitly because SQLite forbids
/// mixing `?` with `?N` in one statement.
struct Args(Vec<Box<dyn rusqlite::types::ToSql>>);

impl Args {
    fn push<V: rusqlite::types::ToSql + 'static>(&mut self, v: V) -> usize {
        self.0.push(Box::new(v));
        self.0.len()
    }

    fn refs(&self) -> Vec<&dyn rusqlite::types::ToSql> {
        self.0.iter().map(|a| a.as_ref()).collect()
    }
}

fn build_where(f: &Filter, args: &mut Args) -> String {
    let mut w: Vec<String> = Vec::new();

    let text = f.text.trim().to_lowercase();
    for term in text.split_whitespace() {
        let needle = format!("%{term}%");
        let i = args.push(needle);
        w.push(format!("t.search_text LIKE ?{i}"));
    }

    let or_group = |values: Vec<String>, column: &str, args: &mut Args| -> Option<String> {
        let mut parts = Vec::new();
        for v in values {
            // Text columns must bind as text: parsing "wav" as an integer would
            // silently turn the clause into `t.ext = 0` and match nothing.
            let i = match column {
                "t.ext" => args.push(v),
                _ => args.push(v.parse::<i64>().unwrap_or(0)),
            };
            parts.push(format!("{column} = ?{i}"));
        }
        if parts.is_empty() {
            None
        } else {
            Some(format!("({})", parts.join(" OR ")))
        }
    };

    if !f.formats.is_empty() {
        if let Some(g) = or_group(f.formats.iter().map(|s| s.to_lowercase()).collect(), "t.ext", args) {
            w.push(g);
        }
    }
    if !f.sample_rates.is_empty() {
        if let Some(g) = or_group(f.sample_rates.iter().map(|v| v.to_string()).collect(), "t.sample_rate", args) {
            w.push(g);
        }
    }
    if !f.channels.is_empty() {
        if let Some(g) = or_group(f.channels.iter().map(|v| v.to_string()).collect(), "t.channels", args) {
            w.push(g);
        }
    }
    if !f.durations.is_empty() {
        let mut parts = Vec::new();
        for bucket in &f.durations {
            let Some((_, lo, hi)) = DURATION_BUCKETS.iter().find(|(k, _, _)| k == bucket) else {
                continue;
            };
            let a = args.push(*lo);
            let b = args.push(*hi);
            parts.push(format!("(t.duration_ms >= ?{a} AND t.duration_ms < ?{b})"));
        }
        if !parts.is_empty() {
            w.push(format!("({})", parts.join(" OR ")));
        }
    }

    if !f.tag_ids.is_empty() {
        let mut marks = Vec::new();
        for id in &f.tag_ids {
            let i = args.push(*id);
            marks.push(format!("?{i}"));
        }
        let list = marks.join(",");
        if f.tag_match == "all" {
            w.push(format!(
                "(SELECT COUNT(DISTINCT tt.tag_id) FROM track_tags tt
                  WHERE tt.track_id = t.id AND tt.tag_id IN ({list})) = {}",
                f.tag_ids.len()
            ));
        } else {
            w.push(format!(
                "EXISTS(SELECT 1 FROM track_tags tt WHERE tt.track_id = t.id AND tt.tag_id IN ({list}))"
            ));
        }
    }

    match f.scope.as_str() {
        "untagged" => w.push("NOT EXISTS(SELECT 1 FROM track_tags tt WHERE tt.track_id = t.id)".into()),
        "notes" => w.push("trim(t.notes) <> ''".into()),
        "recent" => {
            let i = args.push(now_secs() - 7 * 24 * 3600);
            w.push(format!("t.added_at >= ?{i}"));
        }
        "clipped" => w.push("t.clipped = 1".into()),
        "undecodable" => w.push("t.decodeable = 0".into()),
        "root" => {
            if let Some(rid) = f.root_id {
                let i = args.push(rid);
                w.push(format!("t.root_id = ?{i}"));
            }
        }
        _ => {}
    }

    if w.is_empty() {
        "WHERE 1=1".into()
    } else {
        format!("WHERE {}", w.join(" AND "))
    }
}

pub fn query(conn: &Connection, f: &Filter) -> AppResult<QueryResult> {
    let mut args = Args(Vec::new());
    let where_sql = build_where(f, &mut args);

    let total: i64 = {
        let bound = args.refs();
        conn.query_row(
            &format!("SELECT COUNT(*) FROM tracks t {where_sql}"),
            bound.as_slice(),
            |r| r.get(0),
        )?
    };

    let limit = f.limit.clamp(1, 2000);
    let offset = f.offset.max(0);
    let li = args.push(limit);
    let oi = args.push(offset);

    let sql = format!(
        r#"SELECT t.id, t.name, t.alias, t.ext, t.dir, t.path, t.size_bytes, t.mtime,
                  t.duration_ms, t.sample_rate, t.bit_depth, t.channels, t.bitrate, t.peak_db,
                  t.decodeable, (trim(t.notes) <> ''), t.added_at
           FROM tracks t {where_sql}
           ORDER BY {}
           LIMIT ?{li} OFFSET ?{oi}"#,
        sort_sql(&f.sort, &f.dir)
    );

    let mut out = {
        let bound = args.refs();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), |r| {
            Ok(TrackRow {
                id: r.get(0)?,
                name: r.get(1)?,
                alias: r.get(2)?,
                ext: r.get(3)?,
                dir: r.get(4)?,
                path: r.get(5)?,
                size_bytes: r.get(6)?,
                mtime: r.get(7)?,
                duration_ms: r.get(8)?,
                sample_rate: r.get(9)?,
                bit_depth: r.get(10)?,
                channels: r.get(11)?,
                bitrate: r.get(12)?,
                peak_db: r.get(13)?,
                decodeable: r.get::<_, i64>(14)? != 0,
                has_notes: r.get::<_, i64>(15)? != 0,
                tag_ids: Vec::new(),
                added_at: r.get(16)?,
            })
        })?;
        rows.filter_map(|x| x.ok()).collect::<Vec<TrackRow>>()
    };

    let ids: Vec<i64> = out.iter().map(|i| i.id).collect();
    let tags = tags_for_tracks(conn, &ids)?;
    for row in &mut out {
        if let Some(list) = tags.get(&row.id) {
            row.tag_ids = list.clone();
        }
    }
    Ok(QueryResult { items: out, total })
}

fn tags_for_tracks(conn: &Connection, ids: &[i64]) -> AppResult<HashMap<i64, Vec<i64>>> {
    let mut map: HashMap<i64, Vec<i64>> = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let mut args = Args(Vec::new());
    let mut marks = Vec::new();
    for id in ids {
        let i = args.push(*id);
        marks.push(format!("?{i}"));
    }
    let sql = format!(
        "SELECT tt.track_id, tt.tag_id FROM track_tags tt WHERE tt.track_id IN ({}) ORDER BY tt.added_at",
        marks.join(",")
    );
    let bound = args.refs();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(bound.as_slice(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows.flatten() {
        map.entry(row.0).or_default().push(row.1);
    }
    Ok(map)
}

pub fn track_detail(conn: &Connection, id: i64) -> AppResult<TrackDetail> {
    let mut d = conn
        .query_row(
            r#"SELECT id, name, alias, ext, dir, path, size_bytes, mtime, duration_ms, sample_rate,
                      bit_depth, channels, bitrate, codec, container, peak_db, rms_db, decodeable,
                      notes, added_at, play_count, last_played_at
               FROM tracks WHERE id = ?1"#,
            params![id],
            |r| {
                Ok(TrackDetail {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    alias: r.get(2)?,
                    ext: r.get(3)?,
                    dir: r.get(4)?,
                    path: r.get(5)?,
                    size_bytes: r.get(6)?,
                    mtime: r.get(7)?,
                    duration_ms: r.get(8)?,
                    sample_rate: r.get(9)?,
                    bit_depth: r.get(10)?,
                    channels: r.get(11)?,
                    bitrate: r.get(12)?,
                    codec: r.get(13)?,
                    container: r.get(14)?,
                    peak_db: r.get(15)?,
                    rms_db: r.get(16)?,
                    decodeable: r.get::<_, i64>(17)? != 0,
                    notes: r.get(18)?,
                    tag_ids: Vec::new(),
                    added_at: r.get(19)?,
                    play_count: r.get(20)?,
                    last_played_at: r.get(21)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| AppError::Other("该条目已不在库中".into()))?;
    d.tag_ids = tags_for_tracks(conn, &[id])?.remove(&id).unwrap_or_default();
    Ok(d)
}

/// Tag chips and note flags for many rows at once. A bulk tag or note edit
/// repaints every selected row, and one query per row reads as jank.
pub fn row_flags(conn: &Connection, ids: &[i64]) -> AppResult<HashMap<i64, RowFlags>> {
    let mut map = tags_for_tracks(conn, ids)?
        .into_iter()
        .map(|(id, tag_ids)| (id, RowFlags { tag_ids, has_notes: false }))
        .collect::<HashMap<_, _>>();
    if !ids.is_empty() {
        let mut args = Args(Vec::new());
        let marks = ids.iter().map(|id| format!("?{}", args.push(*id))).collect::<Vec<_>>();
        let sql = format!("SELECT id, trim(notes) FROM tracks WHERE id IN ({})", marks.join(","));
        let bound = args.refs();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows.flatten() {
            if let Some(f) = map.get_mut(&row.0) {
                f.has_notes = !row.1.is_empty();
            }
        }
    }
    // Rows with no tags need an entry, so the caller can clear stale chips.
    for id in ids {
        map.entry(*id).or_insert_with(|| RowFlags { tag_ids: Vec::new(), has_notes: false });
    }
    Ok(map)
}

pub fn track_path(conn: &Connection, id: i64) -> AppResult<String> {
    Ok(conn
        .query_row("SELECT path FROM tracks WHERE id = ?1", params![id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| AppError::Other("该条目已不在库中".into()))?)
}

pub fn peaks_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn track_peaks(conn: &Connection, id: i64) -> AppResult<Peaks> {
    let blob: Option<Option<Vec<u8>>> = conn
        .query_row("SELECT peaks FROM tracks WHERE id = ?1", params![id], |r| r.get(0))
        .optional()?;
    let bytes = blob.flatten().unwrap_or_default();
    let values: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(Peaks { buckets: values.len() / 2, values })
}

pub fn set_alias(conn: &Connection, id: i64, alias: &str) -> AppResult<()> {
    let v = if alias.trim().is_empty() {
        None
    } else {
        Some(alias.trim().to_string())
    };
    conn.prepare("UPDATE tracks SET alias = ?2 WHERE id = ?1")?
        .execute(params![id, v])?;
    reindex_one(conn, id)
}

/// `mode` is "append" (timestamped, never destructive) or anything else for replace.
pub fn set_notes(conn: &Connection, ids: &[i64], text: &str, mode: &str) -> AppResult<()> {
    let mut upd = conn.prepare("UPDATE tracks SET notes = ?2 WHERE id = ?1")?;
    for id in ids {
        let value = if mode == "append" && !text.trim().is_empty() {
            let cur: String = conn
                .query_row("SELECT notes FROM tracks WHERE id = ?1", params![id], |r| r.get(0))
                .unwrap_or_default();
            let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M");
            if cur.trim().is_empty() {
                format!("[{stamp}] {}", text.trim())
            } else {
                format!("{}\n[{}] {}", cur.trim_end(), stamp, text.trim())
            }
        } else {
            text.to_string()
        };
        upd.execute(params![id, value])?;
        reindex_one(conn, *id)?;
    }
    Ok(())
}

pub fn remove_tracks(conn: &Connection, ids: &[i64]) -> AppResult<usize> {
    let mut n = 0;
    let mut stmt = conn.prepare("DELETE FROM tracks WHERE id = ?1")?;
    for id in ids {
        n += stmt.execute(params![id])?;
    }
    Ok(n)
}

pub fn note_play(conn: &Connection, id: i64) -> AppResult<()> {
    conn.prepare("UPDATE tracks SET play_count = play_count + 1, last_played_at = ?2 WHERE id = ?1")?
        .execute(params![id, now_secs()])?;
    Ok(())
}

/* --------------------------------------------------------------------- tags */

pub fn list_tags(conn: &Connection) -> AppResult<Vec<Tag>> {
    let mut stmt = conn.prepare(
        r#"SELECT g.id, g.name, g.color,
                  (SELECT COUNT(*) FROM track_tags tt WHERE tt.tag_id = g.id)
           FROM tags g
           ORDER BY 4 DESC, g.name COLLATE NOCASE"#,
    )?;
    let out = stmt
        .query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                color: r.get(2)?,
                use_count: r.get(3)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(out)
}

pub fn create_tag(conn: &Connection, name: &str, color: Option<&str>) -> AppResult<Tag> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Other("标签名不能为空".into()));
    }
    let existing: Option<(i64, Option<String>, i64)> = conn
        .query_row(
            r#"SELECT g.id, g.color, (SELECT COUNT(*) FROM track_tags tt WHERE tt.tag_id = g.id)
               FROM tags g WHERE g.name = ?1"#,
            params![name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, color, use_count)) = existing {
        return Ok(Tag { id, name: name.to_string(), color, use_count });
    }
    conn.prepare("INSERT INTO tags(name, color, created_at) VALUES (?1, ?2, ?3)")?
        .execute(params![name, color, now_secs()])?;
    Ok(Tag {
        id: conn.last_insert_rowid(),
        name: name.to_string(),
        color: color.map(str::to_string),
        use_count: 0,
    })
}

pub fn update_tag(conn: &Connection, id: i64, name: Option<&str>, color: Option<&str>) -> AppResult<()> {
    if let Some(n) = name.map(str::trim).filter(|s| !s.is_empty()) {
        conn.prepare("UPDATE tags SET name = ?2 WHERE id = ?1")?
            .execute(params![id, n])?;
    }
    if let Some(c) = color {
        let v = if c.is_empty() { None } else { Some(c) };
        conn.prepare("UPDATE tags SET color = ?2 WHERE id = ?1")?
            .execute(params![id, v])?;
    }
    let affected = tracks_with_tag(conn, id)?;
    reindex_many(conn, &affected)
}

pub fn delete_tag(conn: &Connection, id: i64) -> AppResult<()> {
    let affected = tracks_with_tag(conn, id)?;
    conn.prepare("DELETE FROM tags WHERE id = ?1")?.execute(params![id])?;
    reindex_many(conn, &affected)
}

fn tracks_with_tag(conn: &Connection, tag_id: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT track_id FROM track_tags WHERE tag_id = ?1")?;
    let out = stmt
        .query_map(params![tag_id], |r| r.get(0))?
        .filter_map(|x| x.ok())
        .collect();
    Ok(out)
}

pub fn update_track_tags(conn: &Connection, tracks: &[i64], add: &[i64], remove: &[i64]) -> AppResult<usize> {
    if tracks.is_empty() {
        return Ok(0);
    }
    let stamp = now_secs();
    let mut changed = 0;
    {
        let mut ins = conn.prepare("INSERT OR IGNORE INTO track_tags(track_id, tag_id, added_at) VALUES (?1, ?2, ?3)")?;
        let mut del = conn.prepare("DELETE FROM track_tags WHERE track_id = ?1 AND tag_id = ?2")?;
        for tid in tracks {
            for tag in add {
                changed += ins.execute(params![*tid, *tag, stamp])?;
            }
            for tag in remove {
                changed += del.execute(params![*tid, *tag])?;
            }
        }
    }
    for tid in tracks {
        reindex_one(conn, *tid)?;
    }
    if !remove.is_empty() {
        conn.execute(
            "DELETE FROM tags WHERE NOT EXISTS(SELECT 1 FROM track_tags tt WHERE tt.tag_id = tags.id)",
            [],
        )?;
    }
    Ok(changed)
}

/* -------------------------------------------------------------------- stats */

fn facet(conn: &Connection, sql: &str) -> AppResult<Vec<FacetCount>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| Ok(FacetCount { value: r.get(0)?, count: r.get(1)? }))?;
    Ok(rows.filter_map(|x| x.ok()).collect())
}

pub fn stats(conn: &Connection) -> AppResult<Stats> {
    let week_ago = now_secs() - 7 * 24 * 3600;
    let (track_count, untagged, notes, recent, clipped, undecodable, total_duration_ms, total_size_bytes) = conn
        .query_row(
            // Every SUM needs a COALESCE: over an empty library SQLite returns
            // NULL, and a typed read of that is an error, not a zero.
            r#"SELECT COUNT(*),
                      COALESCE(SUM(CASE WHEN NOT EXISTS(SELECT 1 FROM track_tags tt WHERE tt.track_id = t.id) THEN 1 ELSE 0 END), 0),
                      COALESCE(SUM(CASE WHEN trim(t.notes) <> '' THEN 1 ELSE 0 END), 0),
                      COALESCE(SUM(CASE WHEN t.added_at >= ?1 THEN 1 ELSE 0 END), 0),
                      COALESCE(SUM(CASE WHEN t.clipped = 1 THEN 1 ELSE 0 END), 0),
                      COALESCE(SUM(CASE WHEN t.decodeable = 0 THEN 1 ELSE 0 END), 0),
                      COALESCE(SUM(t.duration_ms), 0),
                      COALESCE(SUM(t.size_bytes), 0)
               FROM tracks t"#,
            params![week_ago],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            },
        )?;

    let tag_count: i64 = conn.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?;
    let root_count: i64 = conn.query_row("SELECT COUNT(*) FROM roots", [], |r| r.get(0))?;

    let durations = {
        let mut map: HashMap<String, i64> = HashMap::new();
        let mut stmt = conn.prepare("SELECT duration_ms FROM tracks WHERE duration_ms IS NOT NULL")?;
        let vals = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .filter_map(|x| x.ok())
            .collect::<Vec<i64>>();
        drop(stmt);
        for v in vals {
            for (name, lo, hi) in DURATION_BUCKETS {
                if v >= lo && v < hi {
                    *map.entry(name.to_string()).or_default() += 1;
                    break;
                }
            }
        }
        DURATION_BUCKETS
            .iter()
            .filter_map(|(name, _, _)| {
                map.get(*name).map(|c| FacetCount { value: name.to_string(), count: *c })
            })
            .collect()
    };

    Ok(Stats {
        track_count,
        untagged,
        notes,
        recent,
        clipped,
        undecodable,
        total_duration_ms,
        total_size_bytes,
        tag_count,
        root_count,
        formats: facet(conn, "SELECT ext, COUNT(*) FROM tracks WHERE ext <> '' GROUP BY ext ORDER BY COUNT(*) DESC")?,
        sample_rates: facet(
            conn,
            "SELECT CAST(sample_rate AS TEXT), COUNT(*) FROM tracks WHERE sample_rate IS NOT NULL
              GROUP BY sample_rate ORDER BY sample_rate",
        )?,
        channels: facet(
            conn,
            "SELECT CAST(channels AS TEXT), COUNT(*) FROM tracks WHERE channels IS NOT NULL
              GROUP BY channels ORDER BY channels",
        )?,
        durations,
    })
}

pub fn get_settings(conn: &Connection) -> AppResult<Settings> {
    let mut s = Settings::default();
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|x| x.ok())
        .collect();
    for (k, v) in rows {
        match k.as_str() {
            "genPeaks" => s.gen_peaks = v == "true",
            "peakBuckets" => s.peak_buckets = v.parse().unwrap_or(512),
            "theme" => s.theme = v,
            "scanHidden" => s.scan_hidden = v == "true",
            "minSizeBytes" => s.min_size_bytes = v.parse().unwrap_or(0),
            "volume" => s.volume = v.parse().unwrap_or(0.7),
            "autoUpdate" => s.auto_update = v == "true",
            _ => {}
        }
    }
    Ok(s)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.prepare(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )?
    .execute(params![key, value])?;
    Ok(())
}

pub fn db_path_of(conn: &Connection) -> Option<PathBuf> {
    conn.path().map(PathBuf::from)
}

/// Path → (id, size, mtime) for one root (or the whole library), so the scanner
/// can compare fingerprints without a query per file.
pub fn fingerprint_map(conn: &Connection, root_id: Option<i64>) -> AppResult<HashMap<String, (i64, i64, i64)>> {
    let sql = match root_id {
        Some(_) => "SELECT path, id, size_bytes, mtime FROM tracks WHERE root_id = ?1",
        None => "SELECT path, id, size_bytes, mtime FROM tracks",
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = match root_id {
        Some(rid) => stmt.query_map(params![rid], map_fingerprint),
        None => stmt.query_map([], map_fingerprint),
    }?;
    let mut map = HashMap::new();
    for row in rows.flatten() {
        map.insert(row.0.to_lowercase(), (row.1, row.2, row.3));
    }
    Ok(map)
}

fn map_fingerprint(r: &rusqlite::Row) -> rusqlite::Result<(String, i64, i64, i64)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
}

pub fn delete_track(conn: &Connection, id: i64) -> AppResult<()> {
    conn.prepare("DELETE FROM tracks WHERE id = ?1")?.execute(params![id])?;
    Ok(())
}

/// Used by "rebuild waveforms": every known file, with its stored id.
pub fn all_tracks_for_rebuild(conn: &Connection) -> AppResult<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("SELECT id, path FROM tracks ORDER BY id")?;
    let out = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(|x| x.ok())
        .collect();
    Ok(out)
}

pub fn set_wave(conn: &Connection, id: i64, bytes: &[u8], peak_db: f64, rms_db: f64, clipped: bool) -> AppResult<()> {
    conn.prepare("UPDATE tracks SET peaks = ?2, peak_db = ?3, rms_db = ?4, clipped = ?5, decodeable = 1 WHERE id = ?1")?
        .execute(params![id, bytes, peak_db, rms_db, clipped as i64])?;
    Ok(())
}

pub fn mark_undecodable(conn: &Connection, id: i64) -> AppResult<()> {
    conn.prepare("UPDATE tracks SET decodeable = 0, peaks = NULL WHERE id = ?1")?
        .execute(params![id])?;
    Ok(())
}

/// Used by the manifest importer: which library rows are known by path.
pub fn find_track_by_path(conn: &Connection, path: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row("SELECT id FROM tracks WHERE path = ?1", params![path], |r| r.get(0))
        .optional()?)
}

/// Path and duration for the playback command, so the player never re-reads
/// metadata the library already has.
pub fn track_play_target(conn: &Connection, id: i64) -> AppResult<(String, i64)> {
    conn.query_row(
        "SELECT path, COALESCE(duration_ms, 0) FROM tracks WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|_| AppError::Other("该文件已不在库中".into()))
}

/* ----------------------------------------------------------------- manifest */

pub fn export_rows(conn: &Connection) -> AppResult<Vec<ManifestEntry>> {
    let mut tag_of: HashMap<i64, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT tt.track_id, g.name FROM track_tags tt JOIN tags g ON g.id = tt.tag_id ORDER BY g.name",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for (track_id, name) in rows.flatten() {
            tag_of.entry(track_id).or_default().push(name);
        }
    }
    let mut stmt = conn.prepare("SELECT id, path, alias, notes FROM tracks ORDER BY path")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows.flatten() {
        out.push(ManifestEntry {
            path: row.1,
            alias: row.2,
            notes: row.3,
            tags: tag_of.remove(&row.0).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Restores one backup row onto the file it names. Returns false when that file
/// is not in the library, so the caller can report how much was skipped.
pub fn apply_manifest_entry(conn: &Connection, entry: &ManifestEntry) -> AppResult<bool> {
    let Some(id) = find_track_by_path(conn, &entry.path)? else {
        return Ok(false);
    };
    if let Some(alias) = &entry.alias {
        if !alias.trim().is_empty() {
            set_alias(conn, id, alias)?;
        }
    }
    if !entry.notes.trim().is_empty() {
        set_notes(conn, std::slice::from_ref(&id), &entry.notes, "replace")?;
    }
    let mut tag_ids = Vec::new();
    for name in &entry.tags {
        if !name.trim().is_empty() {
            tag_ids.push(create_tag(conn, name, None)?.id);
        }
    }
    if !tag_ids.is_empty() {
        update_track_tags(conn, std::slice::from_ref(&id), &tag_ids, &[])?;
    }
    Ok(true)
}

/* -------------------------------------------------------------------- views */

/// Saved filter combinations. The filter is stored as the exact JSON the frontend
/// sent, so re-applying a view reproduces every facet the user had picked.
pub fn list_views(conn: &Connection) -> AppResult<Vec<SavedView>> {
    let mut stmt = conn.prepare("SELECT id, name, filter, created_at FROM views ORDER BY created_at, id")?;
    let mut out: Vec<SavedView> = stmt
        .query_map([], |r| {
            Ok(SavedView {
                id: r.get(0)?,
                name: r.get(1)?,
                filter: r.get(2)?,
                created_at: r.get(3)?,
                count: 0,
            })
        })?
        .flatten()
        .collect();
    drop(stmt);
    for v in &mut out {
        if let Ok(f) = serde_json::from_str::<Filter>(&v.filter) {
            v.count = count_matching(conn, &f).unwrap_or(0);
        }
    }
    Ok(out)
}

pub fn count_matching(conn: &Connection, f: &Filter) -> AppResult<i64> {
    let mut args = Args(Vec::new());
    let where_sql = build_where(f, &mut args);
    let bound = args.refs();
    let n = conn.query_row(
        &format!("SELECT COUNT(*) FROM tracks t {where_sql}"),
        bound.as_slice(),
        |r| r.get(0),
    )?;
    Ok(n)
}

fn read_view(conn: &Connection, id: i64) -> AppResult<SavedView> {
    let mut v = conn.query_row(
        "SELECT id, name, filter, created_at FROM views WHERE id = ?1",
        params![id],
        |r| {
            Ok(SavedView {
                id: r.get(0)?,
                name: r.get(1)?,
                filter: r.get(2)?,
                created_at: r.get(3)?,
                count: 0,
            })
        },
    )?;
    if let Ok(f) = serde_json::from_str::<Filter>(&v.filter) {
        v.count = count_matching(conn, &f).unwrap_or(0);
    }
    Ok(v)
}

fn other_view_with_name(conn: &Connection, name: &str, except_id: i64) -> AppResult<bool> {
    let id: Option<i64> = conn
        .query_row("SELECT id FROM views WHERE name = ?1 AND id <> ?2", params![name, except_id], |r| r.get(0))
        .optional()?;
    Ok(id.is_some())
}

/// The name is a view's identity, so saving under an existing name replaces that
/// view rather than piling up near-duplicates.
pub fn save_view(conn: &Connection, name: &str, filter_json: &str) -> AppResult<SavedView> {
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM views WHERE name = ?1", params![name], |r| r.get(0))
        .optional()?;
    let id = match existing {
        Some(id) => {
            conn.prepare("UPDATE views SET filter = ?2 WHERE id = ?1")?.execute(params![id, filter_json])?;
            id
        }
        None => {
            conn.prepare("INSERT INTO views(name, filter, created_at) VALUES (?1, ?2, ?3)")?
                .execute(params![name, filter_json, now_secs()])?;
            conn.last_insert_rowid()
        }
    };
    read_view(conn, id)
}

pub fn update_view(conn: &Connection, id: i64, name: &str, filter_json: &str) -> AppResult<SavedView> {
    if other_view_with_name(conn, name, id)? {
        return Err(AppError::Other(format!("已有同名视图「{name}」")));
    }
    let changed = conn
        .prepare("UPDATE views SET name = ?2, filter = ?3 WHERE id = ?1")?
        .execute(params![id, name, filter_json])?;
    if changed == 0 {
        return Err(AppError::Other("该视图已不存在".into()));
    }
    read_view(conn, id)
}

pub fn delete_view(conn: &Connection, id: i64) -> AppResult<()> {
    conn.prepare("DELETE FROM views WHERE id = ?1")?.execute(params![id])?;
    Ok(())
}
