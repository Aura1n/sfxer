//! Playback through rodio. Only one track at a time, which is what an SFX
//! review session needs: press, hear, move on. Nothing here ever writes to the
//! audio files.

use std::fs::File;
use std::io::BufReader;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ape_decoder::ApeDecoder;
use rodio::mixer::Mixer;
use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source};

use crate::error::{AppError, AppResult};
use crate::model::PlayerStatus;
use crate::probe;

struct Inner {
    /// Dropping the sink tears down the output stream, so it must stay alive.
    _sink: Option<MixerDeviceSink>,
    mixer: Option<Mixer>,
    player: Option<Arc<Player>>,
    path: Option<String>,
    track_id: Option<i64>,
    duration_ms: i64,
    volume: f32,
    looped: bool,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            _sink: None,
            mixer: None,
            player: None,
            path: None,
            track_id: None,
            duration_ms: 0,
            volume: 0.7,
            looped: false,
        }
    }
}

pub struct PlayerCtl {
    inner: Mutex<Inner>,
}

impl Default for PlayerCtl {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerCtl {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Inner::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn ensure_sink(inner: &mut Inner) -> AppResult<()> {
        if inner.mixer.is_some() {
            return Ok(());
        }
        let mut sink = DeviceSinkBuilder::open_default_sink()
            .map_err(|e| AppError::Other(format!("没有可用的音频输出设备：{e}")))?;
        // The library runs without a logger; the drop message would only be noise.
        sink.log_on_drop(false);
        inner.mixer = Some(sink.mixer().clone());
        inner._sink = Some(sink);
        Ok(())
    }

    fn start(inner: &mut Inner, path: &str, track_id: i64, duration_ms: i64) -> AppResult<()> {
        Self::ensure_sink(inner)?;
        let source = open_source(path)?;
        let Some(mixer) = inner.mixer.clone() else {
            return Err(AppError::Other("没有可用的音频输出设备".into()));
        };
        let player = Arc::new(Player::connect_new(&mixer));
        player.set_volume(inner.volume);
        player.append(source);
        player.play();
        // Releasing the previous handle stops its sound.
        inner.player = Some(player);
        inner.path = Some(path.to_owned());
        inner.track_id = Some(track_id);
        inner.duration_ms = duration_ms;
        Ok(())
    }

    /// A rodio queue that has run out cannot be resumed: `play()` on a drained
    /// player is a no-op, so the track has to be decoded again from the start.
    fn drained(inner: &Inner) -> bool {
        matches!(&inner.player, Some(p) if !p.is_paused() && p.empty())
    }

    fn replay(inner: &mut Inner) -> bool {
        let (Some(path), Some(id)) = (inner.path.clone(), inner.track_id) else {
            return false;
        };
        let duration = inner.duration_ms;
        Self::start(inner, &path, id, duration).is_ok()
    }

    /// Doubles as the resume path: the row that is still queued just un-pauses,
    /// the one that already finished restarts.
    pub fn play(&self, path: &str, track_id: i64, duration_ms: i64) -> AppResult<()> {
        let mut inner = self.lock();
        if inner.track_id == Some(track_id) {
            if let Some(p) = &inner.player {
                if !p.empty() {
                    p.play();
                    return Ok(());
                }
            }
        }
        Self::start(&mut inner, path, track_id, duration_ms)
    }

    /// Returns `true` when playback is running afterwards.
    pub fn toggle(&self) -> bool {
        let mut inner = self.lock();
        if Self::drained(&inner) {
            return Self::replay(&mut inner);
        }
        match &inner.player {
            Some(p) if !p.is_paused() => {
                p.pause();
                false
            }
            Some(p) => {
                p.play();
                true
            }
            None => false,
        }
    }

    pub fn stop(&self) {
        let mut inner = self.lock();
        inner.player = None;
        inner.path = None;
        inner.track_id = None;
        inner.duration_ms = 0;
    }

    pub fn seek(&self, ms: i64) -> AppResult<()> {
        let inner = self.lock();
        let Some(p) = &inner.player else {
            return Ok(());
        };
        p.try_seek(Duration::from_millis(ms.max(0) as u64))
            .map_err(|e| AppError::Other(format!("该文件不支持跳转到该位置：{e}")))
    }

    pub fn set_volume(&self, value: f32) {
        let mut inner = self.lock();
        inner.volume = value.clamp(0.0, 1.0);
        if let Some(p) = &inner.player {
            p.set_volume(inner.volume);
        }
    }

    pub fn set_loop(&self, on: bool) {
        self.lock().looped = on;
    }

    /// `status` also restarts a looping track: the frontend polls it while a
    /// track is loaded, so the queue never has to buffer a whole file the way
    /// `repeat_infinite` would.
    pub fn status(&self) -> PlayerStatus {
        let mut inner = self.lock();
        if inner.looped && Self::drained(&inner) {
            Self::replay(&mut inner);
        }
        let Some(p) = &inner.player else {
            return PlayerStatus {
                track_id: None,
                playing: false,
                paused: false,
                position_ms: 0,
                duration_ms: 0,
                volume: inner.volume,
                looped: inner.looped,
            }
        };
        let paused = p.is_paused();
        PlayerStatus {
            track_id: inner.track_id,
            playing: !paused && !p.empty(),
            paused,
            position_ms: p.get_pos().as_millis() as i64,
            duration_ms: inner.duration_ms,
            volume: inner.volume,
            looped: inner.looped,
        }
    }
}

/// Opens a file as a rodio source. Monkey's Audio needs its own decoder, and a
/// WAV whose payload is MPEG audio has to be re-opened as a window over the
/// `data` chunk because the RIFF reader rejects such files.
fn open_source(path: &str) -> AppResult<Box<dyn Source + Send>> {
    let file_path = Path::new(path);
    let ext = file_path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if ext == "ape" {
        return Ok(Box::new(ApeSource::open(path)?));
    }

    let file = File::open(file_path).map_err(|e| AppError::Io(format!("{path}：{e}")))?;
    match Decoder::try_from(file) {
        Ok(decoder) => Ok(Box::new(decoder)),
        Err(err) => {
            if probe::riff_exts(&ext) {
                if let Some(span) = probe::embedded_mpeg_span(file_path) {
                    if let Ok(slice) = probe::FileSlice::open(file_path, span) {
                        if let Ok(decoder) = Decoder::builder()
                            .with_data(slice)
                            .with_hint("mp3")
                            .build()
                        {
                            return Ok(Box::new(decoder));
                        }
                    }
                }
            }
            Err(AppError::Other(format!("该文件无法解码播放：{err}")))
        }
    }
}

/// Frame-by-frame Monkey's Audio playback, following the same shape as rodio's
/// own decoder so the player and the seek bar do not need to care which one is
/// playing.
struct ApeSource {
    decoder: ApeDecoder<BufReader<File>>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    sample_bytes: usize,
    frame_bytes: usize,
    total_frames: u32,
    total_samples: u64,
    duration: Duration,
    float: bool,
    big_endian: bool,
    signed8: bool,
    frame_idx: u32,
    skip_samples: u32,
    buf: Vec<f32>,
    pos: usize,
}

impl ApeSource {
    fn open(path: &str) -> AppResult<Self> {
        let file = File::open(path).map_err(|e| AppError::Io(format!("{path}：{e}")))?;
        let decoder = ApeDecoder::new(BufReader::new(file))
            .map_err(|e| AppError::Other(format!("该文件无法解码播放：{e}")))?;
        let info = decoder.info().clone();
        let channels = NonZeroU16::new(info.channels)
            .ok_or_else(|| AppError::Other("该文件无法解码播放：声道数无效".into()))?;
        let sample_rate = NonZeroU32::new(info.sample_rate)
            .ok_or_else(|| AppError::Other("该文件无法解码播放：采样率无效".into()))?;
        let sample_bytes = info.bytes_per_sample as usize;
        let frame_bytes = sample_bytes.saturating_mul(channels.get() as usize);
        if frame_bytes == 0 {
            return Err(AppError::Other("该文件无法解码播放：位深无效".into()));
        }
        Ok(Self {
            decoder,
            channels,
            sample_rate,
            sample_bytes,
            frame_bytes,
            total_frames: info.total_frames,
            total_samples: info.total_samples,
            duration: Duration::from_millis(info.duration_ms),
            float: info.is_floating_point,
            big_endian: info.is_big_endian,
            signed8: info.is_signed_8bit,
            frame_idx: 0,
            skip_samples: 0,
            buf: Vec::new(),
            pos: 0,
        })
    }

    fn refill(&mut self) -> bool {
        while self.frame_idx < self.total_frames {
            let pcm = match self.decoder.decode_frame(self.frame_idx) {
                Ok(pcm) => pcm,
                Err(_) => return false,
            };
            self.frame_idx += 1;
            self.buf.clear();
            self.pos = 0;
            let channel_count = self.channels.get() as usize;
            for chunk in pcm.chunks_exact(self.frame_bytes) {
                for ch in 0..channel_count {
                    let bytes = &chunk[ch * self.sample_bytes..(ch + 1) * self.sample_bytes];
                    self.buf.push(probe::pcm_sample(
                        bytes,
                        self.float,
                        self.big_endian,
                        self.signed8,
                    ));
                }
            }
            if self.skip_samples > 0 {
                let skip = (self.skip_samples as usize * channel_count).min(self.buf.len());
                self.buf.drain(..skip);
                self.skip_samples = 0;
            }
            if !self.buf.is_empty() {
                return true;
            }
        }
        false
    }
}

impl Iterator for ApeSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if self.pos < self.buf.len() {
                let sample = self.buf[self.pos];
                self.pos += 1;
                return Some(sample);
            }
            if !self.refill() {
                return None;
            }
        }
    }
}

impl Source for ApeSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.duration)
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let target = (pos.as_millis() as u64).saturating_mul(self.sample_rate.get() as u64) / 1000;
        match self.decoder.seek(target.min(self.total_samples.saturating_sub(1))) {
            Ok(found) => {
                self.frame_idx = found.frame_index;
                self.skip_samples = found.skip_samples;
                self.buf.clear();
                self.pos = 0;
                Ok(())
            }
            Err(_) => Err(SeekError::NotSupported { underlying_source: "ape" }),
        }
    }
}
