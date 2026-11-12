use thiserror::Error;

/// Everything the audio engine can fail with.
#[derive(Debug, Error)]
pub enum MediaError {
    /// The file is damaged and can't be read. `code` is the upper-case extension.
    #[error("This {code} is damaged and can't be read.")]
    Damaged { code: String },
    /// The file's audio is a codec Kiln can't read yet.
    #[error("This file's audio is {codec}, which Kiln can't read yet.")]
    Unsupported { codec: String },
    /// A video with no audio stream.
    #[error("This video has no sound to convert.")]
    NoAudio,
    /// A file with no video stream.
    #[error("This file has no video to convert.")]
    NoVideo,
    /// Windows' Media Feature Pack (or the codec in it) isn't installed.
    #[error("Needs Windows' Media Feature Pack, which isn't installed.")]
    NeedsMediaFoundation,
    /// Windows can't decode this video's codec until one of Microsoft's
    /// Store extensions is installed. `codec` is "HEVC", "AV1" or "VP9";
    /// `product` is the extension's Store product ID, for the Get it link
    /// (`ms-windows-store://pdp/?ProductId=` and the ID).
    #[error("Needs Microsoft's {codec} Video Extensions to read {codec} video.")]
    NeedsExtension {
        codec: &'static str,
        product: &'static str,
    },
    /// Windows has no decoder for the picture's codec at all ("ProRes",
    /// "MPEG-4"), so only another engine (FFmpeg) can read it.
    #[error("Windows has no decoder for {codec} video.")]
    NoDecoder { codec: String },
    /// Windows can't encode `codec` until Microsoft's Store extension is
    /// installed (HEVC on a PC with no graphics card encoder for it). The
    /// same Store product as `NeedsExtension`, for the Get it link.
    #[error("Needs Microsoft's {codec} Video Extensions to write {codec} video.")]
    NeedsExtensionToWrite {
        codec: &'static str,
        product: &'static str,
    },
    /// The owner chose the graphics card to encode the picture, and this PC
    /// has no graphics card encoder for the codec that works. `codec` is
    /// "H.264" or "HEVC".
    #[error("This PC has no graphics card encoder for {codec}. Choose Automatic or Processor.")]
    NoHardwareEncoder { codec: &'static str },
    /// `lame_enc.dll` couldn't be loaded.
    #[error("Kiln's MP3 encoder is missing. Reinstall Kiln to fix it.")]
    LameMissing,
    /// The target size is below what the format can reach for this file.
    #[error("{format} can't go under {} MB for this file.", fmt_mb(*.min_bytes))]
    TooSmall { format: String, min_bytes: u64 },
    /// The conversion was cancelled.
    #[error("Cancelled.")]
    Cancelled,
    /// Writing (or anything but reading the source) failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Reading the source failed: opening it, or a block of it.
    #[error(transparent)]
    Read(std::io::Error),
    /// An encoder reported a failure.
    #[error("The encoder failed: {0}")]
    Encoder(String),
}

/// Megabytes (1 MB = 1,000,000 bytes), rounded up to one decimal.
pub(crate) fn fmt_mb(bytes: u64) -> String {
    let tenths = bytes.div_ceil(100_000);
    format!("{}.{}", tenths / 10, tenths % 10)
}

impl MediaError {
    /// The same error, with an I/O failure marked as one reading the source.
    pub fn reading(self) -> Self {
        match self {
            MediaError::Io(e) => MediaError::Read(e),
            other => other,
        }
    }
}

pub type Result<T> = std::result::Result<T, MediaError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_marks_only_io_errors() {
        let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(
            MediaError::Io(gone).reading(),
            MediaError::Read(_)
        ));
        assert!(matches!(MediaError::NoAudio.reading(), MediaError::NoAudio));
    }

    #[test]
    fn row_copy_matches_the_spec() {
        assert_eq!(
            MediaError::Damaged { code: "MP3".into() }.to_string(),
            "This MP3 is damaged and can't be read."
        );
        assert_eq!(
            MediaError::Unsupported {
                codec: "DTS".into()
            }
            .to_string(),
            "This file's audio is DTS, which Kiln can't read yet."
        );
        assert_eq!(
            MediaError::TooSmall {
                format: "MP3".into(),
                min_bytes: 1_234_567
            }
            .to_string(),
            "MP3 can't go under 1.3 MB for this file."
        );
        assert_eq!(
            MediaError::NoAudio.to_string(),
            "This video has no sound to convert."
        );
        assert_eq!(
            MediaError::NoVideo.to_string(),
            "This file has no video to convert."
        );
        assert_eq!(
            MediaError::Damaged {
                code: "video".into()
            }
            .to_string(),
            "This video is damaged and can't be read."
        );
        assert_eq!(
            MediaError::NeedsExtension {
                codec: "HEVC",
                product: "9N4WGH0Z6VHQ"
            }
            .to_string(),
            "Needs Microsoft's HEVC Video Extensions to read HEVC video."
        );
        assert_eq!(
            MediaError::NeedsExtensionToWrite {
                codec: "HEVC",
                product: "9N4WGH0Z6VHQ"
            }
            .to_string(),
            "Needs Microsoft's HEVC Video Extensions to write HEVC video."
        );
        assert_eq!(
            MediaError::NoHardwareEncoder { codec: "H.264" }.to_string(),
            "This PC has no graphics card encoder for H.264. Choose Automatic or Processor."
        );
        assert_eq!(
            MediaError::NoHardwareEncoder { codec: "HEVC" }.to_string(),
            "This PC has no graphics card encoder for HEVC. Choose Automatic or Processor."
        );
    }
}
