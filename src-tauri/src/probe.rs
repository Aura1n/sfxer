use std::fs::File;
use std::path::Path;

use lofty::file::FileType;
use lofty::prelude::*;
use lofty::probe::Probe;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::error::{AppError, AppResult};

/// Container/codec information straight from the file, without decoding it.
pub struct Meta {
    pub duration_ms: Option<i64>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub channels: Option<i64>,
    pub bitrate: Option<i64>,
    pub codec: Option<String>,
    pub container: Option<String>,
}

pub fn read_meta(path: &Path) -> AppResult<Meta> {
    let tagged = Probe::open(path)
        .map_err(|e| AppError::Probe(format!("{}：{e}", path.display())))?
        .guess_file_type()
        .map_err(|e| AppError::Probe(format!("{}：{e}", path.display())))?
        .read()
        .map_err(|e| AppError::Probe(format!("{}：{e}", path.display())))?;

    let props = tagged.properties();
    let codec = describe_codec(tagged.file_type());
    let container = format!("{:?}", tagged.file_type());

    Ok(Meta {
        duration_ms: Some(props.duration().as_millis() as i64),
        sample_rate: props.sample_rate().map(i64::from),
        bit_depth: props.bit_depth().map(|d| i64::from(d)),
        channels: props.channels().map(|c| c as i64),
        // lofty reports kbps; the UI divides by 1000, so store bits per second.
        bitrate: props
            .audio_bitrate()
            .or(props.overall_bitrate())
            .map(|k| k as i64 * 1000),
        codec,
        container: Some(container),
    })
}

fn describe_codec(ty: FileType) -> Option<String> {
    let s = match ty {
        FileType::Mpeg => "MPEG audio (MP1/MP2/MP3)",
        FileType::Flac => "FLAC",
        FileType::Vorbis => "Vorbis",
        FileType::Opus => "Opus",
        FileType::Speex => "Speex",
        FileType::Mp4 => "AAC / ALAC (MP4)",
        FileType::Aac => "ADTS AAC",
        FileType::Wav => "PCM / ADPCM (WAV)",
        FileType::Aiff => "PCM (AIFF)",
        FileType::Ape => "APE",
        FileType::Mpc => "Musepack",
        FileType::WavPack => "WavPack",
        FileType::Custom(name) => return Some(name.to_string()),
        _ => return None,
    };
    Some(s.to_string())
}

pub struct Wave {
    /// Interleaved (min, max) pairs normalised to full scale.
    pub peaks: Vec<f32>,
    pub peak_db: f64,
    pub rms_db: f64,
    pub clipped: bool,
}

const BLOCK: usize = 512;

/// Decodes the whole file once to produce the waveform envelope and loudness
/// figures. The accumulator uses fixed sample blocks and is resampled to the
/// requested bucket count at the end, so no prior knowledge of the duration is
/// needed.
pub fn read_wave(path: &Path, buckets: usize) -> AppResult<Wave> {
    let file = File::open(path).map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|_| AppError::Unsupported)?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or(AppError::Unsupported)?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(AppError::Unsupported)?;
    let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(1).max(1);

    let mut codecs = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut codecs);
    codecs.register_audio_decoder::<symphonia_adapter_opus_rs::OpusAdapter>();
    let mut decoder = codecs
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|_| AppError::Unsupported)?;

    let track_id = track.id;
    let mut blocks: Vec<(f32, f32)> = Vec::new();
    let mut cur_min = f32::MAX;
    let mut cur_max = f32::MAX;
    let mut cur_count = 0usize;
    let mut filled = 0usize;
    let mut square_sum = 0f64;
    let mut sample_count = 0f64;
    let mut abs_max = 0f32;
    let mut scratch: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(symphonia::core::errors::Error::ResetRequired) => break,
            Err(_) => break,
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let n = buf.samples_interleaved();
                scratch.resize(n, 0.0);
                buf.copy_to_slice_interleaved(&mut scratch);
                let samples = n / channels;
                for frame in 0..samples {
                    let base = frame * channels;
                    let mut v = 0f32;
                    for c in 0..channels {
                        let s = scratch[base + c];
                        v += s;
                        let a = s.abs();
                        if a > abs_max {
                            abs_max = a;
                        }
                        square_sum += (s as f64) * (s as f64);
                        sample_count += 1.0;
                    }
                    v /= channels as f32;
                    if cur_count == 0 || v < cur_min {
                        cur_min = v;
                    }
                    if cur_count == 0 || v > cur_max {
                        cur_max = v;
                    }
                    cur_count += 1;
                    filled += 1;
                    if filled >= BLOCK {
                        blocks.push((cur_min, cur_max));
                        cur_min = f32::MAX;
                        cur_max = f32::MAX;
                        cur_count = 0;
                        filled = 0;
                    }
                }
            }
            // A damaged frame must not invalidate the rest of the file.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => continue,
        }
    }
    if filled > 0 {
        blocks.push((cur_min, cur_max));
    }

    if blocks.is_empty() {
        return Err(AppError::Unsupported);
    }

    let target = buckets.clamp(32, 4096);
    let mut peaks = Vec::with_capacity(target * 2);
    for i in 0..target {
        let lo = i * blocks.len() / target;
        let hi = ((i + 1) * blocks.len() / target).max(lo + 1).min(blocks.len());
        let (mut mn, mut mx) = (1.0f32, -1.0f32);
        for &(bmin, bmax) in &blocks[lo..hi] {
            if bmin < mn {
                mn = bmin;
            }
            if bmax > mx {
                mx = bmax;
            }
        }
        peaks.push(mn);
        peaks.push(mx);
    }

    let peak_db = db_of(abs_max as f64);
    let rms_db = if sample_count > 0.0 {
        db_of((square_sum / sample_count).sqrt())
    } else {
        f64::NEG_INFINITY
    };

    Ok(Wave {
        peaks,
        peak_db,
        rms_db,
        clipped: abs_max >= 0.999,
    })
}

fn db_of(amplitude: f64) -> f64 {
    if amplitude <= 0.0 {
        -99.0
    } else {
        (20.0 * amplitude.log10()).max(-99.0)
    }
}
