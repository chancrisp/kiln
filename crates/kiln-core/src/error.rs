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

    #[error("{} isn't a {code} file.", path.display())]
    WrongContent { path: PathBuf, code: String },

    #[error("Kiln can't convert {} files.", if .0.is_empty() { "extensionless".to_owned() } else { .0.to_ascii_uppercase() })]
    UnsupportedInput(String),

    #[error("{} isn't a format Kiln knows.", .0.to_ascii_uppercase())]
    UnknownFormat(String),

    #[error("Can't convert {from} to {to}. Supported formats: {}.", supported.join(", "))]
    UnsupportedPair {
        from: String,
        to: String,
        supported: Vec<String>,
    },

    #[error("The output folder {} doesn't exist.", .0.display())]
    OutputDirNotFound(PathBuf),

    #[error("Couldn't read the image in {}: {reason}", path.display())]
    Decode { path: PathBuf, reason: String },

    #[error("Couldn't write the {code}: {reason}")]
    Encode { code: String, reason: String },

    #[error("JPG quality must be between 1 and 100, got {0}.")]
    InvalidQuality(u8),

    #[error("{engine} isn't installed. Get it from Settings > Engines.")]
    EngineMissing { engine: &'static str },

    /// `summary` is the first line of the engine's stderr; `details` is
    /// all of it, for a "Show details" view.
    #[error("{engine} couldn't convert the file: {summary}")]
    EngineFailed {
        engine: &'static str,
        summary: String,
        details: String,
    },

    /// A tool that doesn't apply to these files. The message says why.
    #[error("{0}")]
    ToolUnavailable(String),

    /// A tool that applies but has nothing to do, such as cropping an
    /// image with no border.
    #[error("{0}")]
    NothingToDo(String),

    #[error("Cancelled.")]
    Cancelled,

    #[error("formats.toml: {0}")]
    Registry(String),

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
