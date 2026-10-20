//! Plain names for formats: what a type is for or what is special about
//! it, in a few words. The UI shows them as "WEBP: a small image for the
//! web".

/// What a format is, for people who don't know the code: the part that
/// follows "WEBP: ". The match ignores case and a leading dot, and an
/// alias ("jpeg") reads the same as its format ("jpg").
///
/// The text has no full stop and starts in lower case unless it opens with
/// a proper name, and it is at most 40 characters.
pub fn describe_format(ext: &str) -> Option<&'static str> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    Some(match ext.as_str() {
        // Images
        "png" => "a lossless image with transparency",
        "jpg" | "jpeg" | "jfif" => "a photo that opens anywhere",
        "webp" => "a small image for the web",
        "bmp" => "an uncompressed Windows image",
        "gif" => "a simple image that can loop",
        "tiff" | "tif" => "a lossless image for print and scans",
        "ico" => "an icon image for apps and sites",
        "tga" => "an image used by games and 3D tools",
        "qoi" => "a fast, lossless image",
        "ppm" | "pnm" | "pgm" | "pbm" | "pam" => "a plain, uncompressed image",
        "svg" => "a drawing that scales to any size",
        "dng" => "Adobe's open camera raw photo",
        "cr2" => "a raw photo from a Canon camera",
        "cr3" => "a newer raw photo from a Canon camera",
        "nef" | "nrw" => "a raw photo from a Nikon camera",
        "arw" | "srf" | "sr2" => "a raw photo from a Sony camera",
        "orf" => "a raw photo from an Olympus camera",
        "rw2" => "a raw photo from a Panasonic camera",
        "raf" => "a raw photo from a Fujifilm camera",
        "pef" => "a raw photo from a Pentax camera",
        "srw" => "a raw photo from a Samsung camera",
        "avif" => "a very small, modern image",
        "jxl" => "a modern image that stays small",
        "heic" | "heif" | "hif" => "a compact photo, common on iPhones",
        "apng" => "a PNG that can animate",
        // Audio
        "wav" => "raw, uncompressed audio",
        "mp3" => "audio that plays everywhere",
        "flac" => "lossless audio at a smaller size",
        "ogg" | "oga" => "open, free audio",
        "opus" => "audio that stays clear when small",
        "m4a" => "audio common on Apple devices",
        "aac" => "audio used by video and streaming",
        "aiff" | "aif" => "uncompressed audio from Apple",
        "wma" => "Microsoft Windows Media audio",
        "ac3" => "surround sound audio for films",
        "wv" => "WavPack, lossless audio",
        // Video
        "mp4" => "video that plays almost anywhere",
        "m4v" => "video for Apple devices and iTunes",
        "mov" => "QuickTime video from Apple",
        "mkv" => "flexible video with many tracks",
        "webm" => "open video made for the web",
        "avi" => "an older Windows video format",
        "wmv" => "Microsoft Windows Media video",
        "flv" => "video from the Flash era",
        "mpg" | "mpeg" | "vob" => "video from the DVD era",
        "ts" | "mts" | "m2ts" => "video from TV, camcorders and Blu-ray",
        "3gp" => "small video from old phones",
        "ogv" => "open, free video",
        // Subtitles
        "srt" => "subtitles in plain text",
        "vtt" => "subtitles for web video",
        "ass" | "ssa" => "subtitles with styles and positions",
        // Documents
        "md" | "markdown" => "Markdown, plain text with formatting",
        "html" | "htm" => "a web page",
        "docx" => "a Microsoft Word document",
        "odt" => "a LibreOffice text document",
        "epub" => "an e-book that reflows on any screen",
        "rst" => "a reStructuredText document",
        "tex" => "a LaTeX document",
        "rtf" => "a basic formatted document",
        "txt" | "text" => "plain text with no formatting",
        "org" => "an Emacs Org-mode document",
        "adoc" | "asciidoc" => "an AsciiDoc document",
        "typ" => "a Typst document",
        "wiki" | "mediawiki" => "a MediaWiki page",
        "textile" => "a Textile document",
        "dbk" | "docbook" => "a DocBook document",
        "fb2" => "a FictionBook e-book",
        "ipynb" => "a Jupyter notebook",
        "mobi" => "an older Kindle e-book",
        "azw3" | "azw" => "a Kindle e-book",
        "doc" => "an older Microsoft Word document",
        "pdf" => "a document that looks the same anywhere",
        "pages" => "an Apple Pages document",
        "wpd" => "a WordPerfect document",
        "vsdx" | "vsd" => "a Microsoft Visio diagram",
        "pub" => "a Microsoft Publisher document",
        // Spreadsheets
        "xlsx" => "a Microsoft Excel spreadsheet",
        "xls" => "an older Microsoft Excel spreadsheet",
        "ods" => "a LibreOffice spreadsheet",
        "csv" => "a table as plain text",
        "numbers" => "an Apple Numbers spreadsheet",
        // Presentations
        "pptx" => "a Microsoft PowerPoint presentation",
        "ppt" => "an older PowerPoint presentation",
        "odp" => "a LibreOffice presentation",
        "key" => "an Apple Keynote presentation",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    /// Proper names a description may open with, in their own case.
    const PROPER: &[&str] = &["WavPack", "Microsoft", "QuickTime", "Markdown", "Adobe's"];

    #[test]
    fn the_spec_examples_read_verbatim() {
        for (ext, text) in [
            ("webp", "WEBP: a small image for the web"),
            ("qoi", "QOI: a fast, lossless image"),
            ("wv", "WV: WavPack, lossless audio"),
            ("dbk", "DBK: a DocBook document"),
        ] {
            let said = format!("{}: {}", ext.to_uppercase(), describe_format(ext).unwrap());
            assert_eq!(said, text);
        }
    }

    #[test]
    fn the_match_ignores_case_and_a_leading_dot() {
        assert_eq!(describe_format(".WebP"), describe_format("webp"));
        assert_eq!(describe_format("JPEG"), describe_format("jpg"));
        assert_eq!(describe_format("nope"), None);
        assert_eq!(describe_format(""), None);
    }

    #[test]
    fn every_extension_the_registry_knows_has_a_description() {
        let reg = Registry::builtin();
        let mut seen = 0;
        for ext in reg.extensions() {
            seen += 1;
            assert!(describe_format(ext).is_some(), "{ext} has no description");
        }
        assert!(seen > 90, "only {seen} extensions");
    }

    #[test]
    fn descriptions_follow_the_voice() {
        let reg = Registry::builtin();
        for ext in reg.extensions() {
            let d = describe_format(ext).unwrap();
            assert!(!d.is_empty() && d == d.trim(), "{ext}: {d:?}");
            assert!(d.chars().count() <= 40, "{ext}: {d:?} is too long");
            assert!(!d.ends_with('.'), "{ext}: {d:?} ends in a full stop");
            assert!(!d.contains('!'), "{ext}: {d:?}");
            let first = d.chars().next().unwrap();
            assert!(
                first.is_lowercase() || PROPER.iter().any(|p| d.starts_with(p)),
                "{ext}: {d:?} should start in lower case"
            );
        }
    }

    #[test]
    fn an_alias_reads_like_its_format() {
        let reg = Registry::builtin();
        for ext in reg.extensions() {
            let canonical = &reg.format(ext).unwrap().ext;
            assert_eq!(describe_format(ext), describe_format(canonical), "{ext}");
        }
    }
}
