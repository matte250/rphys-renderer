//! Event-driven offline audio engine.
//!
//! Provides [`OfflineAudioMixer`] for buffer-based, headless audio mixing during
//! export. No real-time playback is performed — all audio is rendered into a flat
//! `Vec<f32>` PCM buffer that can be written to a WAV file via [`OfflineAudioMixer::write_wav`].
//!
//! # Example
//!
//! ```rust,no_run
//! use rphys_audio::{AudioEvent, OfflineAudioMixer};
//! use std::path::PathBuf;
//!
//! let mut mixer = OfflineAudioMixer::new(44100, 1);
//! mixer.add_event(AudioEvent {
//!     timestamp_secs: 0.5,
//!     path: PathBuf::from("bounce.wav"),
//!     volume: 0.8,
//! });
//! let samples = mixer.mix(2.0); // 2 seconds of PCM
//! ```

use std::path::{Path, PathBuf};

use thiserror::Error;

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that can occur in the audio engine.
#[derive(Debug, Error)]
pub enum AudioError {
    /// The requested audio file does not exist on disk.
    #[error("Sound file not found: '{path}'")]
    FileNotFound { path: PathBuf },

    /// The audio file exists but could not be decoded.
    #[error("Failed to decode audio file '{path}': {reason}")]
    DecodeFailed { path: PathBuf, reason: String },

    /// Writing the output WAV file failed.
    #[error("Failed to write WAV: {0}")]
    WavWriteFailed(String),
}

// ── AudioEvent ────────────────────────────────────────────────────────────────

/// A scheduled sound event: play `path` at `timestamp_secs` into the mix.
///
/// The `volume` field is a linear scalar applied to all samples (0.0 = silent,
/// 1.0 = full amplitude). Values above 1.0 are permitted but may clip.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioEvent {
    /// Physics-time offset at which this sound starts (seconds).
    pub timestamp_secs: f32,
    /// Path to the WAV file to play.
    pub path: PathBuf,
    /// Linear volume scalar.
    pub volume: f32,
}

// ── AudioBuffer ───────────────────────────────────────────────────────────────

/// A PCM audio buffer ready for export or WAV writing.
#[derive(Debug, Default, Clone)]
pub struct AudioBuffer {
    /// Sample rate in Hz (e.g. 44100).
    pub sample_rate: u32,
    /// Number of interleaved channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Interleaved f32 PCM samples.
    pub samples: Vec<f32>,
}

impl AudioBuffer {
    /// Duration of the buffer in seconds.
    pub fn duration(&self) -> f32 {
        if self.sample_rate == 0 || self.channels == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / (self.sample_rate as f32 * self.channels as f32)
    }

    /// Write the buffer to a WAV file at `path`.
    pub fn write_wav(&self, path: &Path) -> Result<(), AudioError> {
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        let mut writer = hound::WavWriter::create(path, spec)
            .map_err(|e| AudioError::WavWriteFailed(e.to_string()))?;

        for &sample in &self.samples {
            writer
                .write_sample(sample)
                .map_err(|e| AudioError::WavWriteFailed(e.to_string()))?;
        }

        writer
            .finalize()
            .map_err(|e| AudioError::WavWriteFailed(e.to_string()))?;

        Ok(())
    }
}

// ── OfflineAudioMixer ─────────────────────────────────────────────────────────

/// Headless, offline audio mixer suitable for export and CI environments.
///
/// Events are queued via [`add_event`][OfflineAudioMixer::add_event] and
/// rendered into a flat interleaved f32 PCM buffer by
/// [`mix`][OfflineAudioMixer::mix]. Missing audio files are logged to stderr
/// and silently skipped — the mixer never panics on bad input.
///
/// **Note:** Basic sample-rate and channel layout conversion is applied
/// (mono↔stereo, nearest-neighbour rate conversion). Full-quality resampling
/// is out of scope for MVP.
#[derive(Debug)]
pub struct OfflineAudioMixer {
    sample_rate: u32,
    channels: u16,
    events: Vec<AudioEvent>,
}

impl OfflineAudioMixer {
    /// Create a new mixer with the given output format.
    ///
    /// `sample_rate` — output sample rate in Hz (e.g. `44100`).  
    /// `channels`    — number of interleaved channels (1 = mono, 2 = stereo).
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            events: Vec::new(),
        }
    }

    /// Queue an [`AudioEvent`] for mixing.
    ///
    /// Events whose `timestamp_secs` falls at or beyond `duration_secs` passed
    /// to [`mix`] will be ignored during rendering.
    pub fn add_event(&mut self, event: AudioEvent) {
        self.events.push(event);
    }

    /// Render all queued events into a flat interleaved f32 PCM buffer.
    ///
    /// The returned buffer has exactly
    /// `(duration_secs * sample_rate * channels).ceil()` samples.
    /// Silence (`0.0`) fills regions where no event contributes.
    ///
    /// Events that start at or after `duration_secs` are skipped entirely.
    /// Events that start before `duration_secs` but extend beyond it are
    /// clipped at the buffer boundary.
    ///
    /// Missing or unreadable audio files emit a warning to stderr and are
    /// skipped without panicking.
    pub fn mix(&self, duration_secs: f32) -> Vec<f32> {
        let total_samples =
            (duration_secs * self.sample_rate as f32).ceil() as usize * self.channels as usize;
        let mut buffer = vec![0.0_f32; total_samples];

        for event in &self.events {
            // Skip events that begin at or after the requested duration.
            if event.timestamp_secs >= duration_secs {
                continue;
            }

            let file_samples = match read_wav_as_f32(&event.path) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!(
                        "WARN [rphys-audio] skipping event at {:.3}s — {err}",
                        event.timestamp_secs
                    );
                    continue;
                }
            };

            // Convert to our output channel layout.
            let converted =
                convert_channels(file_samples.samples, file_samples.channels, self.channels);

            // Nearest-neighbour sample-rate conversion.
            let resampled = resample(
                converted,
                file_samples.sample_rate,
                self.sample_rate,
                self.channels,
            );

            // Write into the output buffer starting at the event's offset.
            let start_sample =
                (event.timestamp_secs * self.sample_rate as f32) as usize * self.channels as usize;

            for (offset, sample) in resampled.into_iter().enumerate() {
                let idx = start_sample + offset;
                if idx >= buffer.len() {
                    break;
                }
                buffer[idx] += sample * event.volume;
            }
        }

        buffer
    }

    /// Mix all queued events and write the result as a 32-bit float WAV file.
    ///
    /// Creates or overwrites `path`. Parent directories must already exist.
    pub fn write_wav(&self, path: &Path, duration_secs: f32) -> Result<(), AudioError> {
        let samples = self.mix(duration_secs);

        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        let mut writer = hound::WavWriter::create(path, spec)
            .map_err(|e| AudioError::WavWriteFailed(e.to_string()))?;

        for sample in samples {
            writer
                .write_sample(sample)
                .map_err(|e| AudioError::WavWriteFailed(e.to_string()))?;
        }

        writer
            .finalize()
            .map_err(|e| AudioError::WavWriteFailed(e.to_string()))?;

        Ok(())
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Raw decoded audio from a WAV file before channel/rate conversion.
struct RawAudio {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
}

/// Read a WAV file and return all samples as normalised f32.
///
/// Supports 32-bit float, 16-bit int, and other integer depths.
/// Returns [`AudioError`] if the file is missing or cannot be decoded.
fn read_wav_as_f32(path: &Path) -> Result<RawAudio, AudioError> {
    if !path.exists() {
        return Err(AudioError::FileNotFound {
            path: path.to_owned(),
        });
    }

    let mut reader = hound::WavReader::open(path).map_err(|e| AudioError::DecodeFailed {
        path: path.to_owned(),
        reason: e.to_string(),
    })?;

    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| {
                s.map_err(|e| AudioError::DecodeFailed {
                    path: path.to_owned(),
                    reason: e.to_string(),
                })
            })
            .collect::<Result<Vec<f32>, AudioError>>()?,

        hound::SampleFormat::Int => match spec.bits_per_sample {
            16 => reader
                .samples::<i16>()
                .map(|s| {
                    s.map(|v| v as f32 / i16::MAX as f32)
                        .map_err(|e| AudioError::DecodeFailed {
                            path: path.to_owned(),
                            reason: e.to_string(),
                        })
                })
                .collect::<Result<Vec<f32>, AudioError>>()?,
            bits => {
                let max_val = (1_i64 << (bits - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| {
                        s.map(|v| v as f32 / max_val)
                            .map_err(|e| AudioError::DecodeFailed {
                                path: path.to_owned(),
                                reason: e.to_string(),
                            })
                    })
                    .collect::<Result<Vec<f32>, AudioError>>()?
            }
        },
    };

    Ok(RawAudio {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        samples,
    })
}

/// Convert interleaved samples between channel counts.
///
/// Supported conversions:
/// - Same → pass-through  
/// - Mono → Stereo (duplicate each sample)  
/// - Stereo → Mono (average each pair)  
/// - All other mismatches → pass-through with a warning
fn convert_channels(samples: Vec<f32>, from_ch: u16, to_ch: u16) -> Vec<f32> {
    if from_ch == to_ch {
        return samples;
    }

    match (from_ch, to_ch) {
        (1, 2) => {
            // Mono → Stereo: duplicate each sample.
            samples.iter().flat_map(|&s| [s, s]).collect()
        }
        (2, 1) => {
            // Stereo → Mono: average each L/R pair.
            samples
                .chunks_exact(2)
                .map(|pair| (pair[0] + pair[1]) * 0.5)
                .collect()
        }
        _ => {
            eprintln!(
                "WARN [rphys-audio] unsupported channel conversion {from_ch}→{to_ch}, passing through"
            );
            samples
        }
    }
}

/// Nearest-neighbour sample-rate conversion.
///
/// If rates already match the input is returned unchanged.
fn resample(samples: Vec<f32>, from_rate: u32, to_rate: u32, channels: u16) -> Vec<f32> {
    if from_rate == to_rate {
        return samples;
    }

    let from_frames = samples.len() / channels as usize;
    let to_frames = (from_frames as f64 * to_rate as f64 / from_rate as f64) as usize;
    let ch = channels as usize;

    let mut out = Vec::with_capacity(to_frames * ch);

    for out_frame in 0..to_frames {
        let src_frame = (out_frame as f64 * from_rate as f64 / to_rate as f64) as usize;
        let src_frame = src_frame.min(from_frames.saturating_sub(1));
        for c in 0..ch {
            out.push(samples[src_frame * ch + c]);
        }
    }

    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    /// Helper: write a simple 32-bit float mono WAV with a constant sample value.
    fn write_test_wav(
        value: f32,
        num_samples: usize,
        sample_rate: u32,
        channels: u16,
    ) -> NamedTempFile {
        let file = NamedTempFile::new().expect("tempfile");
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(file.path(), spec).expect("WavWriter");
        for _ in 0..num_samples {
            writer.write_sample(value).expect("write_sample");
        }
        writer.finalize().expect("finalize");
        file
    }

    // ── 1. Silence with no events ─────────────────────────────────────────────

    #[test]
    fn test_mix_no_events_is_silence() {
        let mixer = OfflineAudioMixer::new(44100, 1);
        let buf = mixer.mix(1.0);
        // Expected: 44100 samples, all zero.
        assert_eq!(buf.len(), 44100);
        assert!(buf.iter().all(|&s| s == 0.0), "expected all silence");
    }

    #[test]
    fn test_mix_no_events_correct_length_stereo() {
        let mixer = OfflineAudioMixer::new(44100, 2);
        let buf = mixer.mix(2.0);
        // 2 seconds × 44100 Hz × 2 channels = 176400 samples.
        assert_eq!(buf.len(), 176400);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    // ── 2. One event at t=0 → samples present at start ───────────────────────

    #[test]
    fn test_mix_one_event_at_t0_has_samples() {
        let wav = write_test_wav(0.5, 100, 44100, 1);
        let mut mixer = OfflineAudioMixer::new(44100, 1);
        mixer.add_event(AudioEvent {
            timestamp_secs: 0.0,
            path: wav.path().to_path_buf(),
            volume: 1.0,
        });
        let buf = mixer.mix(1.0);

        // First 100 samples should be 0.5 (the test WAV value).
        assert_eq!(buf.len(), 44100);
        for (i, &sample) in buf.iter().enumerate().take(100) {
            assert!(
                (sample - 0.5).abs() < 1e-5,
                "sample[{i}] = {sample} (expected ~0.5)",
            );
        }
        // Everything beyond the event should be silent.
        assert!(buf[100..].iter().all(|&s| s == 0.0));
    }

    // ── 3. Event past duration is ignored ────────────────────────────────────

    #[test]
    fn test_mix_event_past_duration_is_ignored() {
        let wav = write_test_wav(1.0, 100, 44100, 1);
        let mut mixer = OfflineAudioMixer::new(44100, 1);
        // Event starts at exactly duration — should be skipped.
        mixer.add_event(AudioEvent {
            timestamp_secs: 1.0,
            path: wav.path().to_path_buf(),
            volume: 1.0,
        });
        let buf = mixer.mix(1.0);

        assert_eq!(buf.len(), 44100);
        assert!(
            buf.iter().all(|&s| s == 0.0),
            "event past duration must be silent"
        );
    }

    #[test]
    fn test_mix_event_well_past_duration_is_ignored() {
        let wav = write_test_wav(1.0, 100, 44100, 1);
        let mut mixer = OfflineAudioMixer::new(44100, 1);
        mixer.add_event(AudioEvent {
            timestamp_secs: 5.0,
            path: wav.path().to_path_buf(),
            volume: 1.0,
        });
        let buf = mixer.mix(1.0);

        assert!(buf.iter().all(|&s| s == 0.0));
    }

    // ── 4. write_wav produces a valid WAV file ────────────────────────────────

    #[test]
    fn test_write_wav_produces_valid_file() {
        let out = NamedTempFile::new().expect("tempfile");
        let mixer = OfflineAudioMixer::new(44100, 1);
        mixer
            .write_wav(out.path(), 0.5)
            .expect("write_wav should succeed");

        // Re-open and verify the WAV is readable with correct metadata.
        let mut reader = hound::WavReader::open(out.path()).expect("re-open wav");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 44100);
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_format, hound::SampleFormat::Float);

        let samples: Vec<f32> = reader
            .samples::<f32>()
            .map(|s| s.expect("sample"))
            .collect();
        // 0.5s × 44100 Hz × 1 channel = 22050 samples (with ceil = 22050).
        assert_eq!(samples.len(), 22050);
        assert!(samples.iter().all(|&s| s == 0.0), "empty mixer → silence");
    }

    #[test]
    fn test_write_wav_with_event_has_non_zero_samples() {
        let wav = write_test_wav(0.75, 200, 44100, 1);
        let out = NamedTempFile::new().expect("tempfile");

        let mut mixer = OfflineAudioMixer::new(44100, 1);
        mixer.add_event(AudioEvent {
            timestamp_secs: 0.0,
            path: wav.path().to_path_buf(),
            volume: 1.0,
        });
        mixer.write_wav(out.path(), 1.0).expect("write_wav");

        let mut reader = hound::WavReader::open(out.path()).expect("re-open");
        let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();

        // First 200 samples should be ~0.75.
        for (i, &sample) in samples.iter().enumerate().take(200) {
            assert!(
                (sample - 0.75).abs() < 1e-5,
                "sample[{i}] = {sample} (expected ~0.75)",
            );
        }
    }

    // ── 5. Missing audio file is skipped without panic ────────────────────────

    #[test]
    fn test_missing_file_does_not_panic() {
        let mut mixer = OfflineAudioMixer::new(44100, 1);
        mixer.add_event(AudioEvent {
            timestamp_secs: 0.0,
            path: PathBuf::from("/this/file/does/not/exist.wav"),
            volume: 1.0,
        });
        // Must not panic — missing file is silently skipped.
        let buf = mixer.mix(1.0);
        assert_eq!(buf.len(), 44100);
        assert!(
            buf.iter().all(|&s| s == 0.0),
            "missing file should yield silence"
        );
    }

    #[test]
    fn test_missing_file_write_wav_does_not_panic() {
        let out = NamedTempFile::new().expect("tempfile");
        let mut mixer = OfflineAudioMixer::new(44100, 1);
        mixer.add_event(AudioEvent {
            timestamp_secs: 0.0,
            path: PathBuf::from("/nonexistent/audio.wav"),
            volume: 1.0,
        });
        mixer
            .write_wav(out.path(), 1.0)
            .expect("should succeed with missing file skipped");
    }

    // ── 6. Volume scaling ─────────────────────────────────────────────────────

    #[test]
    fn test_volume_is_applied() {
        let wav = write_test_wav(1.0, 10, 44100, 1);
        let mut mixer = OfflineAudioMixer::new(44100, 1);
        mixer.add_event(AudioEvent {
            timestamp_secs: 0.0,
            path: wav.path().to_path_buf(),
            volume: 0.5,
        });
        let buf = mixer.mix(1.0);
        for (i, &sample) in buf.iter().enumerate().take(10) {
            assert!(
                (sample - 0.5).abs() < 1e-5,
                "sample[{i}] should be 0.5 (1.0 × 0.5 volume)"
            );
        }
    }

    // ── 7. AudioBuffer helpers ────────────────────────────────────────────────

    #[test]
    fn test_audio_buffer_duration() {
        let buf = AudioBuffer {
            sample_rate: 44100,
            channels: 2,
            samples: vec![0.0; 88200], // 1 second of stereo
        };
        assert!((buf.duration() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_audio_buffer_write_wav_roundtrip() {
        let out = NamedTempFile::new().expect("tempfile");
        let buf = AudioBuffer {
            sample_rate: 48000,
            channels: 1,
            samples: vec![0.1, 0.2, 0.3],
        };
        buf.write_wav(out.path()).expect("write_wav");

        let mut reader = hound::WavReader::open(out.path()).expect("re-open");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 48000);
        let read_back: Vec<f32> = reader.samples::<f32>().map(|s| s.unwrap()).collect();
        assert_eq!(read_back.len(), 3);
        assert!((read_back[1] - 0.2).abs() < 1e-5);
    }

    // ── 8. Channel conversion helpers ────────────────────────────────────────

    #[test]
    fn test_convert_mono_to_stereo() {
        let mono = vec![0.1, 0.2, 0.3];
        let stereo = convert_channels(mono, 1, 2);
        assert_eq!(stereo, vec![0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
    }

    #[test]
    fn test_convert_stereo_to_mono() {
        let stereo = vec![0.0, 1.0, 0.0, 0.5];
        let mono = convert_channels(stereo, 2, 1);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 1e-5); // (0.0 + 1.0) / 2
        assert!((mono[1] - 0.25).abs() < 1e-5); // (0.0 + 0.5) / 2
    }

    // ── 9. Stereo mixer with mono WAV event ───────────────────────────────────

    #[test]
    fn test_stereo_mixer_with_mono_wav() {
        let wav = write_test_wav(0.6, 10, 44100, 1);
        let mut mixer = OfflineAudioMixer::new(44100, 2);
        mixer.add_event(AudioEvent {
            timestamp_secs: 0.0,
            path: wav.path().to_path_buf(),
            volume: 1.0,
        });
        let buf = mixer.mix(1.0);
        assert_eq!(buf.len(), 44100 * 2);
        // First 10 stereo frames = 20 samples, all ~0.6.
        for (i, &sample) in buf.iter().enumerate().take(20) {
            assert!(
                (sample - 0.6).abs() < 1e-5,
                "buf[{i}] = {sample} expected ~0.6",
            );
        }
    }
}
