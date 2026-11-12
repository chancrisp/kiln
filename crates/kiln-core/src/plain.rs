//! Plain sentences for failures people meet often, each with a next step.
//! The engine's own words stay in Show details.

use std::path::Path;

/// The Settings tab that lists the helper apps, as the missing-engine
/// sentence names it. The app's tab label reads it from here too, so a
/// rename changes both.
pub const HELPER_APPS_TAB: &str = "Helper apps";

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
            // Kiln's own PDF reader, and Windows' for drawing pages.
            "the pdf couldn't be read (",
            "windows couldn't open the pdf: unspecified error",
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
/// Kiln recognises, else that the job stopped before it finished.
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
/// the job stopped before it finished and to try again. Rows never name
/// the helper that failed: the helper's own words are in Show details.
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
        None => stopped(job.verb),
    }
}

/// Whether trying the same job again could work, judging by the row line:
/// a helper that stopped, a file another app had open, a full drive.
/// A damaged file or a missing helper will fail the same way, so a Retry
/// link would only mislead. Lives beside the sentences so a change to one
/// is a change to the other (the tests below tie them).
pub fn retry_may_help(message: &str) -> bool {
    message.ends_with("Try again.")
        || message.contains("didn't finish. Try again,")
        || message.contains("Check it isn't open in another app.")
        || message.contains("free space")
}

/// The row line when a job ends without a cause Kiln recognises, such as
/// a helper that crashed: the job, what happened, what to do.
fn stopped(verb: &str) -> String {
    let job = if verb == "convert" {
        "The conversion"
    } else {
        "The job"
    };
    format!("{job} stopped before it finished. Try again.")
}

/// The row line for a file whose content isn't the type it claims
/// (`code` is the type, such as "PNG"). Same words as a damaged file the
/// helper reports, so a person meets one sentence for one problem.
pub fn damaged_as(code: &str) -> String {
    let code = code.to_ascii_uppercase();
    format!(
        "This file looks damaged, or isn't really {} {code}. Try opening it in another app.",
        a_or_an(&code)
    )
}

/// The row line for a file that stops before its end (`code` is the type,
/// such as "JPG"): it can only be got again whole.
pub fn cut_short(code: &str) -> String {
    format!(
        "This {} looks cut short. Try copying or downloading it again.",
        code.to_ascii_uppercase()
    )
}

/// The row line for an image Kiln couldn't decode. `reason` is the
/// decoder's own words; when Kiln wrote a whole sentence for it (a camera
/// RAW file Windows can't read) that sentence is the line, else the image
/// is called damaged.
pub fn unreadable_image(reason: &str) -> String {
    let reason = reason.trim();
    let whole_sentence = reason.ends_with('.')
        && reason.chars().next().is_some_and(char::is_uppercase)
        && !reason.contains(['\\', '/', ':']);
    if whole_sentence {
        reason.to_owned()
    } else {
        "This image looks damaged, or isn't a type Kiln can read. Try opening it in another app."
            .to_owned()
    }
}

/// The row line for a file Kiln can't open or write because of the
/// operating system: not there, locked, or something else.
pub fn io_failure(kind: std::io::ErrorKind) -> String {
    if kind == std::io::ErrorKind::NotFound {
        gone()
    } else {
        "Kiln couldn't read or write the file. Check it isn't open in another app.".to_owned()
    }
}

/// The row line for a file that is no longer where it was added from.
pub fn gone() -> String {
    "The file isn't there any more. Check it wasn't moved or deleted, then add it again.".to_owned()
}

/// The row line when Kiln couldn't write a result of type `code`.
pub fn cannot_write(code: &str) -> String {
    format!(
        "Kiln couldn't write the {}. Try another format, or check the drive has room.",
        code.to_ascii_uppercase()
    )
}

/// The row line for a Kiln-written failure whose own sentence is already
/// plain (the PDF code says "the PDF has no pages."). Returns `None` when
/// the text carries a library's words, a path or a code.
pub fn own_sentence(summary: &str) -> Option<String> {
    let t = summary.trim();
    let clean = t.ends_with('.') && !t.contains(['(', ')', '{', '}', '\\', '/', ':']);
    let first = t.chars().next()?;
    clean.then(|| first.to_uppercase().chain(t.chars().skip(1)).collect())
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
///
/// `engine` is the helper's name, or "audio-mf" for Windows' Media
/// Foundation, which no helper app brings.
pub fn missing_engine(engine: &str) -> String {
    if engine == crate::audio::AUDIO_MF {
        return crate::error::media_feature_pack();
    }
    if !has_download_page(engine) {
        return format!("Needs {engine}, which isn't installed.");
    }
    format!(
        "Needs {engine}, which isn't installed. Settings > {HELPER_APPS_TAB} says where to get it."
    )
}

/// The row line when Office's `app` ("Word") meets a file with a password.
pub fn office_password(app: &str) -> String {
    format!("This file has a password. Open it in {app} and save it without one, then try again.")
}

/// The row line when Office's `app` didn't finish in time.
pub fn office_timeout(app: &str) -> String {
    format!("{app} didn't finish. Try again, or close {app} if it's showing a message.")
}

/// The row line when Office's `app` is already open and LibreOffice isn't
/// installed to do the work instead (decision 4).
pub fn office_in_use(app: &str) -> String {
    format!("Close {app} to convert this, or install LibreOffice.")
}

/// Whether Settings > Helper apps has a Get it for `helper`, by the name
/// a missing one goes by ("FFmpeg"). FFmpeg comes with Kiln, so it has
/// none, and nothing sends anyone to look for one.
pub fn has_download_page(helper: &str) -> bool {
    helper != "FFmpeg"
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
    fn office_messages_are_the_specs() {
        assert_eq!(
            office_password("Word"),
            "This file has a password. Open it in Word and save it without one, then try again."
        );
        assert_eq!(
            office_timeout("Word"),
            "Word didn't finish. Try again, or close Word if it's showing a message."
        );
        assert_eq!(
            office_in_use("PowerPoint"),
            "Close PowerPoint to convert this, or install LibreOffice."
        );
        // Trying again can help a stuck Office, not a password or an open app.
        assert!(retry_may_help(&office_timeout("Excel")));
        assert!(!retry_may_help(&office_password("Word")));
        assert!(!retry_may_help(&office_in_use("PowerPoint")));
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
                "The conversion stopped before it finished. Try again."
            );
            assert_eq!(
                plain_failure(engine, "mp4", "title: my password"),
                "The conversion stopped before it finished. Try again."
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
            "The conversion stopped before it finished. Try again."
        );
        // Not even for a document engine, in any case, or in the name of
        // the result beside it.
        let input = Path::new(r"D:\Access Denied\Encrypted.docx");
        let paths = [input, Path::new(r"D:\Access Denied\Encrypted.pdf")];
        let raw = "convert d:\\access denied\\encrypted.docx -> D:\\Access Denied\\ENCRYPTED.pdf\nError: source file could not be loaded";
        assert_eq!(
            plain_failure_for(&job("LibreOffice", "convert", &paths), raw),
            "The conversion stopped before it finished. Try again."
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
    fn a_pdf_kiln_cant_read_is_damaged_not_stopped() {
        // D33: lopdf's or Windows' "couldn't read it" is a damaged PDF,
        // which no Retry mends.
        let pdf = FailedJob {
            source: "pdf",
            target: "docx",
            ..job("Kiln", "convert", &[])
        };
        let want = "This file looks damaged, or isn't really a PDF. Try opening it in another app.";
        for raw in [
            "the PDF couldn't be read (xref table not found at offset 51204).\n",
            "Windows couldn't open the PDF: Unspecified error\n",
        ] {
            let said = plain_failure_for(&pdf, raw);
            assert_eq!(said, want, "{raw}");
            assert!(!retry_may_help(&said));
        }
        // A password is still a password, and a page that couldn't be
        // drawn is not the file's fault.
        assert_eq!(
            plain_failure_for(&pdf, "the PDF is protected with a password.\n"),
            "This file is password-protected. Remove the password, then try again."
        );
        assert_eq!(
            plain_failure_for(&pdf, "a page couldn't be drawn (bad PNG).\n"),
            "The conversion stopped before it finished. Try again."
        );
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
    fn anything_else_says_the_job_stopped_and_to_try_again() {
        assert_eq!(
            plain_failure("FFmpeg", "mp4", "Conversion failed!"),
            "The conversion stopped before it finished. Try again."
        );
        assert_eq!(
            plain_failure("Pandoc", "md", ""),
            "The conversion stopped before it finished. Try again."
        );
        for (verb, want) in [
            (
                "convert",
                "The conversion stopped before it finished. Try again.",
            ),
            ("compress", "The job stopped before it finished. Try again."),
            ("crop", "The job stopped before it finished. Try again."),
            ("trim", "The job stopped before it finished. Try again."),
            ("split", "The job stopped before it finished. Try again."),
            ("merge", "The job stopped before it finished. Try again."),
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
            // FFmpeg comes with Kiln, so Settings has nowhere to get it.
            ("FFmpeg", "Needs FFmpeg, which isn't installed."),
            (
                "Pandoc",
                "Needs Pandoc, which isn't installed. Settings > Helper apps says where to get it.",
            ),
            (
                "LibreOffice",
                "Needs LibreOffice, which isn't installed. Settings > Helper apps says where to get it.",
            ),
            (
                "Calibre",
                "Needs Calibre, which isn't installed. Settings > Helper apps says where to get it.",
            ),
        ] {
            assert_eq!(missing_engine(engine), want);
        }
    }

    #[test]
    fn retry_is_offered_only_where_trying_again_can_work() {
        for line in [
            stopped("convert"),
            stopped("crop"),
            io_failure(std::io::ErrorKind::PermissionDenied),
            plain_failure("FFmpeg", "wav", "out.wav: Permission denied"),
            plain_failure("FFmpeg", "mov", "No space left on device"),
        ] {
            assert!(retry_may_help(&line), "{line}");
        }
        for line in [
            damaged_as("png"),
            unreadable_image("unexpected end of file"),
            gone(),
            missing_engine("FFmpeg"),
            plain_failure("FFmpeg", "opus", "Unknown encoder 'libopus'"),
            plain_failure("Kiln", "pdf", "the PDF is protected with a password."),
        ] {
            assert!(!retry_may_help(&line), "{line}");
        }
    }

    #[test]
    fn a_row_line_never_names_the_helper_or_a_path() {
        let path = Path::new(r"C:\Users\me\Pictures\holiday.png");
        for line in [
            damaged_as("png"),
            unreadable_image("unexpected end of file"),
            unreadable_image(&path.to_string_lossy()),
            io_failure(std::io::ErrorKind::NotFound),
            io_failure(std::io::ErrorKind::PermissionDenied),
            cannot_write("jpg"),
            gone(),
            stopped("convert"),
        ] {
            assert!(!line.contains('\\'), "{line}");
            assert!(
                !line.contains("FFmpeg") && !line.contains("Pandoc"),
                "{line}"
            );
            assert!(line.ends_with('.'), "{line}");
        }
    }

    #[test]
    fn a_wrong_or_damaged_image_says_what_to_try() {
        assert_eq!(
            damaged_as("png"),
            "This file looks damaged, or isn't really a PNG. Try opening it in another app."
        );
        assert_eq!(
            unreadable_image("unexpected end of file"),
            "This image looks damaged, or isn't a type Kiln can read. Try opening it in another app."
        );
        // A sentence Kiln wrote itself is kept.
        let raw = "Windows can't read this camera's RAW files. Install the Raw Image Extension from the Microsoft Store.";
        assert_eq!(unreadable_image(raw), raw);
    }

    #[test]
    fn a_files_own_plain_sentence_is_kept_but_library_text_is_not() {
        assert_eq!(
            own_sentence("the PDF has no pages.").as_deref(),
            Some("The PDF has no pages.")
        );
        assert_eq!(own_sentence("the PDF couldn't be read (bad xref)."), None);
        assert_eq!(own_sentence("windows couldn't draw: 0x8898"), None);
        assert_eq!(own_sentence(r"C:\x\y.pdf isn't a PDF."), None);
    }

    #[test]
    fn media_errors_read_plainly() {
        use crate::error::Error;
        use crate::runner::JobError;
        use kiln_media::MediaError;
        let (input, out) = (Path::new(r"C:\a\song.wav"), Path::new(r"C:\held\song.mp3"));
        let from_media = |e: MediaError| crate::audio::from_media(e, input, out);
        let row = |e: MediaError| JobError::from(from_media(e)).message;
        assert_eq!(
            row(MediaError::Damaged { code: "MP3".into() }),
            "This MP3 is damaged and can't be read."
        );
        assert_eq!(
            row(MediaError::Unsupported {
                codec: "DTS".into()
            }),
            "This file's audio is DTS, which Kiln can't read yet."
        );
        assert_eq!(
            row(MediaError::NoAudio),
            "This video has no sound to convert."
        );
        assert_eq!(
            row(MediaError::NeedsMediaFoundation),
            "Needs Windows' Media Feature Pack, which isn't installed."
        );
        assert!(matches!(
            from_media(MediaError::NeedsMediaFoundation),
            Error::EngineMissing { engine: "audio-mf" }
        ));
        assert_eq!(
            missing_engine("audio-mf"),
            "Needs Windows' Media Feature Pack, which isn't installed."
        );
        assert_eq!(
            row(MediaError::LameMissing),
            "Kiln's MP3 encoder is missing. Reinstall Kiln to fix it."
        );
        assert_eq!(
            row(MediaError::TooSmall {
                format: "Opus".into(),
                min_bytes: 200_000
            }),
            "Opus can't go under 0.2 MB for this file."
        );
        assert_eq!(
            row(MediaError::TooSmall {
                format: "MP3".into(),
                min_bytes: 1_234_567
            }),
            "MP3 can't go under 1.3 MB for this file."
        );
        assert!(matches!(
            from_media(MediaError::Cancelled),
            Error::Cancelled
        ));
        // A file that couldn't be read is named; else the one being written.
        let gone = || std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(
            from_media(MediaError::Read(gone())),
            Error::Io { path, .. } if path == input
        ));
        assert!(matches!(
            from_media(MediaError::Io(gone())),
            Error::Io { path, .. } if path == out
        ));
        // kiln-media's own whole sentences pass through as they are.
        for sentence in [
            "AC3 takes up to 5.1 channels.",
            "WMA can't be written with no audio.",
        ] {
            assert_eq!(row(MediaError::Encoder(sentence.into())), sentence);
        }
        // An encoder's own words wait in Show details.
        let e = JobError::from(from_media(MediaError::Encoder(
            "LAME refused the settings".into(),
        )));
        assert_eq!(
            e.message,
            "The conversion stopped before it finished. Try again."
        );
        assert_eq!(e.details, "LAME refused the settings");
        // The CLI prints the error itself: the same sentence.
        assert_eq!(
            from_media(MediaError::NoAudio).to_string(),
            "This video has no sound to convert."
        );
        assert_eq!(
            from_media(MediaError::NeedsMediaFoundation).to_string(),
            "Needs Windows' Media Feature Pack, which isn't installed."
        );
    }
}
