use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use ape_decoder::ApeDecoder;
use lofty::file::FileType;
use lofty::prelude::*;
use lofty::probe::Probe;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
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
    match read_meta_lofty(path) {
        Ok(meta) => Ok(meta),
        Err(err) => {
            // Lofty refuses some containers it cannot fully parse (odd or
            // slightly damaged headers) even though the audio decodes fine, so
            // the data the UI needs is recovered with the decoders instead.
            let ext = extension(path);
            if ext == "ape" {
                return meta_from_ape(path).map_err(|_| err);
            }
            if symphonia_exts(&ext) {
                return meta_from_symphonia(path, &ext).map_err(|_| err);
            }
            Err(err)
        }
    }
}

fn read_meta_lofty(path: &Path) -> AppResult<Meta> {
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
        FileType::Ape => "Monkey's Audio (APE)",
        FileType::Mpc => "Musepack",
        FileType::WavPack => "WavPack",
        FileType::Custom(name) => return Some(name.to_string()),
        _ => return None,
    };
    Some(s.to_string())
}

/// Reads the same figures without decoding, via symphonia's demuxers. Falls
/// back to the embedded MPEG stream for RIFF files that only carry MP3 frames.
fn meta_from_symphonia(path: &Path, ext: &str) -> AppResult<Meta> {
    let file = File::open(path).map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
    match probe_meta(file, ext) {
        Ok(meta) => Ok(meta),
        Err(err) => {
            if riff_exts(ext) {
                if let Some(span) = embedded_mpeg_span(path) {
                    if let Ok(slice) = FileSlice::open(path, span) {
                        if let Ok(mut meta) = probe_meta(slice, "mp3") {
                            meta.container = Some("Wav".to_string());
                            return Ok(meta);
                        }
                    }
                }
            }
            Err(err)
        }
    }
}

fn probe_meta<R: MediaSource + 'static>(reader: R, ext: &str) -> AppResult<Meta> {
    let mss = MediaSourceStream::new(Box::new(reader), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(ext);

    let format = symphonia::default::get_probe()
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

    let sample_rate = params.sample_rate.map(i64::from);
    let duration_ms = track
        .duration
        .and_then(|dur| track.time_base.and_then(|tb| tb.calc_duration(dur)))
        .map(|t| t.as_millis() as i64)
        .or_else(|| match (track.num_frames, params.sample_rate) {
            (Some(frames), Some(rate)) if rate > 0 => {
                Some((frames as f64 * 1000.0 / rate as f64).round() as i64)
            }
            _ => None,
        });

    // Bit depth only means something for PCM-like containers; codecs like MP3
    // report a decoded sample width that would read as a made-up figure.
    let pcm = riff_exts(ext) || matches!(ext, "aif" | "aiff" | "aifc" | "caf" | "flac");
    let (codec, container) = describe_symphonia(ext);
    Ok(Meta {
        duration_ms,
        sample_rate,
        bit_depth: if pcm {
            params.bits_per_sample.map(|b| b as i64)
        } else {
            None
        },
        channels: params.channels.as_ref().map(|c| c.count() as i64),
        bitrate: None,
        codec,
        container,
    })
}

fn describe_symphonia(ext: &str) -> (Option<String>, Option<String>) {
    let (codec, container) = match ext {
        "mp1" | "mp2" | "mp3" | "mpga" => ("MPEG audio (MP1/MP2/MP3)", "Mpeg"),
        "wav" | "wave" | "bwf" | "w64" | "rf64" | "bw64" => ("PCM / ADPCM (WAV)", "Wav"),
        "flac" => ("FLAC", "Flac"),
        "ogg" | "oga" => ("Vorbis / Opus (Ogg)", "Ogg"),
        "opus" => ("Opus", "Ogg"),
        "spx" => ("Speex", "Ogg"),
        "m4a" | "m4b" | "m4p" | "m4r" | "mp4" => ("AAC / ALAC (MP4)", "Mp4"),
        "aac" | "adts" => ("ADTS AAC", "Aac"),
        "aif" | "aiff" | "aifc" => ("PCM (AIFF)", "Aiff"),
        "caf" => ("PCM (CAF)", "Caf"),
        "mka" | "mkv" | "webm" => ("Matroska audio", "Mkv"),
        "3gp" => ("AAC (3GP)", "Mp4"),
        _ => return (None, None),
    };
    (Some(codec.to_string()), Some(container.to_string()))
}

fn meta_from_ape(path: &Path) -> AppResult<Meta> {
    let file = File::open(path).map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
    let decoder = ApeDecoder::new(BufReader::new(file))
        .map_err(|e| AppError::Probe(format!("{}：{e}", path.display())))?;
    let info = decoder.info();
    Ok(Meta {
        duration_ms: Some(info.duration_ms as i64),
        sample_rate: Some(i64::from(info.sample_rate)),
        bit_depth: Some(i64::from(info.bits_per_sample)),
        channels: Some(i64::from(info.channels)),
        bitrate: (info.average_bitrate_kbps > 0)
            .then(|| i64::from(info.average_bitrate_kbps) * 1000),
        codec: Some("Monkey's Audio (APE)".to_string()),
        container: Some("Ape".to_string()),
    })
}

pub struct Wave {
    /// Interleaved (min, max) pairs normalised to full scale.
    pub peaks: Vec<f32>,
    pub peak_db: f64,
    pub rms_db: f64,
    pub clipped: bool,
}

const BLOCK: usize = 512;

/// Extensions whose data symphonia can actually decode with the features this
/// build enables. Anything else is refused up front: a lenient prober can
/// mistake unrelated bytes (APE, WavPack, WMA) for an MPEG stream and produce a
/// waveform that is pure noise.
fn symphonia_exts(ext: &str) -> bool {
    matches!(
        ext,
        "mp1"
            | "mp2"
            | "mp3"
            | "mpga"
            | "wav"
            | "wave"
            | "bwf"
            | "w64"
            | "rf64"
            | "bw64"
            | "flac"
            | "ogg"
            | "oga"
            | "opus"
            | "m4a"
            | "m4b"
            | "m4p"
            | "m4r"
            | "mp4"
            | "aac"
            | "adts"
            | "aif"
            | "aiff"
            | "aifc"
            | "caf"
            | "mka"
            | "mkv"
            | "webm"
            | "3gp"
    )
}

pub(crate) fn riff_exts(ext: &str) -> bool {
    matches!(ext, "wav" | "wave" | "bwf" | "w64" | "rf64" | "bw64")
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Decodes the whole file once to produce the waveform envelope and loudness
/// figures. The accumulator uses fixed sample blocks and is resampled to the
/// requested bucket count at the end, so no prior knowledge of the duration is
/// needed.
pub fn read_wave(path: &Path, buckets: usize) -> AppResult<Wave> {
    let ext = extension(path);
    if ext == "ape" {
        return read_wave_ape(path, buckets);
    }
    if !symphonia_exts(&ext) {
        return Err(AppError::Unsupported);
    }
    let file = File::open(path).map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
    match decode_symphonia(file, &ext, buckets) {
        Ok(wave) => Ok(wave),
        Err(err) => {
            // Some WAV files carry MP3 frames instead of PCM. The RIFF reader
            // rejects them, but the frames themselves decode once the header is
            // skipped.
            if riff_exts(&ext) {
                if let Some(span) = embedded_mpeg_span(path) {
                    if let Ok(slice) = FileSlice::open(path, span) {
                        if let Ok(wave) = decode_symphonia(slice, "mp3", buckets) {
                            return Ok(wave);
                        }
                    }
                }
            }
            Err(err)
        }
    }
}

fn decode_symphonia<R: MediaSource + 'static>(reader: R, ext: &str, buckets: usize) -> AppResult<Wave> {
    let mss = MediaSourceStream::new(Box::new(reader), Default::default());

    let mut hint = Hint::new();
    hint.with_extension(ext);

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
    let mut acc = WaveAccum::new();
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
                for frame in scratch.chunks_exact(channels) {
                    acc.push(frame);
                }
            }
            // A damaged frame must not invalidate the rest of the file.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => continue,
        }
    }

    acc.finish(buckets).ok_or(AppError::Unsupported)
}

/// Monkey's Audio is not part of symphonia, so it gets its own streaming
/// decoder. Passing an APE file to symphonia would "succeed" as noise.
fn read_wave_ape(path: &Path, buckets: usize) -> AppResult<Wave> {
    let file = File::open(path).map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
    let mut decoder = ApeDecoder::new(BufReader::new(file))
        .map_err(|e| AppError::Probe(format!("{}：{e}", path.display())))?;
    let info = decoder.info().clone();
    let channels = info.channels.max(1) as usize;
    let sample_bytes = info.bytes_per_sample as usize;
    let frame_bytes = sample_bytes * channels;
    if frame_bytes == 0 {
        return Err(AppError::Unsupported);
    }

    let mut acc = WaveAccum::new();
    let mut frame: Vec<f32> = Vec::with_capacity(channels);
    for idx in 0..info.total_frames {
        let pcm = decoder
            .decode_frame(idx)
            .map_err(|e| AppError::Probe(format!("{}：{e}", path.display())))?;
        for chunk in pcm.chunks_exact(frame_bytes) {
            frame.clear();
            for ch in 0..channels {
                let bytes = &chunk[ch * sample_bytes..(ch + 1) * sample_bytes];
                frame.push(pcm_sample(
                    bytes,
                    info.is_floating_point,
                    info.is_big_endian,
                    info.is_signed_8bit,
                ));
            }
            acc.push(&frame);
        }
    }

    acc.finish(buckets).ok_or(AppError::Unsupported)
}

/// One PCM sample of any depth the APE container can hold, as full-scale float.
pub(crate) fn pcm_sample(bytes: &[u8], float: bool, big_endian: bool, signed8: bool) -> f32 {
    match bytes.len() {
        1 => {
            if signed8 {
                bytes[0] as i8 as f32 / 128.0
            } else {
                (bytes[0] as f32 - 128.0) / 128.0
            }
        }
        2 => {
            let v = if big_endian {
                i16::from_be_bytes([bytes[0], bytes[1]])
            } else {
                i16::from_le_bytes([bytes[0], bytes[1]])
            };
            v as f32 / 32768.0
        }
        3 => {
            let v = if big_endian {
                ((bytes[0] as i32) << 16) | ((bytes[1] as i32) << 8) | bytes[2] as i32
            } else {
                (bytes[0] as i32) | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16)
            };
            ((v << 8) >> 8) as f32 / 8_388_608.0
        }
        4 => {
            let raw = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if float {
                if big_endian {
                    f32::from_be_bytes(raw)
                } else {
                    f32::from_le_bytes(raw)
                }
            } else {
                let v = if big_endian {
                    i32::from_be_bytes(raw)
                } else {
                    i32::from_le_bytes(raw)
                };
                v as f32 / 2_147_483_648.0
            }
        }
        _ => 0.0,
    }
}

struct WaveAccum {
    blocks: Vec<(f32, f32)>,
    min: f32,
    max: f32,
    filled: usize,
    square_sum: f64,
    sample_count: f64,
    abs_max: f32,
}

impl WaveAccum {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
            filled: 0,
            square_sum: 0.0,
            sample_count: 0.0,
            abs_max: 0.0,
        }
    }

    fn push(&mut self, frame: &[f32]) {
        if frame.is_empty() {
            return;
        }
        let mut sum = 0f32;
        for &s in frame {
            sum += s;
            let a = s.abs();
            if a > self.abs_max {
                self.abs_max = a;
            }
            self.square_sum += (s as f64) * (s as f64);
            self.sample_count += 1.0;
        }
        let v = sum / frame.len() as f32;
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
        self.filled += 1;
        if self.filled >= BLOCK {
            self.blocks.push((self.min, self.max));
            self.min = f32::INFINITY;
            self.max = f32::NEG_INFINITY;
            self.filled = 0;
        }
    }

    fn finish(mut self, buckets: usize) -> Option<Wave> {
        if self.filled > 0 {
            self.blocks.push((self.min, self.max));
        }
        if self.blocks.is_empty() {
            return None;
        }

        let target = buckets.clamp(32, 4096);
        let mut peaks = Vec::with_capacity(target * 2);
        for i in 0..target {
            let lo = i * self.blocks.len() / target;
            let hi = ((i + 1) * self.blocks.len() / target).max(lo + 1).min(self.blocks.len());
            let (mut mn, mut mx) = (1.0f32, -1.0f32);
            for &(bmin, bmax) in &self.blocks[lo..hi] {
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

        let peak_db = db_of(self.abs_max as f64);
        let rms_db = if self.sample_count > 0.0 {
            db_of((self.square_sum / self.sample_count).sqrt())
        } else {
            f64::NEG_INFINITY
        };

        Some(Wave {
            peaks,
            peak_db,
            rms_db,
            clipped: self.abs_max >= 0.999,
        })
    }
}

/// A read+seek window over a byte range of a file. Symphonia and rodio both
/// need `Seek`, so an embedded stream cannot be fed from a plain pipe.
pub(crate) struct FileSlice {
    file: File,
    start: u64,
    len: u64,
    pos: u64,
}

impl FileSlice {
    pub(crate) fn open(path: &Path, (start, len): (u64, u64)) -> AppResult<Self> {
        let file = File::open(path).map_err(|e| AppError::Io(format!("{}：{e}", path.display())))?;
        Ok(Self { file, start, len, pos: 0 })
    }
}

impl MediaSource for FileSlice {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

impl Read for FileSlice {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        if remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let take = buf.len().min(remaining as usize);
        self.file.seek(SeekFrom::Start(self.start + self.pos))?;
        let n = self.file.read(&mut buf[..take])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for FileSlice {
    fn seek(&mut self, target: SeekFrom) -> std::io::Result<u64> {
        let absolute = match target {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.pos as i128 + n as i128,
            SeekFrom::End(n) => self.len as i128 + n as i128,
        };
        if absolute < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start of the slice",
            ));
        }
        self.pos = absolute.min(self.len as i128) as u64;
        Ok(self.pos)
    }
}

/// Finds the `data` payload of a RIFF/WAVE file whose `fmt ` chunk declares
/// MPEG audio (format tag 0x0055, directly or through WAVE_FORMAT_EXTENSIBLE).
pub(crate) fn embedded_mpeg_span(path: &Path) -> Option<(u64, u64)> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();

    let mut riff = [0u8; 12];
    file.read_exact(&mut riff).ok()?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return None;
    }

    let mut mpeg = false;
    for _ in 0..64 {
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            return None;
        }
        let id = &header[0..4];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
        let body = file.stream_position().ok()?;
        let padded = size + (size & 1);

        if id == b"fmt " {
            let mut fmt = [0u8; 64];
            let take = size.min(64) as usize;
            if file.read_exact(&mut fmt[..take]).is_err() {
                return None;
            }
            if take >= 2 {
                let tag = u16::from_le_bytes([fmt[0], fmt[1]]);
                mpeg = tag == 0x0055
                    || (tag == 0xFFFE && take >= 26 && u16::from_le_bytes([fmt[24], fmt[25]]) == 0x0055);
            }
            file.seek(SeekFrom::Start(body + padded)).ok()?;
        } else if id == b"data" {
            if !mpeg {
                return None;
            }
            let len = if size == 0 || size == u64::from(u32::MAX) || body + size > file_len {
                file_len.checked_sub(body)?
            } else {
                size
            };
            return Some((body, len));
        } else {
            if body + padded > file_len {
                return None;
            }
            file.seek(SeekFrom::Start(body + padded)).ok()?;
        }
    }
    None
}

fn db_of(amplitude: f64) -> f64 {
    if amplitude <= 0.0 {
        -99.0
    } else {
        (20.0 * amplitude.log10()).max(-99.0)
    }
}
