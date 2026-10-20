//! Plain sentences for failures people meet often, each with a next step.
//! The engine's own words stay in Show details.

use std::path::Path;

/// The Settings tab that lists the helper apps, as the missing-engine
/// sentence names it. The app's tab label reads it from here too, so a
/// rename changes both.
pub const HELPER_APPS_TAB: &str = "Engines";

/// A failure Kiln recognises in engine output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Known {
    /// The file is damaged or isn't what its extension says.
    Damaged,
    /// The file is locked or Kiln may not touch it.
    Locked,
    NoSpace,
    /// The document is protected. Only PDF and office files say so.
    Password,
    /// This FFmpeg build can't write the target format.
    NoEncoder,
}

/// What engine output can contain, lower case, and what it means. The
/// first match in this order wins.
const KNOWN: [(Known, &[&str]); 5] = [
    (
        Known::Damaged,
        &[
            "invalid data found when processing input",
            "moov atom not found",
            "could not find codec parameters",
        ],
    ),
    (Known::Locked, &["permission denied", "access denied"]),
    (Known::NoSpace, &["no space left on device"]),
    (Known::Password, &["password", "encrypted"]),
    (Known::NoEncoder, &["unknown encoder", "encoder not found"]),
];

/// The engines whose "password" or "encrypted" means a protected
/// document: Kiln's own PDF code and LibreOffice. Elsewhere those words
/// are more likely a file's name or tags.
const PROTECTED_DOCUMENTS: [&str; 2] = ["Kiln", "LibreOffice"];

/// A failed job, as its sentence needs it.
#[derive(Debug, Clone, Copy)]
pub struct FailedJob<'a> {
    /// The engine that failed: "FFmpeg", or "Kiln" for Kiln's own code.
    pub engine: &'a str,
    /// What the job was doing: "convert", or a tool's id such as
    /// "compress".
    pub verb: &'a str,
    /// The extension of the files it read.
    pub source: &'a str,
    /// The extension of the files it was writing.
    pub target: &'a str,
    /// The files it read and was writing. Their paths and names are taken
    /// out of the engine's output before matching, so a file called
    /// "Password reset.mp4" doesn't read as a protected one.
    pub paths: &'a [&'a Path],
}

/// The row line for a failed engine job that converted `ext` files: a
/// plain sentence when `raw`, all of the engine's output, says something
/// Kiln recognises, else "{engine} couldn't convert the file.".
pub fn plain_failure(engine: &str, ext: &str, raw: &str) -> String {
    plain_failure_for(
        &FailedJob {
            engine,
            verb: "convert",
            source: ext,
            target: ext,
            paths: &[],
        },
        raw,
    )
}

/// [`plain_failure`] for any job: a damaged file names its own type, a
/// missing encoder names the one being written, and anything else says
/// what the job couldn't do.
pub fn plain_failure_for(job: &FailedJob, raw: &str) -> String {
    let raw = without_names(&raw.to_lowercase(), job.paths);
    let known = KNOWN
        .iter()
        .filter(|(k, _)| *k != Known::Password || PROTECTED_DOCUMENTS.contains(&job.engine))
        .find(|(_, needles)| needles.iter().any(|n| raw.contains(n)))
        .map(|(k, _)| *k);
    match known {
        Some(Known::Damaged) if job.source.is_empty() => {
            "This file looks damaged. Try opening it in another app.".to_owned()
        }
        Some(Known::Damaged) => {
            let code = job.source.to_ascii_uppercase();
            format!(
                "This file looks damaged, or isn't really {} {code}. Try opening it in another app.",
                a_or_an(&code)
            )
        }
        Some(Known::Locked) => {
            "Kiln couldn't read or write the file. Check it isn't open in another app.".to_owned()
        }
        Some(Known::NoSpace) => "There isn't enough free space on the drive.".to_owned(),
        Some(Known::Password) => {
            "This file is password-protected. Remove the password, then try again.".to_owned()
        }
        Some(Known::NoEncoder) => format!(
            "This FFmpeg can't write {}. A full build of FFmpeg can.",
            job.target.to_ascii_uppercase()
        ),
        None => format!("{} couldn't {} the file.", job.engine, job.verb),
    }
}

/// `raw` (lower case) with every path in `paths`, its folder and its file
/// name taken out, longest first, so matching never sees a file's own
/// name.
fn without_names(raw: &str, paths: &[&Path]) -> String {
    let mut names: Vec<String> = vec![];
    for p in paths {
        names.push(p.to_string_lossy().into_owned());
        // Only a full folder: a bare relative one could be a word.
        if let Some(dir) = p.parent().filter(|d| d.has_root()) {
            names.push(dir.to_string_lossy().into_owned());
        }
        if let Some(name) = p.file_name() {
            names.push(name.to_string_lossy().into_owned());
        }
    }
    let mut names: Vec<String> = names
        .into_iter()
        .map(|n| n.to_lowercase())
        // A name this short could be part of any word.
        .filter(|n| n.chars().count() >= 3)
        .collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut raw = raw.to_owned();
    for n in &names {
        raw = raw.replace(n.as_str(), " ");
    }
    raw
}

/// "an" before a code whose first letter is spoken with a vowel sound
/// (MP4 is "em-pee-four", SVG "ess-vee-gee"), else "a".
fn a_or_an(code: &str) -> &'static str {
    match code.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some('A' | 'E' | 'F' | 'H' | 'I' | 'L' | 'M' | 'N' | 'O' | 'R' | 'S' | 'X') => "an",
        _ => "a",
    }
}

/// The row line when a job needs an engine that isn't installed.
pub fn missing_engine(engine: &str) -> String {
    format!(
        "Needs {engine}, which isn't installed. Settings > {HELPER_APPS_TAB} says where to get it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job<'a>(engine: &'a str, verb: &'a str, paths: &'a [&'a Path]) -> FailedJob<'a> {
        FailedJob {
            engine,
            verb,
            source: "mp4",
            target: "gif",
            paths,
        }
    }

    #[test]
    fn a_damaged_file_names_its_type() {
        let want =
            "This file looks damaged, or isn't really an MP4. Try opening it in another app.";
        for raw in [
            "[in#0 @ 0x55] Error opening input: Invalid data found when processing input",
            "[mov,mp4 @ 0x1] moov atom not found\nclip.mp4: Invalid argument",
            "Could not find codec parameters for stream 0",
        ] {
            assert_eq!(plain_failure("FFmpeg", "mp4", raw), want);
        }
    }

    #[test]
    fn the_article_follows_the_spoken_letter() {
        let damaged = |ext| plain_failure("FFmpeg", ext, "moov atom not found");
        for (ext, article) in [
            ("mp4", "an MP4"),
            ("mkv", "an MKV"),
            ("flac", "an FLAC"),
            ("svg", "an SVG"),
            ("heic", "an HEIC"),
            ("png", "a PNG"),
            ("jpg", "a JPG"),
            ("wav", "a WAV"),
        ] {
            let want = format!(
                "This file looks damaged, or isn't really {article}. Try opening it in another app."
            );
            assert_eq!(damaged(ext), want);
        }
        // With no extension there's no type to name.
        assert_eq!(
            damaged(""),
            "This file looks damaged. Try opening it in another app."
        );
    }

    #[test]
    fn a_locked_file_says_to_close_it() {
        let want = "Kiln couldn't read or write the file. Check it isn't open in another app.";
        assert_eq!(
            plain_failure("FFmpeg", "wav", "out.wav: Permission denied"),
            want
        );
        assert_eq!(
            plain_failure("LibreOffice", "docx", "Error: Access denied"),
            want
        );
    }

    #[test]
    fn a_full_drive_says_so() {
        assert_eq!(
            plain_failure(
                "FFmpeg",
                "mov",
                "av_interleaved_write_frame(): No space left on device"
            ),
            "There isn't enough free space on the drive."
        );
    }

    #[test]
    fn a_locked_document_asks_for_the_password_off() {
        let want = "This file is password-protected. Remove the password, then try again.";
        assert_eq!(
            plain_failure("Kiln", "pdf", "the PDF is protected with a password."),
            want
        );
        assert_eq!(
            plain_failure("LibreOffice", "xlsx", "Error: source file is encrypted"),
            want
        );
    }

    #[test]
    fn only_documents_are_password_protected() {
        // FFmpeg, Pandoc and Calibre saying "encrypted" or "password" is
        // not a protected document.
        for engine in ["FFmpeg", "Pandoc", "Calibre"] {
            assert_eq!(
                plain_failure(engine, "mp4", "stream is encrypted\nConversion failed!"),
                format!("{engine} couldn't convert the file.")
            );
            assert_eq!(
                plain_failure(engine, "mp4", "title: my password"),
                format!("{engine} couldn't convert the file.")
            );
        }
    }

    #[test]
    fn a_files_own_name_never_matches() {
        let input = Path::new(r"C:\x\Password.mp4");
        let raw = "Input #0, mov,mp4 from 'C:\\x\\Password.mp4':\nConversion failed!";
        let paths = [input];
        assert_eq!(
            plain_failure_for(&job("FFmpeg", "convert", &paths), raw),
            "FFmpeg couldn't convert the file."
        );
        // Not even for a document engine, in any case, or in the name of
        // the result beside it.
        let input = Path::new(r"D:\Access Denied\Encrypted.docx");
        let paths = [input, Path::new(r"D:\Access Denied\Encrypted.pdf")];
        let raw = "convert d:\\access denied\\encrypted.docx -> D:\\Access Denied\\ENCRYPTED.pdf\nError: source file could not be loaded";
        assert_eq!(
            plain_failure_for(&job("LibreOffice", "convert", &paths), raw),
            "LibreOffice couldn't convert the file."
        );
        // What the engine says around the name still counts.
        let raw = "Input #0 from 'C:\\x\\Password.mp4':\nInvalid data found when processing input";
        assert!(plain_failure_for(
            &job("FFmpeg", "convert", &[Path::new(r"C:\x\Password.mp4")]),
            raw
        )
        .starts_with("This file looks damaged"));
    }

    #[test]
    fn a_missing_encoder_names_what_it_was_writing() {
        assert_eq!(
            plain_failure("FFmpeg", "opus", "Unknown encoder 'libopus'"),
            "This FFmpeg can't write OPUS. A full build of FFmpeg can."
        );
        let wav_to_mp3 = FailedJob {
            source: "wav",
            target: "mp3",
            ..job("FFmpeg", "convert", &[])
        };
        assert_eq!(
            plain_failure_for(&wav_to_mp3, "Encoder not found"),
            "This FFmpeg can't write MP3. A full build of FFmpeg can."
        );
        // A damaged file still names the file's own type.
        assert_eq!(
            plain_failure_for(&wav_to_mp3, "moov atom not found"),
            "This file looks damaged, or isn't really a WAV. Try opening it in another app."
        );
    }

    #[test]
    fn anything_else_says_what_the_job_couldnt_do() {
        assert_eq!(
            plain_failure("FFmpeg", "mp4", "Conversion failed!"),
            "FFmpeg couldn't convert the file."
        );
        assert_eq!(
            plain_failure("Pandoc", "md", ""),
            "Pandoc couldn't convert the file."
        );
        for (verb, want) in [
            ("convert", "FFmpeg couldn't convert the file."),
            ("compress", "FFmpeg couldn't compress the file."),
            ("crop", "FFmpeg couldn't crop the file."),
            ("trim", "FFmpeg couldn't trim the file."),
            ("split", "FFmpeg couldn't split the file."),
            ("merge", "FFmpeg couldn't merge the file."),
        ] {
            assert_eq!(
                plain_failure_for(&job("FFmpeg", verb, &[]), "Conversion failed!"),
                want
            );
        }
    }

    #[test]
    fn matching_ignores_case_and_reads_every_line() {
        let raw = "ffmpeg version 7.0\nbuilt with gcc\n\nINVALID DATA FOUND WHEN PROCESSING INPUT";
        assert_eq!(
            plain_failure("FFmpeg", "Mkv", raw),
            "This file looks damaged, or isn't really an MKV. Try opening it in another app."
        );
        assert_eq!(
            plain_failure("Calibre", "epub", "PERMISSION DENIED"),
            "Kiln couldn't read or write the file. Check it isn't open in another app."
        );
    }

    #[test]
    fn the_first_match_in_table_order_wins() {
        // Damaged comes before password-protected.
        let raw = "encrypted\nInvalid data found when processing input";
        assert!(plain_failure("LibreOffice", "docx", raw).starts_with("This file looks damaged"));
        // Permission comes before the encoder.
        let raw = "Unknown encoder 'x'\nPermission denied";
        assert!(plain_failure("FFmpeg", "mp4", raw).starts_with("Kiln couldn't read or write"));
    }

    #[test]
    fn a_missing_engine_says_where_to_look() {
        for (engine, want) in [
            (
                "FFmpeg",
                "Needs FFmpeg, which isn't installed. Settings > Engines says where to get it.",
            ),
            (
                "Pandoc",
                "Needs Pandoc, which isn't installed. Settings > Engines says where to get it.",
            ),
            (
                "LibreOffice",
                "Needs LibreOffice, which isn't installed. Settings > Engines says where to get it.",
            ),
            (
                "Calibre",
                "Needs Calibre, which isn't installed. Settings > Engines says where to get it.",
            ),
        ] {
            assert_eq!(missing_engine(engine), want);
        }
    }
}
