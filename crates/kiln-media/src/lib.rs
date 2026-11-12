//! Kiln's in-process audio engine.
//!
//! Media Foundation code lives in [`mf`] and is `#[cfg(windows)]` underneath;
//! on other targets the crate builds without it.

pub mod ac3enc;
pub mod error;
pub mod flac;
mod gapfill;
pub mod mf;
mod mkvread;
pub mod mp3;
pub mod mp4;
mod mp4read;
pub mod ogg;
pub mod pcm;
pub mod pipeline;
pub mod process;
pub mod sym;
pub mod table;
pub mod tags;
pub mod tune;
pub mod types;
pub mod video;
pub mod wavpack;

pub use error::{MediaError, Result};
pub use mf::{mf_codecs, mf_recheck, MfCodecs};
pub use pipeline::{run, Progressed, Sink, SinkSpec, Source};
pub use process::Process;
pub use table::{can_run, convert, needs_media_foundation, open_source, probe, written};
pub use tune::{ChannelChoice, Preset, Setting, Step, Target, Tune};
pub use types::{Block, Channels, Codec, Picture, SourceInfo, Tags};

/// What the fuzz targets and their tests reach into. Not a stable API.
#[doc(hidden)]
pub mod fuzzing {
    pub use crate::mp4read::{aac_trim, Trim};

    /// The picture's facts from an MP4, MOV or Matroska file's headers.
    pub fn video_info(file: &mut std::fs::File) -> crate::Result<crate::video::VideoInfo> {
        crate::video::info::read(file)
    }

    /// An MP4 muxer as if `bias` bytes came before the file's own, so chunk
    /// offsets past 4 GiB can be tested without writing 4 GiB.
    pub fn mp4_muxer_with_bias(
        c: crate::video::Container,
        out: std::fs::File,
        bias: u64,
    ) -> crate::Result<Box<dyn crate::video::Mux>> {
        crate::video::packet::mp4_muxer_with_bias(c, out, bias)
    }

    /// The child boxes of an MP4 box's body, as (type, body).
    pub fn mp4_children(body: &[u8]) -> Vec<([u8; 4], &[u8])> {
        crate::mp4read::children(body)
    }

    /// The child elements of a Matroska master element's body, as (ID, body).
    pub fn mkv_children(body: &[u8]) -> Vec<(u32, &[u8])> {
        crate::mkvread::elements(body).collect()
    }
}

/// The Media Foundation plumbing the video probe (examples/video_probe.rs)
/// reaches into. Not a stable API.
#[cfg(windows)]
#[doc(hidden)]
pub mod probing {
    pub use crate::mf::com::{byte_stream, com, enum_transforms, Mf};
}
