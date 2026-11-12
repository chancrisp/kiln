/// A run of interleaved `f32` samples. Integers up to 24 bits survive `f32` exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub channels: u16,
    pub rate: u32,
    pub samples: Vec<f32>,
}

impl Block {
    /// Number of frames (one sample per channel each).
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }
}

/// The audio codec of a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    Pcm { bits: u16, float: bool },
    Mp3,
    Aac,
    HeAac,
    Alac,
    Flac,
    Vorbis,
    Opus,
    Wma,
    Ac3,
    WavPack,
    Other(String),
}

impl Codec {
    /// The name shown to the owner, e.g. "MP3", or "DTS" for `Other("dts")`.
    pub fn display_name(&self) -> String {
        match self {
            Codec::Pcm { .. } => "PCM".into(),
            Codec::Mp3 => "MP3".into(),
            Codec::Aac => "AAC".into(),
            Codec::HeAac => "HE-AAC".into(),
            Codec::Alac => "ALAC".into(),
            Codec::Flac => "FLAC".into(),
            Codec::Vorbis => "Vorbis".into(),
            Codec::Opus => "Opus".into(),
            Codec::Wma => "WMA".into(),
            Codec::Ac3 => "AC3".into(),
            Codec::WavPack => "WavPack".into(),
            Codec::Other(s) => match s.to_ascii_lowercase().as_str() {
                "truehd" => "TrueHD".into(),
                "alaw" => "A-LAW".into(),
                "mulaw" => "\u{3bc}-LAW".into(),
                // For a codec nothing can name: "audio is an unknown format".
                "unknown" => "an unknown format".into(),
                _ => s.to_uppercase(),
            },
        }
    }
}

/// Channel count plus the WAVEFORMATEXTENSIBLE speaker mask (0 when unknown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channels {
    pub count: u16,
    pub mask: u32,
}

/// The speaker mask for the common layouts, 0 when unknown. The one place the
/// crate says which speakers a bare channel count means: 7 channels is 6.1
/// (front left and right, centre, LFE, back centre, side left and right).
/// Public only for the quality harness; not a stable API.
#[doc(hidden)]
pub fn default_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        3 => 0x7,
        4 => 0x33,
        5 => 0x37,
        6 => 0x3F,
        7 => 0x70F,
        8 => 0x63F,
        _ => 0,
    }
}

/// Embedded cover art.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Metadata tags carried between formats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    /// (number, total)
    pub track: Option<(u32, Option<u32>)>,
    /// (number, total)
    pub disc: Option<(u32, Option<u32>)>,
    pub year: Option<String>,
    pub picture: Option<Picture>,
}

/// What a source file turned out to hold.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceInfo {
    pub codec: Codec,
    pub rate: u32,
    pub channels: Channels,
    pub bits: Option<u16>,
    pub frames: Option<u64>,
    pub kbps: Option<u32>,
    pub tags: Tags,
}

impl SourceInfo {
    /// Length in seconds, when the frame count is known.
    pub fn duration_secs(&self) -> Option<f64> {
        match (self.frames, self.rate) {
            (Some(f), r) if r > 0 => Some(f as f64 / r as f64),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_info() -> SourceInfo {
        SourceInfo {
            codec: Codec::Flac,
            rate: 44100,
            channels: Channels {
                count: 2,
                mask: 0x3,
            },
            bits: Some(16),
            frames: None,
            kbps: None,
            tags: Tags::default(),
        }
    }

    #[test]
    fn block_counts_frames() {
        let b = Block {
            channels: 2,
            rate: 48000,
            samples: vec![0.0; 16],
        };
        assert_eq!(b.frames(), 8);
    }

    #[test]
    fn duration_from_frames() {
        let i = SourceInfo {
            frames: Some(96000),
            rate: 48000,
            ..test_info()
        };
        assert_eq!(i.duration_secs(), Some(2.0));
    }

    #[test]
    fn duration_unknown_without_frames_or_rate() {
        assert_eq!(test_info().duration_secs(), None);
        let i = SourceInfo {
            frames: Some(10),
            rate: 0,
            ..test_info()
        };
        assert_eq!(i.duration_secs(), None);
    }

    #[test]
    fn other_codec_names_upper_case() {
        assert_eq!(Codec::Other("dts".into()).display_name(), "DTS");
        assert_eq!(Codec::Other("truehd".into()).display_name(), "TrueHD");
    }

    #[test]
    fn companded_and_unknown_codecs_read_properly() {
        assert_eq!(Codec::Other("alaw".into()).display_name(), "A-LAW");
        assert_eq!(Codec::Other("mulaw".into()).display_name(), "\u{3bc}-LAW");
        assert_eq!(
            Codec::Other("unknown".into()).display_name(),
            "an unknown format"
        );
    }

    #[test]
    fn named_codecs_use_plain_names() {
        assert_eq!(Codec::HeAac.display_name(), "HE-AAC");
        assert_eq!(Codec::WavPack.display_name(), "WavPack");
        assert_eq!(
            Codec::Pcm {
                bits: 24,
                float: false
            }
            .display_name(),
            "PCM"
        );
    }

    #[test]
    fn default_masks_name_the_standard_layouts() {
        assert_eq!(default_mask(1), 0x4);
        assert_eq!(default_mask(2), 0x3);
        assert_eq!(default_mask(6), 0x3F);
        // 7 channels is 6.1: FL FR FC LFE BC SL SR.
        assert_eq!(default_mask(7), 0x70F);
        assert_eq!(default_mask(8), 0x63F);
        assert_eq!(default_mask(0), 0);
        assert_eq!(default_mask(9), 0);
        // Every count with a mask names exactly that many speakers.
        for n in 1..=8u16 {
            assert_eq!(default_mask(n).count_ones(), n as u32);
        }
    }
}
