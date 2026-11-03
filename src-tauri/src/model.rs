use serde::{Deserialize, Serialize};

/// One row of the library list. Deliberately small: peaks and notes are only
/// fetched for the selected track.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRow {
    pub id: i64,
    pub name: String,
    pub alias: Option<String>,
    pub ext: String,
    pub dir: String,
    pub path: String,
    pub size_bytes: i64,
    pub mtime: i64,
    pub duration_ms: Option<i64>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub channels: Option<i64>,
    pub bitrate: Option<i64>,
    pub peak_db: Option<f64>,
    pub decodeable: bool,
    pub has_notes: bool,
    pub tag_ids: Vec<i64>,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetail {
    pub id: i64,
    pub name: String,
    pub alias: Option<String>,
    pub ext: String,
    pub dir: String,
    pub path: String,
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
    pub notes: String,
    pub tag_ids: Vec<i64>,
    pub rating: i64,
    pub added_at: i64,
    pub play_count: i64,
    pub last_played_at: Option<i64>,
}

/// What the list needs to repaint a row after a bulk tag/note edit, without
/// paying a full detail query per track.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowFlags {
    pub tag_ids: Vec<i64>,
    pub has_notes: bool,
}

/// Peaks are interleaved (min, max) pairs normalised to full scale, so the
/// waveform can be drawn without decoding anything in the webview.
#[derive(Debug, Clone, Serialize)]
pub struct Peaks {
    pub values: Vec<f32>,
    pub buckets: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub id: i64,
    pub path: String,
    pub label: String,
    pub added_at: i64,
    pub last_scan_at: Option<i64>,
    pub count: i64,
}

/// A saved filter combination. `filter` is the stored JSON text, replayed to the
/// frontend verbatim so nothing about the original pick is lost in translation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    pub id: i64,
    pub name: String,
    pub filter: String,
    pub created_at: i64,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub use_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Filter {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
    /// "any" | "all"
    #[serde(default)]
    pub tag_match: String,
    /// "all" | "untagged" | "notes" | "recent" | "clipped" | "undecodable" | "root"
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub root_id: Option<i64>,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub durations: Vec<String>,
    #[serde(default)]
    pub sample_rates: Vec<i64>,
    #[serde(default)]
    pub channels: Vec<i64>,
    /// Keep only tracks rated at least this many stars; 0 means no limit.
    #[serde(default)]
    pub min_rating: i64,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub dir: String,
    #[serde(default = "default_offset")]
    pub offset: i64,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_offset() -> i64 {
    0
}
fn default_limit() -> i64 {
    400
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            text: String::new(),
            tag_ids: Vec::new(),
            tag_match: "any".into(),
            scope: "all".into(),
            root_id: None,
            formats: Vec::new(),
            durations: Vec::new(),
            sample_rates: Vec::new(),
            channels: Vec::new(),
            min_rating: 0,
            sort: "name".into(),
            dir: "asc".into(),
            offset: 0,
            limit: default_limit(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub items: Vec<TrackRow>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetCount {
    pub value: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub track_count: i64,
    pub untagged: i64,
    pub notes: i64,
    pub recent: i64,
    pub clipped: i64,
    pub undecodable: i64,
    pub tag_count: i64,
    pub root_count: i64,
    pub total_duration_ms: i64,
    pub total_size_bytes: i64,
    pub formats: Vec<FacetCount>,
    pub sample_rates: Vec<FacetCount>,
    pub channels: Vec<FacetCount>,
    pub durations: Vec<FacetCount>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub active: bool,
    pub phase: String,
    pub done: i64,
    pub total: i64,
    pub current: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub scanned: i64,
    pub added: i64,
    pub updated: i64,
    pub removed: i64,
    pub errors: i64,
    pub seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStatus {
    pub track_id: Option<i64>,
    pub playing: bool,
    pub paused: bool,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub volume: f32,
    #[serde(rename = "loop")]
    pub looped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub gen_peaks: bool,
    pub peak_buckets: i64,
    pub theme: String,
    pub scan_hidden: bool,
    pub min_size_bytes: i64,
    pub volume: f32,
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gen_peaks: true,
            peak_buckets: 512,
            theme: "system".into(),
            scan_hidden: false,
            min_size_bytes: 0,
            volume: 0.7,
            auto_update: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub db_path: String,
    pub version: String,
    pub product: String,
    pub repo: String,
    pub repo_url: String,
    pub audio_extensions: Vec<String>,
}

/// One library row in the portable JSON backup. Tag names rather than ids, so
/// a backup restores onto a database whose ids have nothing in common.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub rating: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub count: i64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub updated: i64,
    pub skipped: i64,
}
