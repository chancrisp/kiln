//! Plain names for formats: what a type is for or what is special about
//! it, in a few words. The UI shows them as "WEBP: Small image for the
//! web".

/// What a format is, for people who don't know the code: the part that
/// follows "WEBP: ". The match ignores case and a leading dot, and an
/// alias ("jpeg") reads the same as its format ("jpg").
///
/// The text has no full stop, starts with a capital and no article ("Photo
/// that opens anywhere"), and is at most 40 characters.
pub fn describe_format(ext: &str) -> Option<&'static str> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    Some(match ext.as_str() {
        // Images
        "png" => "Lossless image with transparency",
        "jpg" | "jpeg" | "jfif" => "Photo that opens anywhere",
        "webp" => "Small image for the web",
        "bmp" => "Uncompressed Windows image",
        "gif" => "Simple image that can loop",
        "tiff" | "tif" => "Lossless image for print and scans",
        "ico" => "Icon image for apps and sites",
        "tga" => "Image used by games and 3D tools",
        "qoi" => "Fast, lossless image",
        "ppm" | "pnm" | "pgm" | "pbm" | "pam" => "Plain, uncompressed image",
        "svg" => "Drawing that scales to any size",
        "dng" => "Adobe's open camera raw photo",
        "cr2" => "Raw photo from a Canon camera",
        "cr3" => "Newer raw photo from a Canon camera",
        "nef" | "nrw" => "Raw photo from a Nikon camera",
        "arw" | "srf" | "sr2" => "Raw photo from a Sony camera",
        "orf" => "Raw photo from an Olympus camera",
        "rw2" => "Raw photo from a Panasonic camera",
        "raf" => "Raw photo from a Fujifilm camera",
        "pef" => "Raw photo from a Pentax camera",
        "srw" => "Raw photo from a Samsung camera",
        "avif" => "Very small, modern image",
        "jxl" => "Modern image that stays small",
        "heic" | "heif" | "hif" => "Compact photo, common on iPhones",
        "apng" => "PNG that can animate",
        // Audio
        "wav" => "Raw, uncompressed audio",
        "mp3" => "Audio that plays everywhere",
        "flac" => "Lossless audio at a smaller size",
        "ogg" | "oga" => "Open, free audio",
        "opus" => "Audio that stays clear when small",
        "m4a" => "Audio common on Apple devices",
        "aac" => "Audio used by video and streaming",
        "aiff" | "aif" => "Uncompressed audio from Apple",
        "wma" => "Microsoft Windows Media audio",
        "ac3" => "Surround sound audio for films",
        "wv" => "WavPack, lossless audio",
        // Video
        "mp4" => "Video that plays almost anywhere",
        "m4v" => "Video for Apple devices and iTunes",
        "mov" => "QuickTime video from Apple",
        "mkv" => "Flexible video with many tracks",
        "webm" => "Open video made for the web",
        "avi" => "Older Windows video format",
        "wmv" => "Microsoft Windows Media video",
        "flv" => "Video from the Flash era",
        "mpg" | "mpeg" | "vob" => "Video from the DVD era",
        "ts" | "mts" | "m2ts" => "Video from TV, camcorders and Blu-ray",
        "3gp" => "Small video from old phones",
        "ogv" => "Open, free video",
        // Subtitles
        "srt" => "Subtitles in plain text",
        "vtt" => "Subtitles for web video",
        "ass" | "ssa" => "Subtitles with styles and positions",
        // Documents
        "md" | "markdown" => "Markdown, plain text with formatting",
        "html" | "htm" => "Web page",
        "docx" => "Microsoft Word document",
        "odt" => "LibreOffice text document",
        "epub" => "E-book that reflows on any screen",
        "rst" => "ReStructuredText document",
        "tex" => "LaTeX document",
        "rtf" => "Basic formatted document",
        "txt" | "text" => "Plain text with no formatting",
        "org" => "Emacs Org-mode document",
        "adoc" | "asciidoc" => "AsciiDoc document",
        "typ" => "Typst document",
        "wiki" | "mediawiki" => "MediaWiki page",
        "textile" => "Textile document",
        "dbk" | "docbook" => "DocBook document",
        "fb2" => "FictionBook e-book",
        "ipynb" => "Jupyter notebook",
        "mobi" => "Older Kindle e-book",
        "azw3" | "azw" => "Kindle e-book",
        "doc" => "Older Microsoft Word document",
        "pdf" => "Document that looks the same anywhere",
        "pages" => "Apple Pages document",
        "wpd" => "WordPerfect document",
        "vsdx" | "vsd" => "Microsoft Visio diagram",
        "pub" => "Microsoft Publisher document",
        // Spreadsheets
        "xlsx" => "Microsoft Excel spreadsheet",
        "xls" => "Older Microsoft Excel spreadsheet",
        "ods" => "LibreOffice spreadsheet",
        "csv" => "Table as plain text",
        "numbers" => "Apple Numbers spreadsheet",
        // Presentations
        "pptx" => "Microsoft PowerPoint presentation",
        "ppt" => "Older PowerPoint presentation",
        "odp" => "LibreOffice presentation",
        "key" => "Apple Keynote presentation",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    /// Words a description must not open with: an article is dropped.
    const ARTICLES: &[&str] = &["A ", "An ", "The ", "a ", "an ", "the "];

    #[test]
    fn the_spec_examples_read_verbatim() {
        for (ext, text) in [
            ("webp", "WEBP: Small image for the web"),
            ("qoi", "QOI: Fast, lossless image"),
            ("wv", "WV: WavPack, lossless audio"),
            ("dbk", "DBK: DocBook document"),
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
                first.is_uppercase(),
                "{ext}: {d:?} should start with a capital"
            );
            assert!(
                !ARTICLES.iter().any(|a| d.starts_with(a)),
                "{ext}: {d:?} should not open with an article"
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
