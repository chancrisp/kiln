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

mod audio;
mod budget;
mod calibre;
mod check;
pub mod converter;
pub mod error;
pub mod facts;
pub mod ffmpeg;
mod heif;
pub mod held;
mod job;
mod libreoffice;
mod metadata;
mod names;
pub mod office;
pub mod output;
mod pandoc;
mod pdf;
mod photo;
pub mod plain;
mod process;
pub mod quality;
mod raster;
pub mod registry;
mod runner;
pub mod suggest;
mod svg_guard;
mod threads;
mod tools;
mod video;
mod webp;

pub use audio::AudioConverter;
pub use calibre::{find_installed as find_calibre, CalibreConverter};
pub use check::{check_file, content_is_wrong, signature_fits, FileCheck};
pub use converter::{CancelToken, Context, Converter, Engines, Options, Progress, Request};
pub use error::{is_disk_full, Error, Result};
pub use facts::{inspect, Bars, Facts, Look, PdfFacts, Picture};
pub use ffmpeg::FfmpegConverter;
pub use held::roots::{
    close_run, forget_held_root, held_root_for, in_holding_place, note_held_root, prepare_held_dir,
    read_held_notes, read_held_roots, recovery_roots, remove_empty_held_root, stale_runs_in,
    sweep_stale_in, DriveKind, HeldNote, HeldRuns, HoldKey, Places, Prepared, SystemVolumes,
    Volumes, HELD_DIR_NAME, ROOTS_NAME,
};
#[cfg(all(not(test), any(debug_assertions, feature = "test-hooks")))]
pub use held::slow_copies_for_tests;
pub use held::{
    deliver_into_with, deliver_left, deliver_with, export_into, export_into_with, forget_source,
    kiln_results_in, log_deliveries_to, record_source, stale_runs, sweep_stale, Delivery, Holding,
    Recovered, Report, StaleRun, DISCARD_NAME, KILN_RESULTS, SOURCES_NAME,
};
pub use job::{
    first_of_tracks, sweep_stale_jobs, Availability, Kiln, KEEPS_TEXT_ONLY, NO_SUBTITLES,
};
pub use libreoffice::{find_installed as find_libreoffice, LibreOfficeConverter};
pub use names::describe_format;
pub use pandoc::PandocConverter;
pub use pdf::PdfConverter;
pub use plain::{
    has_download_page, missing_engine, plain_failure, plain_failure_for, retry_may_help, FailedJob,
};
pub use process::{current_path, find_on_path};
pub use quality::{Knob, Limit, MediaInfo, Preset, Quality};
pub use raster::ImageConverter;
pub use registry::{Format, Kind, Registry};
pub use runner::{JobError, JobEvent, JobId, JobKind, Runner};
pub use suggest::{about, suggest, Offered, Suggested, Suggestion, Suggestions};
pub use threads::{spawn_parser, spawn_worker, PARSER_STACK};
pub use tools::{Tool, ToolOptions};
pub use video::{VideoConverter, ON_THE_PROCESSOR};
