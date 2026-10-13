use std::path::Path;
use std::sync::Arc;

use crate::error::LoadError;

/// A sound, whole in memory: 16-bit samples at the rate it was recorded at, mono or stereo.
///
/// A sound heard from a place in the world is played as mono (a stereo one's two channels
/// are averaged); one heard everywhere keeps its two channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub name: String,
    /// Frames per second.
    pub rate: u32,
    /// 1 (mono) or 2 (stereo: each frame is its left sample, then its right).
    pub channels: u8,
    /// The samples, frame by frame. Shared, not copied, with whatever is playing it.
    pub samples: Arc<[i16]>,
}

impl Sound {
    /// A mono sound from samples in -1..1 (for sounds made in code, and tests).
    pub fn mono(name: &str, rate: u32, samples: impl IntoIterator<Item = f32>) -> Sound {
        Sound {
            name: name.to_string(),
            rate,
            channels: 1,
            samples: samples.into_iter().map(from_unit).collect(),
        }
    }

    /// How many frames long it is (a frame is one sample per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    /// How long it plays for at its own pitch, in seconds.
    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.rate as f32
    }
}

fn from_unit(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// `WAVE_FORMAT_PCM`, `WAVE_FORMAT_IEEE_FLOAT` and `WAVE_FORMAT_EXTENSIBLE` (whose real
/// format is the first two bytes of its sub-format).
const PCM: u16 = 1;
const FLOAT: u16 = 3;
const EXTENSIBLE: u16 = 0xfffe;

/// Reads a `.wav` file: uncompressed PCM of 8, 16, 24 or 32 bits or 32-bit float, mono or
/// stereo, at any rate. Everything is kept as 16 bits.
pub fn decode_wav(path: &Path, name: &str, bytes: &[u8]) -> Result<Sound, LoadError> {
    let err = |msg: &str| LoadError::new(path, None, msg);
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(err("not a WAV file (no RIFF/WAVE header)"));
    }
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    // (format, channels, rate, bits per sample)
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    // Chunks: a four-letter name, a length, then that many bytes (and one more to make it
    // even). A data chunk cut short by the end of the file is taken as far as it goes.
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32_at(bytes, at + 4) as usize;
        let body = &bytes[at + 8..(at + 8).saturating_add(size).min(bytes.len())];
        match &bytes[at..at + 4] {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(err("its format chunk is too short"));
                }
                let mut tag = u16_at(body, 0);
                if tag == EXTENSIBLE && body.len() >= 26 {
                    tag = u16_at(body, 24);
                }
                format = Some((tag, u16_at(body, 2), u32_at(body, 4), u16_at(body, 14)));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = (at + 8).saturating_add(size + (size & 1));
    }
    let (tag, channels, rate, bits) = format.ok_or_else(|| err("it has no format chunk"))?;
    let data = data.ok_or_else(|| err("it has no data chunk"))?;
    if !(1..=2).contains(&channels) {
        return Err(err("it must be mono or stereo"));
    }
    if rate == 0 {
        return Err(err("its sample rate is 0"));
    }
    let samples: Arc<[i16]> = match (tag, bits) {
        // 8 bits is unsigned, with silence at 128.
        (PCM, 8) => data.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
        (PCM, 16) => data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(),
        (PCM, 24) => data.chunks_exact(3).map(|b| i16::from_le_bytes([b[1], b[2]])).collect(),
        (PCM, 32) => data.chunks_exact(4).map(|b| i16::from_le_bytes([b[2], b[3]])).collect(),
        (FLOAT, 32) => data
            .chunks_exact(4)
            .map(|b| from_unit(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            .collect(),
        _ => {
            return Err(err(
                "unsupported WAV encoding (expected PCM of 8, 16, 24 or 32 bits, or 32-bit float)",
            ));
        }
    };
    // Whole frames only.
    let channels = channels as usize;
    let whole = samples.len() - samples.len() % channels;
    let samples = if whole == samples.len() { samples } else { samples[..whole].into() };
    Ok(Sound {
        name: name.to_string(),
        rate,
        channels: channels as u8,
        samples,
    })
}

/// Writes 16-bit PCM as a `.wav` file's bytes (for tools and tests; the engine only reads).
pub fn encode_wav(rate: u32, channels: u8, samples: &[i16]) -> Vec<u8> {
    let bytes = (samples.len() * 2) as u32;
    let align = channels as u16 * 2;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&PCM.to_le_bytes());
    out.extend_from_slice(&(channels as u16).to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * align as u32).to_le_bytes());
    out.extend_from_slice(&align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Result<Sound, LoadError> {
        decode_wav(Path::new("test.wav"), "test.wav", bytes)
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let samples = [0i16, 1000, -1000, i16::MAX, i16::MIN, 7];
        let sound = decode(&encode_wav(22050, 2, &samples)).unwrap();
        assert_eq!((sound.rate, sound.channels), (22050, 2));
        assert_eq!(&sound.samples[..], &samples);
        assert_eq!(sound.frames(), 3);
    }

    #[test]
    fn other_sample_sizes_become_16_bits() {
        // The header of a 16-bit file, with its format and sizes changed.
        let with = |tag: u16, bits: u16, data: &[u8]| {
            let mut bytes = encode_wav(8000, 1, &[]);
            bytes[20..22].copy_from_slice(&tag.to_le_bytes());
            bytes[34..36].copy_from_slice(&bits.to_le_bytes());
            bytes[40..44].copy_from_slice(&(data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(data);
            bytes
        };
        assert_eq!(&decode(&with(PCM, 8, &[128, 255, 0])).unwrap().samples[..], &[0, 127 << 8, -128 << 8]);
        assert_eq!(&decode(&with(PCM, 24, &[9, 0x34, 0x12])).unwrap().samples[..], &[0x1234]);
        assert_eq!(&decode(&with(PCM, 32, &[9, 9, 0x34, 0x12])).unwrap().samples[..], &[0x1234]);
        let floats: Vec<u8> = [0.5f32, -1.0, 2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(&decode(&with(FLOAT, 32, &floats)).unwrap().samples[..], &[16384, -32767, 32767]);
        assert!(decode(&with(2, 4, &[0; 8])).is_err(), "ADPCM isn't read");
    }

    #[test]
    fn chunks_before_the_data_are_skipped_and_odd_ones_padded() {
        let plain = encode_wav(8000, 1, &[5, -5]);
        let mut bytes = plain[..12].to_vec();
        bytes.extend_from_slice(b"LIST\x03\x00\x00\x00abc\x00");
        bytes.extend_from_slice(&plain[12..]);
        assert_eq!(&decode(&bytes).unwrap().samples[..], &[5, -5]);
    }

    #[test]
    fn files_that_are_not_sounds_say_why() {
        assert!(decode(b"not a wav").unwrap_err().message.contains("RIFF"));
        let mut six = encode_wav(8000, 1, &[0; 6]);
        six[22] = 6;
        assert!(decode(&six).unwrap_err().message.contains("mono or stereo"));
        assert!(decode(&encode_wav(8000, 1, &[])[..36]).unwrap_err().message.contains("data"));
    }
}
