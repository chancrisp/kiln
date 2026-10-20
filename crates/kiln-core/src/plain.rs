//! Plain sentences for failures people meet often, each with a next step.
//! The engine's own words stay in Show details.

/// The Settings tab that lists the helper apps, as the missing-engine
/// sentence names it. The app's tab label reads it from here too, so a
/// rename changes both.
pub const HELPER_APPS_TAB: &str = "Engines";

/// What engine output can contain, lower case, and the sentence it
/// becomes. `{EXT}` is a format code. The first match wins.
const KNOWN: [(&[&str], &str); 5] = [
    (
        &[
            "invalid data found when processing input",
            "moov atom not found",
            "could not find codec parameters",
        ],
        "This file looks damaged, or isn't really a {EXT}. Try opening it in another app.",
    ),
    (
        &["permission denied", "access denied"],
        "Kiln couldn't read or write the file. Check it isn't open in another app.",
    ),
    (
        &["no space left on device"],
        "There isn't enough free space on the drive.",
    ),
    (
        &["password", "encrypted"],
        "This file is password-protected. Remove the password, then try again.",
    ),
    (
        &["unknown encoder", "encoder not found"],
        "This FFmpeg can't write {EXT}. A full build of FFmpeg can.",
    ),
];

/// The row line for a failed engine job: a plain sentence when `raw`, all
/// of the engine's output, says something Kiln recognises, else
/// "{engine} couldn't convert the file.". `ext` is the file's extension.
pub fn plain_failure(engine: &str, ext: &str, raw: &str) -> String {
    plain_failure_for(engine, ext, ext, raw)
}

/// [`plain_failure`] for a job that reads `source` files and writes
/// `target` ones: a damaged file names its own type, a missing encoder
/// names the one it was writing.
pub fn plain_failure_for(engine: &str, source: &str, target: &str, raw: &str) -> String {
    let raw = raw.to_lowercase();
    let row = KNOWN
        .iter()
        .position(|(needles, _)| needles.iter().any(|n| raw.contains(n)));
    match row {
        // The encoder row is about what's written.
        Some(4) => KNOWN[4].1.replace("{EXT}", &target.to_ascii_uppercase()),
        Some(i) => KNOWN[i].1.replace("{EXT}", &source.to_ascii_uppercase()),
        None => format!("{engine} couldn't convert the file."),
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

    #[test]
    fn a_damaged_file_names_its_type() {
        let want = "This file looks damaged, or isn't really a MP4. Try opening it in another app.";
        for raw in [
            "[in#0 @ 0x55] Error opening input: Invalid data found when processing input",
            "[mov,mp4 @ 0x1] moov atom not found\nclip.mp4: Invalid argument",
            "Could not find codec parameters for stream 0",
        ] {
            assert_eq!(plain_failure("FFmpeg", "mp4", raw), want);
        }
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
    fn a_missing_encoder_names_what_it_was_writing() {
        assert_eq!(
            plain_failure("FFmpeg", "opus", "Unknown encoder 'libopus'"),
            "This FFmpeg can't write OPUS. A full build of FFmpeg can."
        );
        assert_eq!(
            plain_failure_for("FFmpeg", "wav", "mp3", "Encoder not found"),
            "This FFmpeg can't write MP3. A full build of FFmpeg can."
        );
        // A damaged file still names the file's own type.
        assert_eq!(
            plain_failure_for("FFmpeg", "wav", "mp3", "moov atom not found"),
            "This file looks damaged, or isn't really a WAV. Try opening it in another app."
        );
    }

    #[test]
    fn anything_else_names_the_engine_only() {
        assert_eq!(
            plain_failure("FFmpeg", "mp4", "Conversion failed!"),
            "FFmpeg couldn't convert the file."
        );
        assert_eq!(
            plain_failure("Pandoc", "md", ""),
            "Pandoc couldn't convert the file."
        );
    }

    #[test]
    fn matching_ignores_case_and_reads_every_line() {
        let raw = "ffmpeg version 7.0\nbuilt with gcc\n\nINVALID DATA FOUND WHEN PROCESSING INPUT";
        assert_eq!(
            plain_failure("FFmpeg", "Mkv", raw),
            "This file looks damaged, or isn't really a MKV. Try opening it in another app."
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
        assert!(plain_failure("FFmpeg", "mp4", raw).starts_with("This file looks damaged"));
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
