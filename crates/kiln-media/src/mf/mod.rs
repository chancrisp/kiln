//! Media Foundation: which codecs this Windows has, its reader, and the AAC,
//! WMA and AC3 encoders.

pub mod aac;
pub mod ac3;
#[cfg(windows)]
pub(crate) mod com;
pub mod d3d;
mod probe;
pub mod source;
pub mod wma;

pub use probe::{mf_codecs, mf_recheck, MfCodecs};

/// Whether this Windows has Media Foundation's DLLs (see `com::installed`).
/// Always false off Windows.
pub fn mf_installed() -> bool {
    #[cfg(windows)]
    {
        com::installed()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
