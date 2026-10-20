use std::io;
use std::path::{Path, PathBuf};

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
    /// Where a write ran out of room, when that is why this failed.
    pub fn no_room(&self) -> Option<&Path> {
        match self {
            Error::Io { path, source } if is_disk_full(source) => Some(path),
            _ => None,
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

/// Whether `e` says the disk is full: on Windows ERROR_DISK_FULL (112)
/// or ERROR_HANDLE_DISK_FULL (39), elsewhere ENOSPC. Read from the OS
/// error code, which is what `StorageFull` is made from; that kind itself
/// is newer than the crate's minimum Rust.
pub fn is_disk_full(e: &io::Error) -> bool {
    const ERROR_HANDLE_DISK_FULL: i32 = 39;
    const ERROR_DISK_FULL: i32 = 112;
    const ENOSPC: i32 = 28;
    match e.raw_os_error() {
        Some(code) if cfg!(windows) => code == ERROR_DISK_FULL || code == ERROR_HANDLE_DISK_FULL,
        Some(code) => code == ENOSPC,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The OS's own "disk full" error.
    fn full() -> io::Error {
        io::Error::from_raw_os_error(if cfg!(windows) { 112 } else { 28 })
    }

    #[test]
    fn a_full_disk_is_told_apart() {
        assert!(is_disk_full(&full()));
        assert!(!is_disk_full(&io::Error::from(io::ErrorKind::NotFound)));
        // Access denied, on Windows and elsewhere.
        assert!(!is_disk_full(&io::Error::from_raw_os_error(5)));
        assert!(!is_disk_full(&io::Error::from_raw_os_error(13)));
        if cfg!(windows) {
            assert!(is_disk_full(&io::Error::from_raw_os_error(39)));
        }
    }

    #[test]
    fn no_room_names_the_folder_the_write_failed_in() {
        let path = Path::new(r"C:\held\photo.jpg");
        assert_eq!(Error::io(path, full()).no_room(), Some(path));
        let other = Error::io(path, io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(other.no_room(), None);
        assert_eq!(Error::Cancelled.no_room(), None);
    }
}
