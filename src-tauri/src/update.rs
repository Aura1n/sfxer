//! Update checks against the GitHub release feed. A check is one anonymous GET,
//! so it borrows the curl.exe that ships with Windows 10/11 instead of carrying
//! a TLS stack in the binary; no key, no account, and a machine that cannot
//! reach the network simply gets a failed check.

use std::cmp::Ordering;
use std::process::Command;

use serde::{Deserialize, Serialize};

pub const REPO: &str = "Aura1n/sfxer";

pub fn repo_url() -> String {
    format!("https://github.com/{REPO}")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub url: String,
    pub notes: String,
    pub newer: bool,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    body: Option<String>,
}

/// Compares `1.2.3`, `v1.2` and `1.2.3-beta` well enough for release tags:
/// three numeric components, then a pre-release sorts below the plain version.
pub fn compare(a: &str, b: &str) -> Ordering {
    let bare = |s: &str| s.trim().trim_start_matches('v').to_string();
    let core = |s: &str| bare(s).split(['-', '+']).next().unwrap_or("").to_string();
    let parts = |s: &str| -> [u64; 3] {
        let mut out = [0u64; 3];
        for (i, chunk) in core(s).split('.').take(3).enumerate() {
            out[i] = chunk.parse().unwrap_or(0);
        }
        out
    };
    match parts(a).cmp(&parts(b)) {
        Ordering::Equal => {
            let pre = |s: &str| core(s) != bare(s);
            match (pre(a), pre(b)) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => Ordering::Equal,
            }
        }
        other => other,
    }
}

pub fn latest(current: &str) -> Result<Release, String> {
    // The list endpoint, not `/releases/latest`: that one hides pre-releases,
    // so a beta-only project would report every check as a failure.
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=100");
    let mut cmd = Command::new("curl.exe");
    cmd.args(["-sS", "-L", "--max-time", "10", "-H", "Accept: application/vnd.github+json", url.as_str()]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("本机无法发起检查：{e}"))?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if text.is_empty() { "检查更新失败，可能是没有网络".into() } else { text });
    }
    let all: Vec<RawRelease> = serde_json::from_slice(&out.stdout).map_err(|_| "更新源返回了无法识别的内容".to_string())?;
    let raw = all
        .into_iter()
        .filter(|r| !r.draft)
        .max_by(|a, b| compare(&a.tag_name, &b.tag_name))
        .ok_or("仓库还没有发布版本")?;
    let version = raw.tag_name.trim_start_matches('v').to_string();
    let notes: String = raw.body.unwrap_or_default().trim().chars().take(500).collect();
    Ok(Release { newer: compare(&version, current) == Ordering::Greater, version, url: raw.html_url, notes })
}
