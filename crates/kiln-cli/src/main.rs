//! `kiln convert <input> <format>`: the command-line front end for kiln-core.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use kiln_core::{Engines, Error, Kiln, Options, Tool};

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
        /// The output format, such as jpg, mp3 or docx.
        format: String,
        /// Write the result here instead of next to the input.
        #[arg(short, long, value_name = "DIR")]
        out_dir: Option<PathBuf>,
        /// JPG quality, 1 to 100.
        #[arg(short, long, default_value_t = 90, value_parser = clap::value_parser!(u8).range(1..=100))]
        quality: u8,
        /// Path to the FFmpeg executable. Defaults to the one on PATH.
        #[arg(long, value_name = "PATH")]
        ffmpeg: Option<PathBuf>,
        /// Path to the Pandoc executable. Defaults to the one on PATH.
        #[arg(long, value_name = "PATH")]
        pandoc: Option<PathBuf>,
    },
    /// Run a file tool: compress, crop, trim, split or merge. Results are
    /// written next to the first input; originals are never changed.
    Tool {
        /// compress, crop, trim, split or merge.
        tool: String,
        /// The files to work on. Merge takes two or more of one type.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Write results here instead of next to the input.
        #[arg(short, long, value_name = "DIR")]
        out_dir: Option<PathBuf>,
        /// Path to the FFmpeg executable. Defaults to the one on PATH.
        #[arg(long, value_name = "PATH")]
        ffmpeg: Option<PathBuf>,
        /// Path to the Pandoc executable. Defaults to the one on PATH.
        #[arg(long, value_name = "PATH")]
        pandoc: Option<PathBuf>,
    },
    /// List the formats a file can be converted to.
    Formats {
        /// The file, or just a name with its extension.
        input: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let kiln = Kiln::new();
    match cli.command {
        Command::Convert {
            input,
            format,
            out_dir,
            quality,
            ffmpeg,
            pandoc,
        } => {
            let opts = Options {
                out_dir,
                jpeg_quality: quality,
                engines: Engines { ffmpeg, pandoc },
                ..Options::default()
            };
            match kiln.convert(&input, &format, &opts) {
                Ok(out) => {
                    println!("{}", out.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("kiln: {e}");
                    ExitCode::from(exit_code(&e))
                }
            }
        }
        Command::Tool {
            tool,
            inputs,
            out_dir,
            ffmpeg,
            pandoc,
        } => {
            let Some(tool) = Tool::parse(&tool) else {
                let names: Vec<&str> = Tool::ALL.iter().map(|t| t.id()).collect();
                eprintln!("kiln: {tool} isn't a tool. Tools: {}.", names.join(", "));
                return ExitCode::from(2);
            };
            let opts = Options {
                out_dir,
                engines: Engines { ffmpeg, pandoc },
                ..Options::default()
            };
            match kiln.run_tool(tool, &inputs, &opts) {
                Ok(outputs) => {
                    for out in outputs {
                        println!("{}", out.display());
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("kiln: {e}");
                    ExitCode::from(exit_code(&e))
                }
            }
        }
        Command::Formats { input } => {
            let targets = kiln.targets_for(&input);
            if targets.is_empty() {
                let ext = input
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default();
                eprintln!("kiln: {}", Error::UnsupportedInput(ext));
                return ExitCode::from(2);
            }
            for f in targets {
                println!("{}", f.code());
            }
            ExitCode::SUCCESS
        }
    }
}

/// 2 for requests Kiln can't take on, 1 for conversions that failed.
fn exit_code(e: &Error) -> u8 {
    match e {
        Error::UnsupportedInput(_)
        | Error::UnknownFormat(_)
        | Error::UnsupportedPair { .. }
        | Error::ToolUnavailable(_) => 2,
        _ => 1,
    }
}
