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

mod calibre;
pub mod converter;
pub mod error;
pub mod facts;
mod ffmpeg;
pub mod held;
mod job;
mod libreoffice;
mod names;
pub mod output;
mod pandoc;
mod pdf;
pub mod plain;
mod process;
pub mod quality;
mod raster;
pub mod registry;
mod runner;
pub mod suggest;
mod tools;

pub use calibre::{find_installed as find_calibre, CalibreConverter};
pub use converter::{CancelToken, Context, Converter, Engines, Options, Progress, Request};
pub use error::{is_disk_full, Error, Result};
pub use facts::{inspect, Bars, Facts, Look, PdfFacts, Picture};
pub use ffmpeg::FfmpegConverter;
pub use held::{export_as, export_into, sweep_stale, Holding};
pub use job::{Availability, Kiln};
pub use libreoffice::{find_installed as find_libreoffice, LibreOfficeConverter};
pub use names::describe_format;
pub use pandoc::PandocConverter;
pub use pdf::PdfConverter;
pub use plain::{missing_engine, plain_failure, plain_failure_for, FailedJob};
pub use process::find_on_path;
pub use quality::{Knob, Limit, MediaInfo, Preset, Quality};
pub use raster::ImageConverter;
pub use registry::{Format, Kind, Registry};
pub use runner::{JobError, JobEvent, JobId, JobKind, Runner};
pub use suggest::{about, suggest, Habits, Offered, Suggested, Suggestion, Suggestions};
pub use tools::Tool;
