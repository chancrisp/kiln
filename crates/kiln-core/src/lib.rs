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
pub mod output;
mod pandoc;
mod process;
mod raster;
pub mod registry;

pub use converter::{CancelToken, Context, Converter, Engines, Options, Progress, Request};
pub use error::{Error, Result};
pub use ffmpeg::FfmpegConverter;
pub use job::Kiln;
pub use pandoc::PandocConverter;
pub use raster::ImageConverter;
pub use registry::{Format, Kind, Registry};
