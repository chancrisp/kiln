//! Conversion logic for Kiln.
//!
//! This crate has no UI dependencies. The CLI and the Windows app both call
//! into it, so every rule about inputs, outputs and naming lives here.
//!
//! ```no_run
//! use kiln_core::{Kiln, Options};
//! let out = Kiln::new().convert("photo.png".as_ref(), "jpg", &Options::default())?;
//! # Ok::<(), kiln_core::Error>(())
//! ```

pub mod converter;
pub mod error;
mod ffmpeg;
mod job;
mod libreoffice;
pub mod output;
mod pandoc;
mod process;
mod raster;
pub mod registry;
mod runner;
mod tools;

pub use converter::{CancelToken, Context, Converter, Engines, Options, Progress, Request};
pub use error::{Error, Result};
pub use ffmpeg::FfmpegConverter;
pub use job::Kiln;
pub use libreoffice::{find_installed as find_libreoffice, LibreOfficeConverter};
pub use pandoc::PandocConverter;
pub use raster::ImageConverter;
pub use registry::{Format, Kind, Registry};
pub use runner::{JobError, JobEvent, JobId, JobKind, Runner};
pub use tools::Tool;
