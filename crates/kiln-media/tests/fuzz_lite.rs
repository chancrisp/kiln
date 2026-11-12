//! A fuzzer for machines where `cargo fuzz` can't run (it needs nightly, and
//! on Windows its sanitizer needs the MSVC ASan runtime).
//!
//! It takes each fixture in tests/data, plus a few seeds built here (RF64,
//! AIFF-C, tags, an MP4 with iTunSMPB), and runs the same entry points as the
//! cargo-fuzz targets (`fuzz/entry.rs`) over deterministic mutations of it:
//! bit flips, byte overwrites, truncations, length-field bumps, deleted,
//! inserted, filled and copied ranges. Each input runs on its own thread with
//! a 2 s budget. A panic, a failed block rule or a timeout is a finding, and
//! its input is saved. A crash of the whole process (a C library's access
//! violation) leaves the input in `scratch-N.bin` next to the findings.
//!
//! ```text
//! cargo test -p kiln-media --release --test fuzz_lite -- --ignored --nocapture
//! ```
//!
//! Environment: `KILN_FUZZ_MUTATIONS` (default 200000 per fixture),
//! `KILN_FUZZ_ONLY` (only seeds whose name contains this), `KILN_FUZZ_THREADS`,
//! `KILN_FUZZ_START` (first mutation number, to resume).

#[path = "../fuzz/counting_alloc.rs"]
mod counting_alloc;
#[path = "../fuzz/entry.rs"]
mod entry;

use counting_alloc::MEMORY_LIMIT;
use entry::{Outcome, Reader, Scratch, SYM_EXTS};
use kiln_media::tags::{ape, id3};
use kiln_media::{Picture, Tags};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one input may take.
const BUDGET: Duration = Duration::from_secs(2);
/// Mutations per unit of work handed to a thread.
const UNIT: u64 = 2_000;

fn data(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name);
    fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Random numbers

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let mut r = Rng(seed);
        r.next();
        r
    }

    /// SplitMix64.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0..n (n > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

// ---------------------------------------------------------------------------
// Mutations

/// A place to hit: mostly the headers, where the length fields are.
fn pick_offset(len: usize, rng: &mut Rng) -> usize {
    match rng.below(10) {
        0..=4 => rng.below(len.min(512)),
        5 => len - 1 - rng.below(len.min(256)),
        _ => rng.below(len),
    }
}

fn read_int(v: &[u8], at: usize, width: usize, big: bool) -> u64 {
    let mut n = 0u64;
    for i in 0..width {
        let b = v[at + if big { i } else { width - 1 - i }] as u64;
        n = (n << 8) | b;
    }
    n
}

fn write_int(v: &mut [u8], at: usize, width: usize, big: bool, value: u64) {
    for i in 0..width {
        let shift = 8 * (width - 1 - i);
        v[at + if big { i } else { width - 1 - i }] = (value >> shift) as u8;
    }
}

fn mutate_once(v: &mut Vec<u8>, rng: &mut Rng) {
    if v.is_empty() {
        return;
    }
    let len = v.len();
    match rng.below(100) {
        // Bit flips.
        0..=24 => {
            for _ in 0..1 + rng.below(3) {
                let at = pick_offset(v.len(), rng);
                v[at] ^= 1 << rng.below(8);
            }
        }
        // A byte set to something awkward.
        25..=36 => {
            const BYTES: [u8; 8] = [0x00, 0x01, 0x7F, 0x80, 0xFF, 0xFE, 0x40, 0x20];
            let at = pick_offset(len, rng);
            v[at] = BYTES[rng.below(BYTES.len())];
        }
        // A cut.
        37..=49 => {
            let to = match rng.below(4) {
                0 => rng.below(64.min(len + 1)),
                1 => len.saturating_sub(1 + rng.below(16)),
                _ => rng.below(len),
            };
            v.truncate(to);
        }
        // A length field made too big, too small or odd.
        50..=79 => {
            let width = [1usize, 2, 2, 4, 4, 4, 8][rng.below(7)];
            if len < width {
                return;
            }
            let mut at = pick_offset(len, rng).min(len - width);
            if rng.chance(70) {
                at -= at % width.min(4);
            }
            let big = rng.chance(50);
            let old = read_int(v, at, width, big);
            let max = if width == 8 {
                u64::MAX
            } else {
                (1u64 << (8 * width)) - 1
            };
            let new = match rng.below(16) {
                0 => 0,
                1 => 1,
                2 => max,
                3 => max - 1,
                4 => max / 2,
                5 => max / 2 + 1,
                6 => old.wrapping_add(1),
                7 => old.wrapping_sub(1),
                8 => old.wrapping_mul(2),
                9 => old / 2,
                10 => old.wrapping_add(rng.below(256) as u64),
                11 => len as u64,
                12 => (len as u64).wrapping_sub(at as u64),
                13 => (len as u64).wrapping_add(1),
                14 => 0x1000_0000 << rng.below(4),
                _ => rng.next(),
            };
            write_int(v, at, width, big, new & max);
        }
        // A range deleted.
        80..=86 => {
            let at = pick_offset(len, rng);
            let cap = if rng.chance(70) { 16 } else { 4096 };
            let n = 1 + rng.below(cap);
            let end = (at + n).min(len);
            v.drain(at..end);
        }
        // Bytes inserted.
        87..=91 => {
            let at = pick_offset(len, rng);
            let n = 1 + rng.below(32);
            let fill: Vec<u8> = if rng.chance(50) {
                (0..n).map(|_| rng.next() as u8).collect()
            } else {
                vec![[0u8, 0xFF][rng.below(2)]; n]
            };
            v.splice(at..at, fill);
        }
        // A range zeroed or filled with 0xFF.
        92..=95 => {
            let at = pick_offset(len, rng);
            let cap = if rng.chance(70) { 32 } else { 1024 };
            let n = 1 + rng.below(cap);
            let end = (at + n).min(len);
            let b = [0u8, 0xFF][rng.below(2)];
            v[at..end].fill(b);
        }
        // A range copied over another place.
        _ => {
            let from = rng.below(len);
            let n = (1 + rng.below(256)).min(len - from);
            let chunk = v[from..from + n].to_vec();
            let at = pick_offset(len, rng);
            let end = (at + n).min(len);
            v[at..end].copy_from_slice(&chunk[..end - at]);
        }
    }
}

/// The `index`th mutation of a seed. Number 0 is the seed itself.
fn mutation(seed: &[u8], seed_name: &str, index: u64) -> Vec<u8> {
    let mut v = seed.to_vec();
    if index == 0 {
        return v;
    }
    let mut rng = Rng::new(fnv(seed_name) ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let ops = match rng.below(8) {
        0 => 3,
        1 | 2 => 2,
        _ => 1,
    };
    for _ in 0..ops {
        mutate_once(&mut v, &mut rng);
    }
    if seed.starts_with(b"OggS") && rng.chance(75) {
        entry::fix_ogg_crcs(&mut v);
    }
    v
}

// ---------------------------------------------------------------------------
// Seeds

struct Seed {
    name: String,
    bytes: Vec<u8>,
    readers: Vec<Reader>,
}

fn ext_hint(e: &str) -> &'static str {
    SYM_EXTS
        .iter()
        .copied()
        .find(|x| *x == e)
        .unwrap_or_else(|| panic!("{e} isn't in SYM_EXTS"))
}

fn sym(e: &str) -> Reader {
    Reader::Sym(ext_hint(e))
}

fn full_tags() -> Tags {
    Tags {
        title: Some("F\u{fc}\u{df}e \u{2013} \u{6771}\u{4eac}".into()),
        artist: Some("Artist".into()),
        album: Some("Album".into()),
        album_artist: Some("Album Artist".into()),
        genre: Some("Genre".into()),
        comment: Some("A comment".into()),
        track: Some((3, Some(12))),
        disc: Some((1, Some(2))),
        year: Some("2001".into()),
        picture: Some(Picture {
            mime: "image/png".into(),
            data: data("cover.png"),
        }),
    }
}

fn le32(n: u32) -> [u8; 4] {
    n.to_le_bytes()
}

fn be32(n: u32) -> [u8; 4] {
    n.to_be_bytes()
}

/// The chunks of a RIFF (little-endian sizes) or FORM (big-endian) file.
fn chunks(b: &[u8], big: bool) -> Vec<([u8; 4], Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 12;
    while at + 8 <= b.len() {
        let id = [b[at], b[at + 1], b[at + 2], b[at + 3]];
        let s = [b[at + 4], b[at + 5], b[at + 6], b[at + 7]];
        let size = if big {
            u32::from_be_bytes(s)
        } else {
            u32::from_le_bytes(s)
        } as usize;
        let end = (at + 8 + size).min(b.len());
        out.push((id, b[at + 8..end].to_vec()));
        at += 8 + size + (size & 1);
    }
    out
}

fn chunk_of(b: &[u8], big: bool, id: &[u8; 4]) -> Vec<u8> {
    chunks(b, big)
        .into_iter()
        .find(|(k, _)| k == id)
        .unwrap_or_else(|| panic!("no {id:?} chunk"))
        .1
}

fn put_chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8], big: bool) {
    out.extend_from_slice(id);
    out.extend_from_slice(&if big {
        be32(body.len() as u32)
    } else {
        le32(body.len() as u32)
    });
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

/// RF64: the sizes in the RIFF and data headers are 0xFFFFFFFF, and `ds64`
/// carries the real ones.
fn rf64_from(wav: &[u8]) -> Vec<u8> {
    let fmt = chunk_of(wav, false, b"fmt ");
    let pcm = chunk_of(wav, false, b"data");
    let mut ds64 = Vec::new();
    ds64.extend_from_slice(&((pcm.len() + 100) as u64).to_le_bytes());
    ds64.extend_from_slice(&(pcm.len() as u64).to_le_bytes());
    ds64.extend_from_slice(&((pcm.len() / 4) as u64).to_le_bytes());
    ds64.extend_from_slice(&le32(0));
    let mut out = b"RF64".to_vec();
    out.extend_from_slice(&le32(0xFFFF_FFFF));
    out.extend_from_slice(b"WAVE");
    put_chunk(&mut out, b"ds64", &ds64, false);
    put_chunk(&mut out, b"fmt ", &fmt, false);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&le32(0xFFFF_FFFF));
    out.extend_from_slice(&pcm);
    out
}

/// A WAV with an `id3 ` chunk after the audio.
fn wav_with_tag(wav: &[u8]) -> Vec<u8> {
    let mut out = wav.to_vec();
    put_chunk(&mut out, b"id3 ", &id3::write_v24(&full_tags()), false);
    let riff = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&le32(riff));
    out
}

/// AIFF-C with 32-bit float samples and an `ID3 ` chunk, from a 16-bit AIFF.
fn aifc_from(aiff: &[u8]) -> Vec<u8> {
    let comm = chunk_of(aiff, true, b"COMM");
    let ssnd = chunk_of(aiff, true, b"SSND");
    let mut new_comm = comm[..6].to_vec();
    new_comm.extend_from_slice(&32u16.to_be_bytes());
    new_comm.extend_from_slice(&comm[8..18]);
    new_comm.extend_from_slice(b"fl32");
    new_comm.extend_from_slice(&[0, 0]); // an empty compression name
    let samples = &ssnd[8..];
    let mut floats = Vec::with_capacity(samples.len() * 2);
    for s in samples.chunks_exact(2) {
        let v = i16::from_be_bytes([s[0], s[1]]) as f32 / 32768.0;
        floats.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    let mut new_ssnd = vec![0u8; 8];
    new_ssnd.extend_from_slice(&floats);
    let mut body = b"AIFC".to_vec();
    put_chunk(&mut body, b"FVER", &be32(0xA280_5140), true);
    put_chunk(&mut body, b"COMM", &new_comm, true);
    put_chunk(&mut body, b"SSND", &new_ssnd, true);
    put_chunk(&mut body, b"ID3 ", &id3::write_v24(&full_tags()), true);
    let mut out = b"FORM".to_vec();
    out.extend_from_slice(&be32(body.len() as u32));
    out.extend_from_slice(&body);
    out
}

fn syncsafe(n: usize) -> [u8; 4] {
    [
        (n >> 21) as u8 & 0x7F,
        (n >> 14) as u8 & 0x7F,
        (n >> 7) as u8 & 0x7F,
        n as u8 & 0x7F,
    ]
}

/// An ID3v2.3 tag with an extended header, the whole tag unsynchronised.
fn id3v23() -> Vec<u8> {
    let frame = |id: &[u8; 4], body: &[u8]| {
        let mut f = id.to_vec();
        f.extend_from_slice(&be32(body.len() as u32));
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(body);
        f
    };
    let mut title = vec![1u8, 0xFF, 0xFE];
    for u in "Title \u{6771}".encode_utf16() {
        title.extend_from_slice(&u.to_le_bytes());
    }
    let mut comm = vec![0u8];
    comm.extend_from_slice(b"eng\0A comment");
    let mut apic = vec![0u8];
    apic.extend_from_slice(b"image/png\0");
    apic.push(3);
    apic.push(0);
    apic.extend_from_slice(&data("cover.png"));
    let mut body = Vec::new();
    // Extended header: size 6, flags, padding size.
    body.extend_from_slice(&be32(6));
    body.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    body.extend(frame(b"TIT2", &title));
    body.extend(frame(b"TRCK", b"\x003/12"));
    body.extend(frame(b"TYER", b"\x002001"));
    body.extend(frame(b"COMM", &comm));
    body.extend(frame(b"APIC", &apic));
    // Unsynchronise the whole tag: 0xFF followed by 0 or 0xE0.. gets a 0 between.
    let mut unsynced = Vec::new();
    for (i, &b) in body.iter().enumerate() {
        unsynced.push(b);
        if b == 0xFF && body.get(i + 1).is_none_or(|n| *n == 0 || n & 0xE0 == 0xE0) {
            unsynced.push(0);
        }
    }
    let mut tag = b"ID3\x03\x00".to_vec();
    tag.push(0xC0); // unsynchronised, extended header
    tag.extend_from_slice(&syncsafe(unsynced.len()));
    tag.extend_from_slice(&unsynced);
    tag
}

/// The ID3v2 tag at the front of a file.
fn id3_prefix(file: &[u8]) -> Vec<u8> {
    let size = ((file[6] as usize) << 21)
        | ((file[7] as usize) << 14)
        | ((file[8] as usize) << 7)
        | file[9] as usize;
    file[..10 + size].to_vec()
}

fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// An MP4 with no media in it, but a `moov` that says what `aac_trim` reads:
/// a movie header, a sound track with an edit list, and an iTunSMPB tag.
fn mp4_with_trim_tags() -> Vec<u8> {
    let header = |scale: u32| {
        let mut h = vec![0u8; 12];
        h.extend_from_slice(&scale.to_be_bytes());
        h.extend_from_slice(&[0u8; 8]);
        h
    };
    let mut hdlr = vec![0u8; 8];
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 12]);
    let mdia = [mp4_box(b"mdhd", &header(48_000)), mp4_box(b"hdlr", &hdlr)].concat();
    let mut elst = vec![0u8; 4];
    elst.extend_from_slice(&be32(2));
    for (d, t) in [(500u32, -1i32), (2000, 1024)] {
        elst.extend_from_slice(&d.to_be_bytes());
        elst.extend_from_slice(&t.to_be_bytes());
        elst.extend_from_slice(&be32(0x10000));
    }
    let trak = [
        mp4_box(b"edts", &mp4_box(b"elst", &elst)),
        mp4_box(b"mdia", &mdia),
    ]
    .concat();
    let mut name = vec![0u8; 4];
    name.extend_from_slice(b"iTunSMPB");
    let mut value = vec![0u8; 8];
    value.extend_from_slice(b" 00000000 00000840 000001C0 0000000000046E00 00000000");
    let item = [mp4_box(b"name", &name), mp4_box(b"data", &value)].concat();
    let ilst = mp4_box(b"ilst", &mp4_box(b"----", &item));
    let mut meta = vec![0u8; 4];
    meta.extend(ilst);
    let moov = [
        mp4_box(b"mvhd", &header(1000)),
        mp4_box(b"trak", &trak),
        mp4_box(b"udta", &mp4_box(b"meta", &meta)),
    ]
    .concat();
    let mut out = mp4_box(b"ftyp", b"M4A \0\0\0\0M4A isom");
    out.extend(mp4_box(b"moov", &moov));
    out
}

fn seeds() -> Vec<Seed> {
    let mut v: Vec<Seed> = Vec::new();
    let mut add = |name: &str, bytes: Vec<u8>, readers: Vec<Reader>| {
        v.push(Seed {
            name: name.to_string(),
            bytes,
            readers,
        })
    };

    let wav = data("tone.wav");
    add("tone.wav", wav.clone(), vec![Reader::Wav, sym("wav")]);
    add("tone24.wav", data("tone24.wav"), vec![Reader::Wav]);
    add("tone.rf64.wav", rf64_from(&wav), vec![Reader::Wav]);
    add("tone.tagged.wav", wav_with_tag(&wav), vec![Reader::Wav]);

    let aiff = data("tone.aiff");
    add("tone.aiff", aiff.clone(), vec![Reader::Aiff, sym("aiff")]);
    add("tone.float.aifc", aifc_from(&aiff), vec![Reader::Aiff]);

    let wv = data("tone.wv");
    let mut wv_tagged = wv.clone();
    wv_tagged.extend_from_slice(&ape::write(&full_tags()));
    add("tone.wv", wv, vec![Reader::WavPack, Reader::Ape]);
    add(
        "tone.tagged.wv",
        wv_tagged,
        vec![Reader::WavPack, Reader::Ape],
    );
    add("tags.ape", ape::write(&full_tags()), vec![Reader::Ape]);

    let tagged = data("tagged.mp3");
    add(
        "tags.v24.id3",
        id3::write_v24(&full_tags()),
        vec![Reader::Id3],
    );
    add("tags.v23.id3", id3v23(), vec![Reader::Id3]);
    add("tagged.id3", id3_prefix(&tagged), vec![Reader::Id3]);
    add("tagged.mp3", tagged, vec![sym("mp3"), Reader::Id3]);
    add("tone.mp3", data("tone.mp3"), vec![sym("mp3")]);
    add("vbr_noxing.mp3", data("vbr_noxing.mp3"), vec![sym("mp3")]);
    add("cbr_noxing.mp3", data("cbr_noxing.mp3"), vec![sym("mp3")]);

    for f in ["tone.flac", "tone16.flac", "tone24.flac"] {
        add(f, data(f), vec![sym("flac")]);
    }
    add("tone.ogg", data("tone.ogg"), vec![sym("ogg")]);
    add(
        "tone_opus.ogg",
        data("tone_opus.ogg"),
        vec![Reader::Opus, sym("ogg")],
    );
    add("tone.aac", data("tone.aac"), vec![sym("aac")]);

    for f in [
        "tone_aac.m4a",
        "tone_alac.m4a",
        "tone16_alac.m4a",
        "silent.mp4",
        "av.mp4",
        "ac3.mp4",
    ] {
        let ext = f.rsplit('.').next().unwrap();
        add(f, data(f), vec![sym(ext), Reader::Mp4]);
    }
    add("av.mov", data("av.mov"), vec![sym("mov"), Reader::Mp4]);
    add(
        "trim-tags.m4a",
        mp4_with_trim_tags(),
        vec![Reader::Mp4, sym("m4a")],
    );

    // Video facts: the headers of MP4 and Matroska files (h264.mp4 has the avcC
    // and a sound track; hevc10_hdr.mp4's colour description is in its SPS).
    for f in ["h264.mp4", "hevc10_hdr.mp4", "h264.mkv", "av1.mkv"] {
        add(
            &format!("video/{f}"),
            data(&format!("video/{f}")),
            vec![Reader::VideoInfo],
        );
    }

    for f in [
        "av.mkv",
        "ac3.mkv",
        "dts.mkv",
        "tone51_opus.mkv",
        "av.webm",
        "tone_opus.webm",
    ] {
        let ext = f.rsplit('.').next().unwrap();
        add(f, data(f), vec![sym(ext)]);
    }
    v
}

// ---------------------------------------------------------------------------
// Running one input

thread_local! {
    static PANIC: RefCell<String> = const { RefCell::new(String::new()) };
}

enum Fault {
    Panic(String),
    Timeout,
    /// The most memory the reader held at once, in bytes.
    Memory(usize),
}

/// One input through one reader, on its own thread, within [`BUDGET`].
fn guarded(reader: Reader, bytes: &Arc<Vec<u8>>, file: std::fs::File) -> Result<Outcome, Fault> {
    let (tx, rx) = mpsc::channel();
    let bytes = bytes.clone();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let r = catch_unwind(AssertUnwindSafe(|| entry::run(reader, &bytes, file)));
            let peak = counting_alloc::peak();
            if std::env::var_os("KILN_FUZZ_ALLOCTRACE").is_some() {
                eprintln!("peak {peak} live {}", counting_alloc::live());
            }
            let _ = tx.send(match r {
                Err(_) => Err(Fault::Panic(
                    PANIC.with(|p| std::mem::take(&mut *p.borrow_mut())),
                )),
                Ok(_) if peak > MEMORY_LIMIT => Err(Fault::Memory(peak as usize)),
                Ok(o) => Ok(o),
            });
        })
        .expect("start a thread");
    match rx.recv_timeout(BUDGET) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(Fault::Timeout),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(Fault::Panic("the worker thread died".into()))
        }
    }
}

struct Finding {
    count: u64,
    seed: String,
    index: u64,
    reader: String,
    message: String,
    saved: PathBuf,
}

#[derive(Default)]
struct Tally {
    mutations: AtomicU64,
    rejected: AtomicU64,
    read: AtomicU64,
    failed: AtomicU64,
    parsed: AtomicU64,
    units_left: AtomicUsize,
}

struct Shared {
    dir: PathBuf,
    findings: Mutex<BTreeMap<String, Finding>>,
    /// Overflows in other people's code, in a run that has overflow checks on.
    ignored: Mutex<BTreeMap<String, u64>>,
    hangs: AtomicU64,
}

impl Shared {
    fn record(&self, seed: &Seed, index: u64, reader: Reader, bytes: &[u8], fault: Fault) {
        let (signature, message) = match fault {
            Fault::Timeout => (
                format!("timeout in {}", reader.name()),
                format!("no answer in {} s", BUDGET.as_secs()),
            ),
            Fault::Memory(n) => (
                format!("memory in {}", reader.name()),
                format!("held {} MiB at once", n >> 20),
            ),
            Fault::Panic(m) => {
                // "message at path:line:col": the place is the signature.
                let first = m.lines().next().unwrap_or(&m);
                let place = first.rsplit(" at ").next().unwrap_or(first).to_string();
                // A run with overflow checks on (`KILN_FUZZ_KILN_OVERFLOW_ONLY`) is for Kiln's own
                // code: an overflow elsewhere wraps quietly in a release build, as shipped, and is
                // listed, not counted.
                if std::env::var_os("KILN_FUZZ_KILN_OVERFLOW_ONLY").is_some()
                    && first.contains("with overflow")
                    && !place.replace('\\', "/").contains("kiln-media/src/")
                {
                    *self
                        .ignored
                        .lock()
                        .unwrap()
                        .entry(format!("{} {place}", reader.name()))
                        .or_insert(0) += 1;
                    return;
                }
                (format!("{} {place}", reader.name()), m)
            }
        };
        let mut all = self.findings.lock().unwrap();
        let found = all.entry(signature.clone()).or_insert_with(|| {
            let dir = self.dir.join("findings");
            fs::create_dir_all(&dir).unwrap();
            let saved = dir.join(format!(
                "{}-{index}-{}.bin",
                seed.name.replace(['/', '\\'], "_"),
                reader.name().replace(':', "-")
            ));
            fs::write(&saved, bytes).unwrap();
            Finding {
                count: 0,
                seed: seed.name.clone(),
                index,
                reader: reader.name(),
                message,
                saved,
            }
        });
        found.count += 1;
    }
}

fn run_unit(
    shared: &Shared,
    seed: &Seed,
    tally: &Tally,
    scratch: &mut Scratch,
    scratch_name: &Path,
    from: u64,
    to: u64,
) {
    for index in from..to {
        let bytes = Arc::new(mutation(&seed.bytes, &seed.name, index));
        tally.mutations.fetch_add(1, Ordering::Relaxed);
        for &reader in &seed.readers {
            let file = scratch.load(&bytes).expect("write the scratch file");
            match guarded(reader, &bytes, file) {
                Ok(Outcome::Rejected) => tally.rejected.fetch_add(1, Ordering::Relaxed),
                Ok(Outcome::Read) => tally.read.fetch_add(1, Ordering::Relaxed),
                Ok(Outcome::Failed) => tally.failed.fetch_add(1, Ordering::Relaxed),
                Ok(Outcome::Parsed) => tally.parsed.fetch_add(1, Ordering::Relaxed),
                Err(fault) => {
                    let hung = matches!(fault, Fault::Timeout);
                    shared.record(seed, index, reader, &bytes, fault);
                    if hung {
                        // The stuck thread still has the old file: use a new one.
                        shared.hangs.fetch_add(1, Ordering::Relaxed);
                        let n = shared.hangs.load(Ordering::Relaxed);
                        *scratch = Scratch::new(&scratch_name.with_extension(format!("h{n}.bin")))
                            .expect("make a scratch file");
                        // Not run for this seed's other readers: the input is saved.
                        break;
                    }
                    0
                }
            };
        }
        if shared.hangs.load(Ordering::Relaxed) > 24 {
            return;
        }
    }
}

#[test]
#[ignore = "runs for a long time; see the module comment"]
fn fuzz_lite() {
    let per_seed = env_u64("KILN_FUZZ_MUTATIONS", 200_000);
    let start = env_u64("KILN_FUZZ_START", 0);
    let threads = env_u64(
        "KILN_FUZZ_THREADS",
        std::thread::available_parallelism()
            .map_or(4, |n| n.get() as u64)
            .min(14),
    ) as usize;
    let only = std::env::var("KILN_FUZZ_ONLY").unwrap_or_default();

    // A panic is a result here, not noise: note where, and say nothing.
    std::panic::set_hook(Box::new(|info| {
        let msg = match info.payload().downcast_ref::<&str>() {
            Some(s) => s.to_string(),
            None => info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "a panic".into()),
        };
        let place = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let mut text = format!("{msg} at {place}");
        if std::env::var_os("KILN_FUZZ_BACKTRACE").is_some() {
            text += &format!("\n{}", std::backtrace::Backtrace::force_capture());
        }
        PANIC.with(|p| *p.borrow_mut() = text);
    }));

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fuzz-lite");
    fs::create_dir_all(&dir).unwrap();
    let seeds: Vec<Seed> = seeds()
        .into_iter()
        .filter(|s| s.name.contains(&only))
        .collect();
    assert!(!seeds.is_empty(), "no seed matches {only:?}");

    // The unmodified seeds first, so a broken seed says so instead of
    // looking like a reader that rejects everything.
    for s in &seeds {
        for &r in &s.readers {
            let mut scratch = Scratch::new(&dir.join("seed-check.bin")).unwrap();
            let file = scratch.load(&s.bytes).unwrap();
            let out = entry::run(r, &s.bytes, file);
            eprintln!("seed {:<18} {:<10} {out:?}", s.name, r.name());
        }
    }

    let tallies: Vec<Tally> = seeds.iter().map(|_| Tally::default()).collect();
    let mut units = Vec::new();
    for (i, _) in seeds.iter().enumerate() {
        let mut from = start;
        let end = start + per_seed;
        while from < end {
            let to = (from + UNIT).min(end);
            units.push((i, from, to));
            from = to;
        }
        tallies[i]
            .units_left
            .store(units.iter().filter(|u| u.0 == i).count(), Ordering::Relaxed);
    }
    let shared = Shared {
        dir: dir.clone(),
        findings: Mutex::new(BTreeMap::new()),
        ignored: Mutex::new(BTreeMap::new()),
        hangs: AtomicU64::new(0),
    };
    let next = AtomicUsize::new(0);
    let began = Instant::now();

    std::thread::scope(|scope| {
        for t in 0..threads {
            let (seeds, tallies, units, shared, next, dir) =
                (&seeds, &tallies, &units, &shared, &next, &dir);
            scope.spawn(move || {
                let name = dir.join(format!("scratch-{t}.bin"));
                let mut scratch = Scratch::new(&name).expect("make a scratch file");
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(i, from, to)) = units.get(n) else {
                        break;
                    };
                    let (seed, tally) = (&seeds[i], &tallies[i]);
                    run_unit(shared, seed, tally, &mut scratch, &name, from, to);
                    if tally.units_left.fetch_sub(1, Ordering::Relaxed) == 1 {
                        eprintln!(
                            "done {:<18} {:>8} mutations  rejected {:>8}  read {:>8}  failed {:>8}  parsed {:>8}  [{:?}]",
                            seed.name,
                            tally.mutations.load(Ordering::Relaxed),
                            tally.rejected.load(Ordering::Relaxed),
                            tally.read.load(Ordering::Relaxed),
                            tally.failed.load(Ordering::Relaxed),
                            tally.parsed.load(Ordering::Relaxed),
                            began.elapsed(),
                        );
                    }
                }
            });
        }
    });

    let mut summary = String::new();
    for (s, t) in seeds.iter().zip(&tallies) {
        summary += &format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            s.name,
            t.mutations.load(Ordering::Relaxed),
            t.rejected.load(Ordering::Relaxed),
            t.read.load(Ordering::Relaxed),
            t.failed.load(Ordering::Relaxed),
            t.parsed.load(Ordering::Relaxed),
        );
    }
    let _ = fs::write(dir.join("summary.tsv"), summary);

    for (place, n) in shared.ignored.lock().unwrap().iter() {
        eprintln!("OVERFLOW (not counted) {place}: {n} time(s)");
    }
    let findings = shared.findings.lock().unwrap();
    for (sig, f) in findings.iter() {
        eprintln!(
            "FINDING {sig}: {} time(s); first {} #{} through {}: {}\n  saved {}",
            f.count,
            f.seed,
            f.index,
            f.reader,
            f.message,
            f.saved.display()
        );
    }
    assert!(
        findings.is_empty(),
        "{} distinct finding(s)",
        findings.len()
    );
}

/// Runs one saved input through one reader and says what came of it:
/// `KILN_FUZZ_REPLAY=<file> KILN_FUZZ_READER=<name>` (`wav`, `aiff`, `wavpack`,
/// `opus`, `id3`, `ape`, `mp4read`, `video_info`, or `sym:<ext>`).
#[test]
#[ignore = "runs one saved input; needs KILN_FUZZ_REPLAY and KILN_FUZZ_READER"]
fn replay() {
    let (Ok(path), Ok(name)) = (
        std::env::var("KILN_FUZZ_REPLAY"),
        std::env::var("KILN_FUZZ_READER"),
    ) else {
        eprintln!("replay: set KILN_FUZZ_REPLAY and KILN_FUZZ_READER");
        return;
    };
    let reader = match name.as_str() {
        "wav" => Reader::Wav,
        "aiff" => Reader::Aiff,
        "wavpack" => Reader::WavPack,
        "opus" => Reader::Opus,
        "id3" => Reader::Id3,
        "ape" => Reader::Ape,
        "mp4read" => Reader::Mp4,
        "video_info" => Reader::VideoInfo,
        other => sym(other.strip_prefix("sym:").expect("a reader name")),
    };
    let bytes = fs::read(&path).unwrap();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fuzz-lite");
    fs::create_dir_all(&dir).unwrap();
    let mut scratch = Scratch::new(&dir.join("replay.bin")).unwrap();
    let file = scratch.load(&bytes).unwrap();
    let began = Instant::now();
    match guarded(reader, &Arc::new(bytes), file) {
        Ok(out) => eprintln!("{out:?} in {:?}", began.elapsed()),
        Err(Fault::Panic(m)) => eprintln!("panic: {m}"),
        Err(Fault::Timeout) => eprintln!("timeout"),
        Err(Fault::Memory(n)) => eprintln!("held {} MiB", n >> 20),
    }
}
