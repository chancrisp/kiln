//! Inputs the fuzzers found a problem with, each of which used to panic, run
//! out of memory or hang a reader. They sit in tests/data/fuzz-regressions and
//! are named for what was wrong with them.
//!
//! Each case pins the exact outcome the reader gives today: `Rejected` (opening
//! gives an error), `Read` (it opens and reads to the end; the damage is in a
//! part it can decode or skip) or `Failed` (it opens, then reports an error
//! part-way). A change in any of them is a change in how a damaged file is
//! handled and should be looked at. Each runs on its own thread with a time
//! limit, so a hang fails the test instead of stalling it, and none may hold
//! more than 64 MiB.

#[path = "../fuzz/counting_alloc.rs"]
mod counting_alloc;
#[path = "../fuzz/entry.rs"]
mod entry;

use entry::{Outcome, Reader, Scratch, SYM_EXTS};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// The most memory one input may make a reader hold, in bytes.
const MEMORY_BUDGET: isize = 64 << 20;

fn sym(ext: &str) -> Reader {
    Reader::Sym(SYM_EXTS.iter().copied().find(|e| *e == ext).unwrap())
}

/// (file, reader, the outcome)
fn cases() -> Vec<(&'static str, Reader, Outcome)> {
    use Outcome::{Failed, Parsed, Read, Rejected as Rejects};
    vec![
        // WAV: a ds64 data size so big that adding it to the offset wrapped.
        ("rf64-ds64-length-overflow.wav", Reader::Wav, Rejects),
        // Symphonia reads these when Kiln's own WAV and AIFF readers refuse.
        ("wav-empty-zero-rate.wav", sym("wav"), Rejects),
        ("aiff-empty-zero-rate.aiff", sym("aiff"), Rejects),
        ("wav-zero-sample-rate.wav", sym("wav"), Rejects),
        ("aiff-zero-sample-rate.aiff", sym("aiff"), Rejects),
        // MP4 and MOV.
        ("mp4-zero-time-scale.mp4", sym("mp4"), Rejects),
        ("mp4-capacity-overflow.mp4", sym("mp4"), Rejects),
        ("mp4-esds-no-descriptor.mov", sym("mov"), Rejects),
        ("alac-frame-length.m4a", sym("m4a"), Rejects),
        ("aac-section-count.mov", sym("mov"), Read),
        ("aac-band-count.m4a", sym("m4a"), Failed),
        // An MP4 box that claims a 64-bit size of 2^64 - 1.
        ("mp4-box-size-overflow.mp4", Reader::Mp4, Parsed),
        ("mp4-box-size-overflow.mp4", sym("mp4"), Rejects),
        // Bare AAC.
        ("aac-section-count.aac", sym("aac"), Read),
        // Matroska and WebM.
        ("mkv-no-element-open.mkv", sym("mkv"), Rejects),
        ("mkv-element-position.mkv", sym("mkv"), Rejects),
        ("mkv-timestamp-scale.mkv", sym("mkv"), Rejects),
        ("mkv-zero-time-base.mkv", sym("mkv"), Rejects),
        ("mkv-block-length.mkv", sym("mkv"), Rejects),
        ("aac-section-count.mkv", sym("mkv"), Read),
        // Vorbis, in Ogg and in WebM.
        ("ogg-vorbis-setup-lookup.ogg", sym("ogg"), Rejects),
        ("ogg-vorbis-comment-length.ogg", sym("ogg"), Rejects),
        ("vorbis-codeword-length.ogg", sym("ogg"), Rejects),
        ("vorbis-codeword-tree.webm", sym("webm"), Rejects),
        ("vorbis-lookup-values.webm", sym("webm"), Rejects),
        ("vorbis-residue-classbook.webm", sym("webm"), Rejects),
        ("vorbis-zero-class-dimensions.webm", sym("webm"), Rejects),
        ("vorbis-floor-height.webm", sym("webm"), Read),
        ("vorbis-floor-slice.webm", sym("webm"), Read),
        // FLAC: a Vorbis comment that claims a 3 GB vendor string.
        ("flac-comment-length.flac", sym("flac"), Rejects),
    ]
}

#[test]
fn fuzz_regressions_stay_fixed() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("fuzz-regressions");
    let temp = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    for (name, reader, expect) in cases() {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut scratch = Scratch::new(&temp.path().join("input.bin")).unwrap();
        let file = scratch.load(&bytes).unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(move || {
                let outcome = entry::run(reader, &bytes, file);
                let _ = tx.send((outcome, counting_alloc::peak()));
            })
            .unwrap();
        match rx.recv_timeout(Duration::from_secs(20)) {
            Err(e) => failures.push(format!("{name}: no result ({e}); it panicked or hung")),
            // The fixtures are under 100 KB; a reader that holds this much has believed a length.
            Ok((_, peak)) if peak > MEMORY_BUDGET => {
                failures.push(format!("{name}: held {} MiB at once", peak >> 20))
            }
            Ok((got, _)) if got != expect => {
                failures.push(format!("{name}: expected {expect:?}, got {got:?}"))
            }
            Ok(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn an_mp4_box_of_size_u64_max_gives_no_trim() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("fuzz-regressions")
        .join("mp4-box-size-overflow.mp4");
    for rate in [44_100, 48_000] {
        let mut file = std::fs::File::open(&path).unwrap();
        assert!(kiln_media::fuzzing::aac_trim(&mut file, rate).is_none());
    }
}

// ---------------------------------------------------------------------------
// A Vorbis setup header whose codebooks claim more vector values between them
// than one header may hold. Built here, since a file for it would be megabytes.

/// Vorbis packs its bit fields least significant bit first.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u64, count: u32) {
        for i in 0..count {
            if self.used % 8 == 0 {
                self.bytes.push(0);
            }
            *self.bytes.last_mut().unwrap() |= (((value >> i) & 1) as u8) << (self.used % 8);
            self.used += 1;
        }
    }
}

/// An Ogg stream of one logical stream holding these packets, one after the
/// other, with the checksums filled in.
fn ogg_stream(packets: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut seq = 0u32;
    let mut lacing: Vec<u8> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut continued = false;
    let mut flush = |out: &mut Vec<u8>, lacing: &mut Vec<u8>, body: &mut Vec<u8>, cont: bool| {
        out.extend_from_slice(b"OggS\0");
        let mut flags = if cont { 1 } else { 0 };
        if seq == 0 {
            flags |= 2;
        }
        out.push(flags);
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(lacing.len() as u8);
        out.append(lacing);
        out.append(body);
        seq += 1;
    };
    for p in packets {
        let mut rest = &p[..];
        loop {
            let lace = rest.len().min(255);
            lacing.push(lace as u8);
            body.extend_from_slice(&rest[..lace]);
            let ends = lace < 255;
            rest = &rest[lace..];
            if lacing.len() == 255 {
                flush(&mut out, &mut lacing, &mut body, continued);
                continued = !ends;
            }
            if ends {
                break;
            }
        }
        if !lacing.is_empty() {
            flush(&mut out, &mut lacing, &mut body, continued);
        }
        continued = false;
    }
    entry::fix_ogg_crcs(&mut out);
    out
}

/// A codebook of two entries: a scalar one, or a vector one (every value listed).
#[derive(Clone, Copy)]
struct Book {
    dimensions: u16,
    vector: bool,
}

/// A one-channel Vorbis stream, with one silent audio packet, whose setup header
/// holds these codebooks. Codebook 0 is the residue's classbook.
fn vorbis_stream(books: &[Book]) -> Vec<u8> {
    vorbis_stream_with(books, 0)
}

/// [`vorbis_stream`], with codebook `classbook` as the residue's classbook.
fn vorbis_stream_with(books: &[Book], classbook: u8) -> Vec<u8> {
    let mut ident = vec![1];
    ident.extend_from_slice(b"vorbis");
    ident.extend_from_slice(&0u32.to_le_bytes());
    ident.push(1);
    ident.extend_from_slice(&44_100u32.to_le_bytes());
    ident.extend_from_slice(&[0; 12]);
    ident.push(0xB8);
    ident.push(1);

    let mut comment = vec![3];
    comment.extend_from_slice(b"vorbis");
    comment.extend_from_slice(&[0; 8]);
    comment.push(1);

    let mut b = Bits::default();
    b.put(books.len() as u64 - 1, 8);
    for book in books {
        b.put(0x564342, 24);
        b.put(u64::from(book.dimensions), 16);
        b.put(2, 24);
        b.put(0, 2); // not ordered, not sparse
        b.put(0, 5); // two codewords of length 1
        b.put(0, 5);
        if book.vector {
            b.put(2, 4); // lookup type 2: every value is listed
            b.put(0, 32); // minimum
            b.put(0, 32); // delta
            b.put(0, 4); // one bit a value
            b.put(0, 1);
            for _ in 0..2 * u32::from(book.dimensions) {
                b.put(0, 1);
            }
        } else {
            b.put(0, 4); // lookup type 0: no vectors
        }
    }
    b.put(0, 6); // time domain transforms: one
    b.put(0, 16);
    b.put(0, 6); // floors: one, of type 1, with no partitions
    b.put(1, 16);
    b.put(0, 5);
    b.put(0, 2);
    b.put(8, 4);
    b.put(0, 6); // residues: one, of type 1, empty, with codebook 0 as its classbook
    b.put(1, 16);
    b.put(0, 24);
    b.put(0, 24);
    b.put(0, 24);
    b.put(0, 6);
    b.put(u64::from(classbook), 8);
    b.put(0, 3);
    b.put(0, 1);
    b.put(0, 6); // mappings: one of type 0, with one submap
    b.put(0, 16);
    b.put(0, 1);
    b.put(0, 1);
    b.put(0, 2);
    b.put(0, 8);
    b.put(0, 8);
    b.put(0, 8);
    b.put(0, 6); // modes: one
    b.put(0, 1);
    b.put(0, 16);
    b.put(0, 16);
    b.put(0, 8);
    b.put(1, 1); // framing
    let mut setup = vec![5];
    setup.extend_from_slice(b"vorbis");
    setup.extend_from_slice(&b.bytes);
    ogg_stream(&[ident, comment, setup, vec![0; 4]])
}

/// Two entries of 65,535 dimensions: 131,070 values.
const BIG: Book = Book {
    dimensions: 65_535,
    vector: true,
};

fn open_bytes(reader: Reader, bytes: Vec<u8>) -> (Outcome, isize) {
    let temp = tempfile::tempdir().unwrap();
    let mut scratch = Scratch::new(&temp.path().join("input.bin")).unwrap();
    let file = scratch.load(&bytes).unwrap();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let outcome = entry::run(reader, &bytes, file);
            (outcome, counting_alloc::peak())
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn a_vorbis_header_may_not_claim_more_than_16_mi_vector_values() {
    // 128 books of 131,070 values are 16,776,960: under the budget, so the
    // header is read (and the file, which has no audio, opens as empty).
    let (outcome, _) = open_bytes(sym("ogg"), vorbis_stream(&vec![BIG; 128]));
    assert_eq!(outcome, Outcome::Read, "the stream is meant to be valid");
    // The 129th book goes over it.
    let (outcome, peak) = open_bytes(sym("ogg"), vorbis_stream(&vec![BIG; 129]));
    assert_eq!(outcome, Outcome::Rejected);
    assert!(peak < MEMORY_BUDGET * 2, "held {} MiB", peak >> 20);
}

#[test]
fn a_vorbis_setup_may_hold_256_codebooks() {
    // 256 is the most a setup can hold; the count was cast to u8, which
    // wrapped to 0 and refused every book number.
    let one = Book {
        dimensions: 1,
        vector: false,
    };
    let (outcome, _) = open_bytes(sym("ogg"), vorbis_stream(&[one; 256]));
    assert_eq!(outcome, Outcome::Read);
    // The last of them, book 255, is a book the residue may use.
    let (outcome, _) = open_bytes(sym("ogg"), vorbis_stream_with(&[one; 256], 255));
    assert_eq!(outcome, Outcome::Read);
    // With one book fewer, 255 is past the end.
    let (outcome, _) = open_bytes(sym("ogg"), vorbis_stream_with(&[one; 255], 255));
    assert_eq!(outcome, Outcome::Rejected);
}

#[test]
fn only_a_codebook_a_residue_or_a_vector_table_uses_needs_dimensions() {
    let scalar = |dimensions| Book {
        dimensions,
        vector: false,
    };
    let one = scalar(1);
    // A scalar codebook with no dimensions, not used by a residue, is harmless.
    let (outcome, _) = open_bytes(sym("ogg"), vorbis_stream(&[one, scalar(0)]));
    assert_eq!(outcome, Outcome::Read);
    // A vector table with no dimensions has nothing in it.
    let vector0 = Book {
        dimensions: 0,
        vector: true,
    };
    let (outcome, _) = open_bytes(sym("ogg"), vorbis_stream(&[one, vector0]));
    assert_eq!(outcome, Outcome::Rejected);
    // The residue's classbook is a step size, so it needs some.
    let (outcome, _) = open_bytes(sym("ogg"), vorbis_stream(&[scalar(0)]));
    assert_eq!(outcome, Outcome::Rejected);
}
