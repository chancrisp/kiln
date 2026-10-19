use std::io;
use std::path::PathBuf;

/// Errors name the problem plainly. Messages are shown to users, so they
/// stay short, calm and free of exclamation marks.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{} doesn't exist.", .0.display())]
    InputNotFound(PathBuf),

    #[error("{} isn't a file.", .0.display())]
    InputNotAFile(PathBuf),

    #[error("{} isn't a PNG file.", .0.display())]
    NotPng(PathBuf),

    #[error("The output folder {} doesn't exist.", .0.display())]
    OutputDirNotFound(PathBuf),

    #[error("Couldn't read the image in {}: {reason}", path.display())]
    Decode { path: PathBuf, reason: String },

    #[error("Couldn't write the JPG: {0}")]
    Encode(String),

    #[error("JPG quality must be between 1 and 100, got {0}.")]
    InvalidQuality(u8),

    #[error("No free file name for {} after {attempts} tries.", base.display())]
    NoFreeName { base: PathBuf, attempts: u32 },

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}
