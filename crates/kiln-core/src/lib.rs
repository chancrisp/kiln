//! Conversion logic for Kiln.
//!
//! This crate has no UI dependencies. The CLI and the Windows app both call
//! into it, so every rule about inputs, outputs and naming lives here.

pub mod error;
pub mod output;
pub mod raster;

pub use error::{Error, Result};
pub use raster::{png_to_jpg, JpegOptions};
