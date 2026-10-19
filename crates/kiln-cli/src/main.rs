//! `kiln convert <input> <format>`: the command-line front end for kiln-core.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use kiln_core::{png_to_jpg, JpegOptions};

#[derive(Parser)]
#[command(name = "kiln", version, about = "Convert files locally.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert a file to another format. The result is written next to the
    /// input and never replaces an existing file.
    Convert {
        /// The file to convert.
        input: PathBuf,
        /// The output format, such as jpg.
        format: String,
        /// Write the result here instead of next to the input.
        #[arg(short, long, value_name = "DIR")]
        out_dir: Option<PathBuf>,
        /// JPG quality, 1 to 100.
        #[arg(short, long, default_value_t = 90, value_parser = clap::value_parser!(u8).range(1..=100))]
        quality: u8,
    },
}

/// Output formats this build can produce, as shown to users.
const SUPPORTED: &[&str] = &["JPG"];

/// Normalize a user-typed format ("jpg", ".JPEG") to its display code.
fn target_code(format: &str) -> Option<&'static str> {
    match format.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("JPG"),
        _ => None,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Convert {
            input,
            format,
            out_dir,
            quality,
        } => {
            if target_code(&format).is_none() {
                eprintln!(
                    "kiln: Can't convert to {}. Supported formats: {}.",
                    format.trim_start_matches('.').to_ascii_uppercase(),
                    SUPPORTED.join(", ")
                );
                return ExitCode::from(2);
            }
            match png_to_jpg(&input, &JpegOptions { quality, out_dir }) {
                Ok(out) => {
                    println!("{}", out.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("kiln: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
