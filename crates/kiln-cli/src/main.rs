//! `kiln convert <input> <format>`: the command-line front end for kiln-core.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use kiln_core::{CancelToken, Engines, Error, Kiln, Limit, Options, Preset, Quality, Tool};

#[derive(Parser)]
#[command(name = "kiln", version, about = "Convert files locally.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert a file to another format. The result is written next to the
    /// input and never replaces an existing file. Converting a paged
    /// document to an image writes one file per page; each path is printed.
    Convert {
        /// The file to convert.
        input: PathBuf,
        /// The output format, such as jpg, mp3 or docx.
        format: String,
        /// Write the result here instead of next to the input.
        #[arg(short, long, value_name = "DIR")]
        out_dir: Option<PathBuf>,
        #[command(flatten)]
        quality: QualityArgs,
        /// Path to the FFmpeg executable. Defaults to the one on PATH.
        #[arg(long, value_name = "PATH")]
        ffmpeg: Option<PathBuf>,
        /// Path to the Pandoc executable. Defaults to the one on PATH.
        #[arg(long, value_name = "PATH")]
        pandoc: Option<PathBuf>,
        /// Path to LibreOffice's soffice.exe. Defaults to the one on PATH,
        /// then its default install folder.
        #[arg(long, value_name = "PATH")]
        libreoffice: Option<PathBuf>,
        /// Path to Calibre's ebook-convert.exe. Defaults to the one on PATH,
        /// then its default install folder.
        #[arg(long, value_name = "PATH")]
        calibre: Option<PathBuf>,
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
        /// Path to LibreOffice's soffice.exe. Defaults to the one on PATH,
        /// then its default install folder.
        #[arg(long, value_name = "PATH")]
        libreoffice: Option<PathBuf>,
        /// Path to Calibre's ebook-convert.exe. Defaults to the one on PATH,
        /// then its default install folder.
        #[arg(long, value_name = "PATH")]
        calibre: Option<PathBuf>,
    },
    /// List the formats a file can be converted to.
    Formats {
        /// The file, or just a name with its extension.
        input: PathBuf,
    },
}

/// Quality settings. Each applies only where it makes sense: a bitrate
/// means nothing to a PNG, so it is ignored there.
#[derive(Args)]
struct QualityArgs {
    /// small, high or best. Sets compression for video, lossy audio and
    /// lossy images.
    #[arg(short, long, default_value = "high", value_parser = parse_preset)]
    preset: Preset,
    /// JPG quality, 1 to 100. Defaults to the preset's (75, 90 or 97).
    #[arg(short, long, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: Option<u8>,
    /// Largest size: lines for video ("720" for 720p), pixels on the long
    /// side for images and animations. "original" keeps the source's.
    #[arg(long, value_name = "N|original", value_parser = parse_limit)]
    size: Option<Limit>,
    /// Highest frame rate for video and animations, or "original".
    #[arg(long, value_name = "N|original", value_parser = parse_limit)]
    fps: Option<Limit>,
    /// Audio bitrate in kbit/s for lossy audio.
    #[arg(long, value_name = "KBPS", value_parser = clap::value_parser!(u32).range(32..=640))]
    audio_kbps: Option<u32>,
    /// Pixels per inch for pages and slides written as images.
    #[arg(long, value_name = "DPI", default_value_t = 150, value_parser = clap::value_parser!(u32).range(36..=600))]
    dpi: u32,
}

fn parse_preset(s: &str) -> Result<Preset, String> {
    Preset::parse(s).ok_or_else(|| format!("{s} isn't a preset. Use small, high or best."))
}

fn parse_limit(s: &str) -> Result<Limit, String> {
    let s = s.trim().trim_end_matches(['p', 'P']);
    if s.eq_ignore_ascii_case("original") {
        return Ok(Limit::Original);
    }
    match s.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Limit::Max(n)),
        _ => Err(format!("{s} isn't a number or \"original\".")),
    }
}

impl QualityArgs {
    fn jpeg_quality(&self) -> u8 {
        self.quality.unwrap_or(self.preset.jpeg_quality())
    }

    fn quality(&self) -> Quality {
        Quality {
            preset: self.preset,
            size: self.size.unwrap_or_default(),
            fps: self.fps.unwrap_or_default(),
            audio_kbps: self.audio_kbps,
            page_dpi: self.dpi,
        }
    }
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
            libreoffice,
            calibre,
        } => {
            let opts = Options {
                out_dir,
                jpeg_quality: quality.jpeg_quality(),
                quality: quality.quality(),
                engines: Engines {
                    ffmpeg,
                    pandoc,
                    libreoffice,
                    calibre,
                },
                ..Options::default()
            };
            let cancel = CancelToken::new();
            match kiln.convert_outputs(&input, &format, &opts, &mut |_| {}, &cancel) {
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
        Command::Tool {
            tool,
            inputs,
            out_dir,
            ffmpeg,
            pandoc,
            libreoffice,
            calibre,
        } => {
            let Some(tool) = Tool::parse(&tool) else {
                let names: Vec<&str> = Tool::ALL.iter().map(|t| t.id()).collect();
                eprintln!("kiln: {tool} isn't a tool. Tools: {}.", names.join(", "));
                return ExitCode::from(2);
            };
            let opts = Options {
                out_dir,
                engines: Engines {
                    ffmpeg,
                    pandoc,
                    libreoffice,
                    calibre,
                },
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
