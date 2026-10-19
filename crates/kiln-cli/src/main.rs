//! `kiln convert <input> <format>`: the command-line front end for kiln-core.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use kiln_core::{
    inspect, suggest, CancelToken, Engines, Error, Habits, Kiln, Limit, Options, Preset, Quality,
    Suggested, Suggestion, Tool,
};

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
    /// Suggest a conversion for each file, then a tool for the whole set
    /// and, for several files, a conversion they share. Picks made in the
    /// app are ignored, so the output is the same every time.
    Suggest {
        /// The files to look at.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
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
        Command::Suggest {
            inputs,
            ffmpeg,
            pandoc,
            libreoffice,
            calibre,
        } => {
            if let Some(missing) = inputs.iter().find(|p| !p.exists()) {
                eprintln!("kiln: {} doesn't exist.", missing.display());
                return ExitCode::from(2);
            }
            let opts = Options {
                engines: Engines {
                    ffmpeg,
                    pandoc,
                    libreoffice,
                    calibre,
                },
                ..Options::default()
            };
            for (name, label, reason) in suggest_rows(&kiln, &inputs, &opts) {
                println!("{name}{label}{reason}");
            }
            ExitCode::SUCCESS
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

/// The rows `kiln suggest` prints, already padded: one per file, then
/// `(all)` with the set's tool suggestion, then, for several files,
/// `(all)` with the conversion they share. Set rows appear only when there
/// is a suggestion.
fn suggest_rows(kiln: &Kiln, inputs: &[PathBuf], opts: &Options) -> Vec<(String, String, String)> {
    let cancel = CancelToken::new();
    let habits = Habits::default();
    let label = |s: &Suggestion| match &s.action {
        Suggested::Convert(ext) => kiln
            .registry()
            .format(ext)
            .map(|f| f.code())
            .unwrap_or_else(|| ext.to_uppercase()),
        Suggested::Tool(tool) => tool.label().to_string(),
    };

    // (name, Some((label, reason))) or (name, None) for "No suggestion."
    let mut rows: Vec<(String, Option<(String, String)>)> = Vec::new();
    let mut all_facts = Vec::new();
    for path in inputs {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let facts = inspect(kiln, path, opts, &cancel);
        let row = facts.as_ref().and_then(|f| {
            let offered = kiln.offered(&[path.as_path()], opts);
            suggest(std::slice::from_ref(f), &offered, &habits).convert
        });
        rows.push((name, row.map(|s| (label(&s), s.reason))));
        all_facts.push(facts);
    }

    // The set's suggestions need facts for every file.
    let set: Option<Vec<_>> = all_facts.into_iter().collect();
    if let Some(set) = set {
        let paths: Vec<&Path> = inputs.iter().map(PathBuf::as_path).collect();
        let offered = kiln.offered(&paths, opts);
        let both = suggest(&set, &offered, &habits);
        if let Some(tool) = both.tool {
            rows.push(("(all)".into(), Some((label(&tool), tool.reason))));
        }
        if inputs.len() > 1 {
            if let Some(convert) = both.convert {
                rows.push(("(all)".into(), Some((label(&convert), convert.reason))));
            }
        }
    }

    let name_width = rows
        .iter()
        .map(|(n, _)| n.chars().count())
        .max()
        .unwrap_or(0)
        + 3;
    let label_width = rows
        .iter()
        .filter_map(|(_, r)| r.as_ref().map(|(l, _)| l.chars().count()))
        .max()
        .unwrap_or(0)
        + 2;
    rows.into_iter()
        .map(|(name, row)| {
            let name = format!("{name:<name_width$}");
            match row {
                Some((label, reason)) => (name, format!("{label:<label_width$}"), reason),
                None => (name, String::new(), "No suggestion.".to_string()),
            }
        })
        .collect()
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
