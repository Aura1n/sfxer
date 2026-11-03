//! Playback through rodio. Only one track at a time, which is what an SFX
//! review session needs: press, hear, move on. Nothing here ever writes to the
//! audio files.

use std::fs::File;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::mixer::Mixer;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

use crate::error::{AppError, AppResult};
use crate::model::PlayerStatus;

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
        let decoder = Decoder::try_from(File::open(path)?)
            .map_err(|e| AppError::Other(format!("该文件无法解码播放：{e}")))?;
        let Some(mixer) = inner.mixer.clone() else {
            return Err(AppError::Other("没有可用的音频输出设备".into()));
        };
        let player = Arc::new(Player::connect_new(&mixer));
        player.set_volume(inner.volume);
        player.append(decoder);
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
