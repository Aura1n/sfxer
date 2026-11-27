//! Optional help from an installed ffmpeg. Everything the built-in decoders can
//! read never gets here; this is the last resort for WMA, AC3, AMR, WavPack,
//! TTA, DSD and the odd codec inside an otherwise ordinary container.
//!
//! Nothing is bundled: the tools are found next to the program, in
//! `%APPDATA%\dev.zara.sfxer\ffmpeg`, or on PATH, so a machine without them
//! behaves exactly as before and the single-file build stays small.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::probe::{Meta, Pcm, Wave, WaveAccum};

/// Where `name.exe` lives, if anywhere. Looked up on every call: a failure is
/// rare and a few `stat`s are cheaper than a stale answer after the user
/// installs ffmpeg.
fn locate(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}.exe");
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        dirs.push(dir.clone());
        dirs.push(dir.join("ffmpeg"));
        dirs.push(dir.join("ffmpeg").join("bin"));
        dirs.push(dir.join("tools"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let base = PathBuf::from(appdata).join("dev.zara.sfxer").join("ffmpeg");
        dirs.push(base.clone());
        dirs.push(base.join("bin"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|c| c.is_file())
}

/// What the settings page reports: the paths found, or nothing.
pub fn status() -> (Option<PathBuf>, Option<PathBuf>) {
    (locate("ffmpeg"), locate("ffprobe"))
}

fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up for every file.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

pub fn probe_meta(path: &Path) -> AppResult<Meta> {
    let tool = locate("ffprobe").ok_or(AppError::Unsupported)?;
    let out = command(&tool)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
    if !out.status.success() {
        return Err(AppError::Unsupported);
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| AppError::Unsupported)?;
    let stream = v["streams"]
        .as_array()
        .and_then(|list| list.iter().find(|s| s["codec_type"] == "audio"))
        .ok_or(AppError::Unsupported)?;

    let seconds = number(&stream["duration"]).or_else(|| number(&v["format"]["duration"]));
    let bits = number(&stream["bits_per_raw_sample"])
        .or_else(|| number(&stream["bits_per_sample"]))
        .map(|b| b as i64)
        .filter(|b| *b > 0);
    let meta = Meta {
        duration_ms: seconds.map(|s| (s * 1000.0).round() as i64),
        sample_rate: number(&stream["sample_rate"]).map(|r| r as i64),
        bit_depth: bits,
        channels: number(&stream["channels"]).map(|c| c as i64),
        bitrate: number(&stream["bit_rate"]).or_else(|| number(&v["format"]["bit_rate"])).map(|b| b as i64),
        codec: stream["codec_long_name"].as_str().map(|s| format!("{s} (ffmpeg)")),
        container: v["format"]["format_name"].as_str().map(|s| s.to_string()),
    };
    if meta.sample_rate.is_none() && meta.duration_ms.is_none() {
        return Err(AppError::Unsupported);
    }
    Ok(meta)
}

/// Streams the decoded audio out of ffmpeg as little-endian f32 and hands whole
/// frames to `sink`; stops early when `sink` returns false.
fn stream_f32(path: &Path, channels: usize, rate: Option<u32>, mut sink: impl FnMut(&[f32]) -> bool) -> AppResult<()> {
    let tool = locate("ffmpeg").ok_or(AppError::Unsupported)?;
    let mut cmd = command(&tool);
    cmd.args(["-v", "error", "-nostdin", "-i"]).arg(path).args(["-vn", "-f", "f32le", "-acodec", "pcm_f32le", "-ac"]);
    cmd.arg(channels.to_string());
    if let Some(rate) = rate {
        cmd.arg("-ar").arg(rate.to_string());
    }
    cmd.arg("-").stdout(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
    let mut out = child.stdout.take().ok_or(AppError::Unsupported)?;

    let frame_bytes = channels * 4;
    let mut buf = vec![0u8; 1 << 16];
    let mut filled = 0usize;
    let mut frame = vec![0f32; channels];
    loop {
        let n = match out.read(&mut buf[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        filled += n;
        let whole = filled - filled % frame_bytes;
        let mut go_on = true;
        for chunk in buf[..whole].chunks_exact(frame_bytes) {
            for (c, s) in chunk.chunks_exact(4).enumerate() {
                let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                frame[c] = if v.is_finite() { v } else { 0.0 };
            }
            if !sink(&frame) {
                go_on = false;
                break;
            }
        }
        if !go_on {
            break;
        }
        buf.copy_within(whole..filled, 0);
        filled -= whole;
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

pub fn read_wave(path: &Path, buckets: usize) -> AppResult<Wave> {
    let channels = probe_meta(path)
        .ok()
        .and_then(|m| m.channels)
        .map(|c| c.clamp(1, 8) as usize)
        .unwrap_or(2);
    let mut acc = WaveAccum::new();
    stream_f32(path, channels, None, |frame| {
        acc.push(frame);
        true
    })?;
    acc.finish(buckets).ok_or(AppError::Unsupported)
}

pub fn decode_pcm(path: &Path, cap: usize) -> AppResult<Pcm> {
    let meta = probe_meta(path)?;
    let channels = meta.channels.unwrap_or(2).clamp(1, 8) as usize;
    let rate = meta.sample_rate.unwrap_or(44_100).clamp(8_000, 192_000) as u32;
    let mut samples: Vec<f32> = Vec::new();
    stream_f32(path, channels, Some(rate), |frame| {
        samples.extend_from_slice(frame);
        samples.len() < cap
    })?;
    if samples.is_empty() {
        return Err(AppError::Unsupported);
    }
    Ok(Pcm { channels: channels as u16, sample_rate: rate, samples })
}
