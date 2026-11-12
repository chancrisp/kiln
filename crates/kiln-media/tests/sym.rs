//! The Symphonia source and `probe`, against the fixtures in tests/data.
//! `tests/data/make.ps1` writes them; they are checked in.

use kiln_media::pcm::wav;
use kiln_media::tags::id3;
use kiln_media::{probe, sym, Block, Codec, MediaError, Picture, Source, SourceInfo, Tags};
use std::fs::File;
use std::path::{Path, PathBuf};

const BLOCK: usize = 8192;

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}

fn open(path: &Path) -> kiln_media::Result<Box<dyn Source>> {
    sym::open(File::open(path).unwrap(), &ext_of(path))
}

/// Every sample of a source, checking the block rules on the way.
fn read_all(src: &mut dyn Source) -> kiln_media::Result<Vec<f32>> {
    let channels = src.info().channels.count;
    let mut out = Vec::new();
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    let mut short_block_seen = false;
    while src.read(&mut b)? {
        assert!(
            b.frames() > 0 && b.frames() <= BLOCK,
            "{} frames",
            b.frames()
        );
        assert!(!short_block_seen, "a short block came before the last one");
        short_block_seen = b.frames() < BLOCK;
        assert_eq!(b.channels, channels);
        assert_eq!(b.rate, src.info().rate);
        out.extend_from_slice(&b.samples);
    }
    assert_eq!(b.frames(), 0, "the final read left samples in the block");
    assert_eq!(
        src.position().frames as usize * channels as usize,
        out.len()
    );
    Ok(out)
}

fn decoded_frames(path: &Path) -> usize {
    let mut src = open(path).unwrap();
    let ch = src.info().channels.count as usize;
    read_all(&mut *src).unwrap().len() / ch
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

struct Case {
    file: &'static str,
    codec: Codec,
    rate: u32,
    channels: u16,
    frames: u64,
    /// The codec's frame size: how far the length may be from the truth.
    slack: u64,
}

fn pcm16() -> Codec {
    Codec::Pcm {
        bits: 16,
        float: false,
    }
}

#[test]
fn probes_each_format() {
    let cases = [
        Case {
            file: "tone.mp3",
            codec: Codec::Mp3,
            rate: 44100,
            channels: 2,
            frames: 132_300,
            slack: 1152,
        },
        Case {
            file: "tone_aac.m4a",
            codec: Codec::Aac,
            rate: 44100,
            channels: 2,
            frames: 132_300,
            slack: 1024,
        },
        Case {
            file: "tone.aac",
            codec: Codec::Aac,
            rate: 44100,
            channels: 2,
            frames: 44_100,
            slack: 1024,
        },
        Case {
            file: "tone_alac.m4a",
            codec: Codec::Alac,
            rate: 44100,
            channels: 2,
            frames: 132_300,
            slack: 4096,
        },
        Case {
            file: "tone.flac",
            codec: Codec::Flac,
            rate: 44100,
            channels: 2,
            frames: 132_300,
            slack: 0,
        },
        Case {
            file: "tone.ogg",
            codec: Codec::Vorbis,
            rate: 44100,
            channels: 2,
            frames: 132_300,
            slack: 2048,
        },
        Case {
            file: "tone.wav",
            codec: pcm16(),
            rate: 44100,
            channels: 1,
            frames: 44_100,
            slack: 0,
        },
        Case {
            file: "tone.aiff",
            codec: pcm16(),
            rate: 44100,
            channels: 1,
            frames: 44_100,
            slack: 0,
        },
    ];
    for c in cases {
        let path = data(c.file);
        let info = probe(&path).unwrap_or_else(|e| panic!("{}: {e}", c.file));
        assert_eq!(info.codec, c.codec, "{}", c.file);
        assert_eq!(info.rate, c.rate, "{}", c.file);
        assert_eq!(info.channels.count, c.channels, "{}", c.file);
        let mask = if c.channels == 2 { 0x3 } else { 0x4 };
        assert_eq!(info.channels.mask, mask, "{}", c.file);
        // ADTS has no length: only the decode below can say how long it is.
        if let Some(f) = info.frames {
            assert!(f.abs_diff(c.frames) <= c.slack, "{}: {f} frames", c.file);
        } else {
            assert_eq!(c.file, "tone.aac");
        }
        // AAC decoders hand back the encoder's priming and padding too.
        let n = decoded_frames(&path) as u64;
        assert!(
            n.abs_diff(c.frames) <= c.slack * 2,
            "{}: decoded {n} frames",
            c.file
        );
    }
}

#[test]
fn lossless_codecs_decode_bit_for_bit() {
    // The WAV files are the source of the FLAC and ALAC files; the PCM reader
    // is the reference.
    for (lossless, wav_file) in [
        ("tone16.flac", "tone.wav"),
        ("tone16_alac.m4a", "tone.wav"),
        ("tone24.flac", "tone24.wav"),
    ] {
        let mut want = wav::open(File::open(data(wav_file)).unwrap()).unwrap();
        let want = read_all(&mut *want).unwrap();
        let mut got = open(&data(lossless)).unwrap();
        let bits = got.info().bits;
        let got = read_all(&mut *got).unwrap();
        assert_eq!(got.len(), want.len(), "{lossless}");
        assert!(got == want, "{lossless} differs from {wav_file}");
        assert_eq!(bits, Some(if wav_file == "tone24.wav" { 24 } else { 16 }));
    }
}

#[test]
fn symphonia_and_the_pcm_reader_agree_on_wav() {
    let mut a = wav::open(File::open(data("tone.wav")).unwrap()).unwrap();
    let a = read_all(&mut *a).unwrap();
    let mut b = open(&data("tone.wav")).unwrap();
    assert_eq!(b.info().codec, pcm16());
    assert!(read_all(&mut *b).unwrap() == a);
}

#[test]
fn the_bitrate_is_worked_out_from_the_length() {
    let kbps = |f: &str| probe(&data(f)).unwrap().kbps;
    assert!(
        matches!(kbps("tone.mp3"), Some(120..=136)),
        "{:?}",
        kbps("tone.mp3")
    );
    assert!(
        matches!(kbps("tone_aac.m4a"), Some(85..=110)),
        "{:?}",
        kbps("tone_aac.m4a")
    );
    // Video files carry more than the sound, so they give none.
    assert_eq!(kbps("av.mp4"), None);
    // No length, no bitrate.
    assert_eq!(kbps("vbr_noxing.mp3"), None);
}

#[test]
fn reads_tags_and_cover() {
    let info = probe(&data("tagged.mp3")).unwrap();
    let t = &info.tags;
    assert_eq!(
        t.title.as_deref(),
        Some("F\u{fc}\u{df}e \u{2013} \u{6771}\u{4eac}")
    );
    assert_eq!(t.track, Some((3, Some(12))));
    let pic = t.picture.as_ref().expect("a cover");
    assert_eq!(pic.mime, "image/png");
    assert_eq!(pic.data, std::fs::read(data("cover.png")).unwrap());
    // The cover and tags don't change what plays.
    assert_eq!(info.codec, Codec::Mp3);
    assert!(decoded_frames(&data("tagged.mp3")).abs_diff(44_100) <= 1152);
}

#[test]
fn a_video_without_sound_is_no_audio() {
    assert!(matches!(
        probe(&data("silent.mp4")),
        Err(MediaError::NoAudio)
    ));
    assert!(matches!(
        open(&data("silent.mp4")),
        Err(MediaError::NoAudio)
    ));
}

/// What Symphonia itself says of a file it can't decode. (`probe` goes on to
/// the codec table's other readers; see `tests/every_target.rs`.)
fn unsupported(file: &str) -> String {
    match open(&data(file)) {
        Err(MediaError::Unsupported { codec }) => codec,
        Err(other) => panic!("{file}: {other:?}"),
        Ok(_) => panic!("{file}: Symphonia read it"),
    }
}

#[test]
fn dts_in_mkv_is_unsupported() {
    assert_eq!(unsupported("dts.mkv"), "DTS");
}

#[test]
fn codecs_symphonia_cannot_decode_are_unsupported_by_name() {
    assert_eq!(unsupported("ac3.mkv"), "AC3");
    assert_eq!(unsupported("ac3.mp4"), "AC3");
    assert_eq!(unsupported("tone_opus.ogg"), "Opus");
    assert_eq!(unsupported("tone.wv"), "WavPack");
}

#[test]
fn mp4_mov_mkv_webm_audio_decodes() {
    for f in ["av.mp4", "av.mov", "av.mkv", "av.webm"] {
        let path = data(f);
        let info = probe(&path).unwrap_or_else(|e| panic!("{f}: {e}"));
        assert_eq!(info.rate, 44100, "{f}");
        assert_eq!(info.channels.count, 2, "{f}");
        let want = if f == "av.webm" {
            Codec::Vorbis
        } else {
            Codec::Aac
        };
        assert_eq!(info.codec, want, "{f}");
        let n = decoded_frames(&path) as u64;
        assert!(n.abs_diff(44_100) <= 2048, "{f}: decoded {n} frames");
        let known = info.frames.expect("a length");
        assert!(known.abs_diff(44_100) <= 2048, "{f}: {known} frames");
    }
}

/// Bytes from a fixed-seed generator.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

#[test]
fn garbage_is_damaged() {
    let dir = tmp();
    let p = dir.path().join("garbage.mp3");
    std::fs::write(&p, noise(4096)).unwrap();
    match probe(&p) {
        Err(MediaError::Damaged { code }) => assert_eq!(code, "MP3"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_file_with_no_extension_is_damaged_as_a_file() {
    let dir = tmp();
    let p = dir.path().join("garbage");
    std::fs::write(&p, noise(4096)).unwrap();
    assert!(matches!(
        probe(&p),
        Err(MediaError::Damaged { code }) if code == "FILE"
    ));
}

/// `tone.mp3` with `prefix` in front of it, in a temp dir.
fn mp3_with_prefix(dir: &Path, prefix: &[u8]) -> PathBuf {
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(&std::fs::read(data("tone.mp3")).unwrap());
    let p = dir.join("t.mp3");
    std::fs::write(&p, bytes).unwrap();
    p
}

fn syncsafe(n: u32) -> [u8; 4] {
    [
        ((n >> 21) & 0x7F) as u8,
        ((n >> 14) & 0x7F) as u8,
        ((n >> 7) & 0x7F) as u8,
        (n & 0x7F) as u8,
    ]
}

fn id3_header(body_len: usize) -> Vec<u8> {
    let mut h = vec![b'I', b'D', b'3', 4, 0, 0];
    h.extend_from_slice(&syncsafe(body_len as u32));
    h
}

fn frame(id: &[u8; 4], size: u32, body: &[u8]) -> Vec<u8> {
    let mut f = id.to_vec();
    f.extend_from_slice(&syncsafe(size));
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(body);
    f
}

#[test]
fn huge_or_broken_tags_do_not_fail_decoding() {
    let clean = decoded_frames(&data("tone.mp3"));
    let dir = tmp();

    // A 5 MB cover.
    let tags = Tags {
        title: Some("Big".into()),
        picture: Some(Picture {
            mime: "image/png".into(),
            data: noise(5_000_000),
        }),
        ..Tags::default()
    };
    let p = mp3_with_prefix(dir.path(), &id3::write_v24(&tags));
    assert_eq!(decoded_frames(&p), clean);
    let info = probe(&p).unwrap();
    assert_eq!(info.codec, Codec::Mp3);
    assert_eq!(info.tags.title.as_deref(), Some("Big"));

    // A frame whose length runs past the end of the tag.
    let mut body = frame(b"TIT2", 6, &[3, b'H', b'e', b'l', b'l', b'o']);
    body.extend(frame(b"TALB", 0x0FFF_FFFF, &[3, b'x']));
    body.resize(200, 0);
    let mut tag = id3_header(body.len());
    tag.extend(body);
    let p = mp3_with_prefix(dir.path(), &tag);
    assert_eq!(decoded_frames(&p), clean);
    probe(&p).unwrap();

    // A frame whose length is wrong in the other direction, so what follows is junk.
    let mut body = frame(b"TIT2", 2, &[3, b'H', b'e', b'l', b'l', b'o']);
    body.extend(noise(100));
    let mut tag = id3_header(body.len());
    tag.extend(body);
    let p = mp3_with_prefix(dir.path(), &tag);
    assert_eq!(decoded_frames(&p), clean);
    probe(&p).unwrap();
}

#[test]
fn a_vbr_mp3_without_a_header_has_no_length() {
    let info = probe(&data("vbr_noxing.mp3")).unwrap();
    assert_eq!(info.codec, Codec::Mp3);
    assert_eq!(info.frames, None, "an estimate was passed off as a length");
    // It still decodes, to about eight seconds.
    let n = decoded_frames(&data("vbr_noxing.mp3")) as u64;
    assert!(n.abs_diff(8 * 44_100) <= 4096, "{n}");
}

#[test]
fn a_cbr_mp3_without_a_header_has_its_length_and_bitrate() {
    // 128 kbps CBR with no Xing or Info frame, an ID3v2 tag in front and an
    // ID3v1 tag at the end, as older rippers wrote.
    let file = data("cbr_noxing.mp3");
    let info = probe(&file).unwrap();
    assert_eq!(info.codec, Codec::Mp3);
    let n = decoded_frames(&file) as u64;
    let frames = info.frames.expect("a CBR file's length");
    assert!(frames.abs_diff(n) <= 1152, "{frames} against {n} decoded");
    assert!(
        info.kbps.is_some_and(|k| k.abs_diff(128) <= 1),
        "{:?}",
        info.kbps
    );
    // About a second, so the row reads 0:01.
    let secs = info.duration_secs().unwrap();
    assert!((1.0..1.1).contains(&secs), "{secs}");
}

#[test]
fn a_chained_ogg_decodes_whole() {
    // Two Ogg Vorbis files end to end, as stream rips and joined files are:
    // the second part follows the first instead of being dropped.
    let dir = tmp();
    let one = std::fs::read(data("tone.ogg")).unwrap();
    let chained = dir.path().join("chained.ogg");
    std::fs::write(&chained, [one.clone(), one].concat()).unwrap();
    let alone = decoded_frames(&data("tone.ogg"));
    assert_eq!(decoded_frames(&chained), 2 * alone);
    let mut src = open(&chained).unwrap();
    let both = read_all(&mut *src).unwrap();
    let mut first = open(&data("tone.ogg")).unwrap();
    let part = read_all(&mut *first).unwrap();
    assert_eq!(&both[..part.len()], &part[..]);
    assert_eq!(&both[part.len()..], &part[..]);
}

#[test]
fn a_chained_ogg_whose_channels_change_says_so() {
    // Stereo, then a mono part: one pipeline can't carry both, so the file
    // is refused rather than cut short.
    let dir = tmp();
    let mono = dir.path().join("mono.ogg");
    kiln_media::convert(
        &data("tone.wav"),
        &mono,
        kiln_media::Target::Ogg,
        kiln_media::Preset::High,
        &kiln_media::Tune::default(),
        &mut |_| {},
        &|| false,
    )
    .unwrap();
    assert_eq!(probe(&mono).unwrap().channels.count, 1);
    let chained = dir.path().join("chained.ogg");
    let joined = [
        std::fs::read(data("tone.ogg")).unwrap(),
        std::fs::read(&mono).unwrap(),
    ]
    .concat();
    std::fs::write(&chained, joined).unwrap();
    let mut src = open(&chained).unwrap();
    match read_all(&mut *src) {
        Err(MediaError::Unsupported { codec }) => assert_eq!(codec, "a chained stream"),
        other => panic!("{:?}", other.map(|s| s.len())),
    }
}

#[test]
fn position_follows_the_decode() {
    let mut src = open(&data("tone.mp3")).unwrap();
    let total = std::fs::metadata(data("tone.mp3")).unwrap().len();
    assert_eq!(src.position().frames, 0);
    assert_eq!(src.position().total_bytes, total);
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    assert!(src.read(&mut b).unwrap());
    let p = src.position();
    assert_eq!(p.frames, BLOCK as u64);
    assert!(p.bytes > 0 && p.bytes <= total, "{p:?}");
    while src.read(&mut b).unwrap() {}
    let end = src.position();
    assert!(end.bytes <= total && end.bytes >= total - 1024, "{end:?}");
    // At the end a source keeps saying so.
    assert!(!src.read(&mut b).unwrap());
    assert_eq!(b.frames(), 0);
}

#[test]
fn a_file_cut_short_gives_what_is_there() {
    let dir = tmp();
    for f in ["tone.flac", "tone.mp3", "tone.ogg"] {
        let bytes = std::fs::read(data(f)).unwrap();
        let p = dir.path().join(f);
        std::fs::write(&p, &bytes[..bytes.len() * 2 / 3]).unwrap();
        let n = decoded_frames(&p);
        assert!(n > 44_100 && n < 132_300, "{f}: {n}");
    }
}

#[test]
fn a_bad_packet_in_the_middle_is_skipped() {
    let dir = tmp();
    let mut bytes = std::fs::read(data("tone.flac")).unwrap();
    let mid = bytes.len() / 2;
    for (i, b) in noise(64).into_iter().enumerate() {
        bytes[mid + i] = b;
    }
    let p = dir.path().join("t.flac");
    std::fs::write(&p, bytes).unwrap();
    let mut src = open(&p).unwrap();
    let got = read_all(&mut *src).unwrap();
    // A frame or two of 4096 are lost; the rest plays.
    let n = got.len() / 2;
    assert!(n > 132_300 - 4 * 4096 && n < 132_300, "{n}");
}

/// CRC-8 of a FLAC frame header (polynomial 0x07).
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16 of a whole FLAC frame but its last two bytes (polynomial 0x8005).
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Where each frame of tone.flac starts, and how many sample frames each holds.
fn flac_frames(bytes: &[u8]) -> (Vec<usize>, usize) {
    let mut starts = Vec::new();
    let mut p = 0;
    while p + 6 <= bytes.len() {
        // sync, two header bytes, the frame number (one byte below 128), CRC-8
        let k = starts.len();
        if bytes[p] == 0xFF
            && bytes[p + 1] == 0xF8
            && bytes[p + 4] as usize == k
            && bytes[p + 5] == crc8(&bytes[p..p + 5])
        {
            starts.push(p);
        }
        p += 1;
    }
    // The block size from the first header's code, which is a fixed size here.
    let block = match bytes[starts[0] + 2] >> 4 {
        c @ 2..=5 => 576usize << (c - 2),
        c @ 8..=15 => 256usize << (c - 8),
        c => panic!("block size code {c} needs extra header bytes"),
    };
    (starts, block)
}

/// `tone.flac` with the listed frames made undecodable but still intact as far
/// as the container goes: the first subframe's padding bit is set and the
/// frame's CRC-16 put right, so the demuxer hands the frame over and the
/// decoder refuses it.
fn flac_with_bad_frames(bad: &[usize]) -> (Vec<u8>, usize) {
    let mut bytes = std::fs::read(data("tone.flac")).unwrap();
    let (starts, block) = flac_frames(&bytes);
    assert!(starts.len() > 25, "{} frames", starts.len());
    for &k in bad {
        let end = starts.get(k + 1).copied().unwrap_or(bytes.len());
        bytes[starts[k] + 6] |= 0x80;
        let crc = crc16(&bytes[starts[k]..end - 2]);
        bytes[end - 2..end].copy_from_slice(&crc.to_be_bytes());
    }
    (bytes, block)
}

fn read_flac(bytes: Vec<u8>) -> kiln_media::Result<usize> {
    let dir = tmp();
    let p = dir.path().join("t.flac");
    std::fs::write(&p, bytes).unwrap();
    let mut src = open(&p)?;
    Ok(read_all(&mut *src)?.len() / 2)
}

#[test]
fn ten_bad_packets_are_skipped() {
    // Ten bad frames, not next to each other: the count is of all of them.
    let bad: Vec<usize> = (0..10).map(|i| 1 + 2 * i).collect();
    let (bytes, block) = flac_with_bad_frames(&bad);
    let n = read_flac(bytes).unwrap();
    assert_eq!(n, 132_300 - 10 * block);
}

#[test]
fn the_eleventh_bad_packet_is_damaged() {
    let bad: Vec<usize> = (0..11).map(|i| 1 + 2 * i).collect();
    let (bytes, _) = flac_with_bad_frames(&bad);
    match read_flac(bytes) {
        Err(MediaError::Damaged { code }) => assert_eq!(code, "FLAC"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_bad_first_packet_is_damaged_at_open() {
    let (bytes, _) = flac_with_bad_frames(&[0]);
    let dir = tmp();
    let p = dir.path().join("t.flac");
    std::fs::write(&p, bytes).unwrap();
    match open(&p) {
        Err(MediaError::Damaged { code }) => assert_eq!(code, "FLAC"),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("opened"),
    }
}

#[test]
fn info_is_the_same_from_open_and_probe() {
    fn same(a: &SourceInfo, b: &SourceInfo) {
        assert_eq!(a, b);
    }
    let from_open = open(&data("tone.flac")).unwrap();
    same(from_open.info(), &probe(&data("tone.flac")).unwrap());
}
