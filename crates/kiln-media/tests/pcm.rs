use kiln_media::pcm::{aiff, wav};
use kiln_media::{
    Block, Channels, Codec, MediaError, Picture, Setting, Sink, SinkSpec, Source, Tags,
};
use std::fs::File;
use std::path::Path;

const BLOCK: usize = 8192;

#[derive(Clone, Copy)]
enum Fmt {
    Wav,
    Aiff,
}

fn make_sink(
    fmt: Fmt,
    path: &Path,
    rate: u32,
    channels: Channels,
    tags: &Tags,
    s: Setting,
) -> Box<dyn Sink> {
    let spec = SinkSpec {
        rate,
        channels,
        tags,
        setting: &s,
        source_bits: None,
        out: File::create(path).unwrap(),
    };
    match fmt {
        Fmt::Wav => wav::sink(spec).unwrap(),
        Fmt::Aiff => aiff::sink(spec).unwrap(),
    }
}

fn open(fmt: Fmt, path: &Path) -> kiln_media::Result<Box<dyn Source>> {
    let f = File::open(path).unwrap();
    match fmt {
        Fmt::Wav => wav::open(f),
        Fmt::Aiff => aiff::open(f),
    }
}

fn write_file(
    fmt: Fmt,
    path: &Path,
    rate: u32,
    channels: Channels,
    tags: &Tags,
    s: Setting,
    samples: &[f32],
) {
    let mut sink = make_sink(fmt, path, rate, channels, tags, s);
    for chunk in samples.chunks(BLOCK * channels.count as usize) {
        sink.write(&Block {
            channels: channels.count,
            rate,
            samples: chunk.to_vec(),
        })
        .unwrap();
    }
    sink.finish().unwrap();
}

fn read_all(src: &mut dyn Source) -> kiln_media::Result<Vec<f32>> {
    let mut out = Vec::new();
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    while src.read(&mut b)? {
        assert!(b.frames() <= BLOCK && b.frames() > 0);
        out.extend_from_slice(&b.samples);
    }
    Ok(out)
}

/// Integer samples over the whole range (both extremes included), as the f32
/// a decoder would give: `v / 2^(bits-1)`.
fn sweep(bits: u32, count: usize) -> Vec<f32> {
    let scale = (1u64 << (bits - 1)) as f64;
    let max = scale as i64 - 1;
    let min = -(scale as i64);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..count)
        .map(|i| {
            let v = match i {
                0 => min,
                1 => max,
                2 => 0,
                3 => -1,
                _ => {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (state % (max - min + 1) as u64) as i64 + min
                }
            };
            (v as f64 / scale) as f32
        })
        .collect()
}

fn stereo() -> Channels {
    Channels {
        count: 2,
        mask: 0x3,
    }
}

fn pcm(bits: u16) -> Setting {
    Setting::Pcm { bits, float: false }
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn round_trip(fmt: Fmt, ext: &str) {
    let dir = tmp();
    // 44.1k stereo 16-bit
    let path = dir.path().join(format!("a.{ext}"));
    let samples = sweep(16, 2 * 20_000);
    write_file(
        fmt,
        &path,
        44_100,
        stereo(),
        &Tags::default(),
        pcm(16),
        &samples,
    );
    let mut src = open(fmt, &path).unwrap();
    let info = src.info().clone();
    assert_eq!(
        info.codec,
        Codec::Pcm {
            bits: 16,
            float: false
        }
    );
    assert_eq!(info.rate, 44_100);
    assert_eq!(info.channels, stereo());
    assert_eq!(info.bits, Some(16));
    assert_eq!(info.frames, Some(20_000));
    assert_eq!(info.kbps, Some(44_100 * 2 * 16 / 1000));
    assert_eq!(read_all(&mut *src).unwrap(), samples);
    let p = src.position();
    assert_eq!(p.frames, 20_000);
    assert_eq!(p.total_bytes, std::fs::metadata(&path).unwrap().len());
    assert!(p.bytes <= p.total_bytes && p.bytes > 20_000 * 4);

    // 96k 24-bit 5.1
    let path = dir.path().join(format!("b.{ext}"));
    let six = Channels {
        count: 6,
        mask: 0x3F,
    };
    let samples = sweep(24, 6 * 10_000);
    write_file(fmt, &path, 96_000, six, &Tags::default(), pcm(24), &samples);
    let mut src = open(fmt, &path).unwrap();
    let info = src.info().clone();
    assert_eq!(info.rate, 96_000);
    assert_eq!(info.channels.count, 6);
    assert_eq!(info.bits, Some(24));
    assert_eq!(info.frames, Some(10_000));
    if matches!(fmt, Fmt::Wav) {
        assert_eq!(info.channels.mask, 0x3F);
    }
    assert_eq!(read_all(&mut *src).unwrap(), samples);
}

#[test]
fn wav_round_trips_bit_exact_16_and_24() {
    round_trip(Fmt::Wav, "wav");
}

#[test]
fn aiff_round_trips_bit_exact_16_and_24() {
    round_trip(Fmt::Aiff, "aiff");
}

#[test]
fn wav_and_aiff_round_trip_32_bit_and_float() {
    let dir = tmp();
    for (fmt, ext) in [(Fmt::Wav, "wav"), (Fmt::Aiff, "aif")] {
        // 32-bit float keeps every f32 exactly.
        let path = dir.path().join(format!("f.{ext}"));
        let samples: Vec<f32> = (0..2 * 1000)
            .map(|i| ((i as f32) * 0.37).sin() * 0.9)
            .collect();
        let float32 = Setting::Pcm {
            bits: 32,
            float: true,
        };
        write_file(
            fmt,
            &path,
            48_000,
            stereo(),
            &Tags::default(),
            float32,
            &samples,
        );
        let mut src = open(fmt, &path).unwrap();
        assert_eq!(
            src.info().codec,
            Codec::Pcm {
                bits: 32,
                float: true
            }
        );
        assert_eq!(read_all(&mut *src).unwrap(), samples);

        // 32-bit integer: a 24-bit-exact signal survives.
        let path = dir.path().join(format!("i.{ext}"));
        let samples = sweep(24, 2 * 1000);
        write_file(
            fmt,
            &path,
            48_000,
            stereo(),
            &Tags::default(),
            pcm(32),
            &samples,
        );
        let mut src = open(fmt, &path).unwrap();
        assert_eq!(src.info().bits, Some(32));
        assert_eq!(read_all(&mut *src).unwrap(), samples);
    }
}

#[test]
fn out_of_range_samples_clamp() {
    let dir = tmp();
    let path = dir.path().join("c.wav");
    let samples = [2.0f32, -2.0, 1.0, -1.0];
    write_file(
        Fmt::Wav,
        &path,
        44_100,
        stereo(),
        &Tags::default(),
        pcm(16),
        &samples,
    );
    let mut src = open(Fmt::Wav, &path).unwrap();
    assert_eq!(
        read_all(&mut *src).unwrap(),
        [32767.0 / 32768.0, -1.0, 32767.0 / 32768.0, -1.0]
    );
}

fn fake_png(len: usize) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut x = 7u32;
    while v.len() < len {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        v.push((x >> 24) as u8);
    }
    v
}

fn full_tags() -> Tags {
    Tags {
        title: Some("Füße – 東京".into()),
        artist: Some("Ana María".into()),
        album: Some("Álbum".into()),
        album_artist: Some("Various".into()),
        genre: Some("Jazz".into()),
        comment: Some("line one\nline two".into()),
        track: Some((3, Some(12))),
        disc: Some((1, Some(2))),
        year: Some("1999".into()),
        picture: Some(Picture {
            mime: "image/png".into(),
            data: fake_png(300 * 1024),
        }),
    }
}

#[test]
fn wav_keeps_tags_and_cover() {
    let dir = tmp();
    let path = dir.path().join("t.wav");
    let tags = full_tags();
    let samples = sweep(16, 2 * 500);
    write_file(Fmt::Wav, &path, 44_100, stereo(), &tags, pcm(16), &samples);
    let mut src = open(Fmt::Wav, &path).unwrap();
    assert_eq!(src.info().tags, tags);
    assert_eq!(read_all(&mut *src).unwrap(), samples);
}

#[test]
fn aiff_keeps_tags_and_cover() {
    let dir = tmp();
    let path = dir.path().join("t.aiff");
    let tags = full_tags();
    let samples = sweep(16, 2 * 501);
    write_file(Fmt::Aiff, &path, 44_100, stereo(), &tags, pcm(16), &samples);
    let mut src = open(Fmt::Aiff, &path).unwrap();
    assert_eq!(src.info().tags, tags);
    assert_eq!(read_all(&mut *src).unwrap(), samples);
}

#[test]
fn zero_length_wav_reads_as_empty() {
    let dir = tmp();
    for (fmt, ext) in [(Fmt::Wav, "wav"), (Fmt::Aiff, "aiff")] {
        let path = dir.path().join(format!("z.{ext}"));
        write_file(fmt, &path, 44_100, stereo(), &Tags::default(), pcm(16), &[]);
        let mut src = open(fmt, &path).unwrap();
        assert_eq!(src.info().frames, Some(0));
        let mut b = Block {
            channels: 0,
            rate: 0,
            samples: Vec::new(),
        };
        assert!(!src.read(&mut b).unwrap());
        assert_eq!(b.frames(), 0);
        assert!(!src.read(&mut b).unwrap());
    }
}

#[test]
fn odd_length_is_not_padded() {
    let dir = tmp();
    for (fmt, ext) in [(Fmt::Wav, "wav"), (Fmt::Aiff, "aiff")] {
        let path = dir.path().join(format!("o.{ext}"));
        let mono = Channels {
            count: 1,
            mask: 0x4,
        };
        let samples = sweep(24, 8193);
        write_file(
            fmt,
            &path,
            44_100,
            mono,
            &Tags::default(),
            pcm(24),
            &samples,
        );
        let mut src = open(fmt, &path).unwrap();
        assert_eq!(src.info().frames, Some(8193));
        let mut sizes = Vec::new();
        let mut b = Block {
            channels: 0,
            rate: 0,
            samples: Vec::new(),
        };
        let mut all = Vec::new();
        while src.read(&mut b).unwrap() {
            sizes.push(b.frames());
            all.extend_from_slice(&b.samples);
        }
        assert_eq!(sizes, [8192, 1]);
        assert_eq!(all, samples);
    }
}

fn damaged_code(r: kiln_media::Result<Box<dyn Source>>) -> String {
    match r {
        Err(MediaError::Damaged { code }) => code,
        Err(e) => panic!("expected Damaged, got {e:?}"),
        Ok(_) => panic!("expected Damaged, the file opened"),
    }
}

#[test]
fn a_truncated_wav_is_damaged() {
    // Found when the file is opened: the data chunk says more than the file holds.
    let dir = tmp();
    let path = dir.path().join("cut.wav");
    write_file(
        Fmt::Wav,
        &path,
        44_100,
        stereo(),
        &Tags::default(),
        pcm(16),
        &sweep(16, 2 * 4000),
    );
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 5001]).unwrap();
    assert_eq!(damaged_code(open(Fmt::Wav, &path)), "WAV");
}

#[test]
fn a_truncated_aiff_is_damaged() {
    let dir = tmp();
    let path = dir.path().join("cut.aiff");
    write_file(
        Fmt::Aiff,
        &path,
        44_100,
        stereo(),
        &Tags::default(),
        pcm(16),
        &sweep(16, 2 * 4000),
    );
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 5001]).unwrap();
    assert_eq!(damaged_code(open(Fmt::Aiff, &path)), "AIFF");
}

#[test]
fn a_wav_cut_mid_sample_reads_its_whole_frames_then_reports_damaged() {
    let dir = tmp();
    let path = dir.path().join("mid.wav");
    let samples = sweep(16, 2 * 100);
    write_file(
        Fmt::Wav,
        &path,
        44_100,
        stereo(),
        &Tags::default(),
        pcm(16),
        &samples,
    );
    // Cut 3 bytes off the end and make the header agree: 99 frames and 1 byte.
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(bytes.len() - 3);
    let data_len = (100 * 4 - 3) as u32;
    let at = bytes.windows(4).position(|w| w == b"data").unwrap();
    bytes[at + 4..at + 8].copy_from_slice(&data_len.to_le_bytes());
    let riff = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let mut src = open(Fmt::Wav, &path).unwrap();
    assert_eq!(src.info().frames, Some(99));
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    assert!(src.read(&mut b).unwrap());
    assert_eq!(b.samples, samples[..2 * 99]);
    match src.read(&mut b) {
        Err(MediaError::Damaged { code }) => assert_eq!(code, "WAV"),
        other => panic!("expected Damaged, got {other:?}"),
    }
}

#[test]
fn files_that_are_not_audio_are_damaged() {
    let dir = tmp();
    let path = dir.path().join("junk.wav");
    std::fs::write(&path, b"this is not a wave file at all").unwrap();
    assert_eq!(damaged_code(open(Fmt::Wav, &path)), "WAV");
    assert_eq!(damaged_code(open(Fmt::Aiff, &path)), "AIFF");
    std::fs::write(&path, b"").unwrap();
    assert_eq!(damaged_code(open(Fmt::Wav, &path)), "WAV");
}

#[test]
fn a_wav_in_a_format_kiln_cant_read_says_which() {
    let dir = tmp();
    let path = dir.path().join("mp3.wav");
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&0x55u16.to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&44_100u32.to_le_bytes());
    fmt.extend_from_slice(&16_000u32.to_le_bytes());
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&0u16.to_le_bytes());
    let mut f = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    f.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    f.extend_from_slice(&fmt);
    f.extend_from_slice(b"data\x04\0\0\0abcd");
    std::fs::write(&path, f).unwrap();
    match open(Fmt::Wav, &path) {
        Err(MediaError::Unsupported { codec }) => assert_eq!(codec, "MP3"),
        other => panic!("expected Unsupported, got {:?}", other.err()),
    }
}

/// A plain WAV written by hand: 8-bit unsigned, then 32-bit float64 variants.
fn handmade_wav(tag: u16, channels: u16, bits: u16, data: &[u8]) -> Vec<u8> {
    let align = channels * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&8000u32.to_le_bytes());
    fmt.extend_from_slice(&(8000 * align as u32).to_le_bytes());
    fmt.extend_from_slice(&align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    let mut f = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&fmt);
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend_from_slice(data);
    f
}

#[test]
fn wav_reads_8_bit_and_float64() {
    let dir = tmp();
    let path = dir.path().join("u8.wav");
    std::fs::write(&path, handmade_wav(1, 1, 8, &[0, 128, 255])).unwrap();
    let mut src = open(Fmt::Wav, &path).unwrap();
    assert_eq!(src.info().channels.mask, 0x4);
    assert_eq!(read_all(&mut *src).unwrap(), [-1.0, 0.0, 127.0 / 128.0]);

    let path = dir.path().join("f64.wav");
    let mut data = Vec::new();
    for v in [0.5f64, -0.25] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(&path, handmade_wav(3, 1, 64, &data)).unwrap();
    let mut src = open(Fmt::Wav, &path).unwrap();
    assert_eq!(
        src.info().codec,
        Codec::Pcm {
            bits: 64,
            float: true
        }
    );
    assert_eq!(read_all(&mut *src).unwrap(), [0.5, -0.25]);
}

#[test]
fn aiff_c_sowt_and_fl32_are_read() {
    let dir = tmp();
    // sowt: little-endian 16-bit in an AIFF-C
    let build = |ctype: &[u8; 4], bits: u16, data: &[u8]| {
        let mut comm = Vec::new();
        comm.extend_from_slice(&1u16.to_be_bytes());
        comm.extend_from_slice(&((data.len() / (bits as usize / 8)) as u32).to_be_bytes());
        comm.extend_from_slice(&bits.to_be_bytes());
        comm.extend_from_slice(&[0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0]); // 8000
        comm.extend_from_slice(ctype);
        comm.extend_from_slice(&[0, 0]); // empty pstring, padded
        let mut f = b"FORM\0\0\0\0AIFC".to_vec();
        f.extend_from_slice(b"COMM");
        f.extend_from_slice(&(comm.len() as u32).to_be_bytes());
        f.extend_from_slice(&comm);
        f.extend_from_slice(b"SSND");
        f.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
        f.extend_from_slice(&[0; 8]);
        f.extend_from_slice(data);
        f
    };
    let path = dir.path().join("sowt.aifc");
    std::fs::write(&path, build(b"sowt", 16, &[0x00, 0x40, 0x00, 0xC0])).unwrap();
    let mut src = open(Fmt::Aiff, &path).unwrap();
    assert_eq!(src.info().rate, 8000);
    assert_eq!(read_all(&mut *src).unwrap(), [0.5, -0.5]);

    let path = dir.path().join("fl32.aifc");
    let mut data = Vec::new();
    for v in [0.25f32, -0.75] {
        data.extend_from_slice(&v.to_be_bytes());
    }
    std::fs::write(&path, build(b"fl32", 32, &data)).unwrap();
    let mut src = open(Fmt::Aiff, &path).unwrap();
    assert_eq!(
        src.info().codec,
        Codec::Pcm {
            bits: 32,
            float: true
        }
    );
    assert_eq!(read_all(&mut *src).unwrap(), [0.25, -0.75]);

    let path = dir.path().join("ulaw.aifc");
    std::fs::write(&path, build(b"ulaw", 8, &[1, 2])).unwrap();
    match open(Fmt::Aiff, &path) {
        Err(MediaError::Unsupported { codec }) => assert_eq!(codec, "ulaw"),
        other => panic!("expected Unsupported, got {:?}", other.err()),
    }
}

#[test]
fn a_small_wav_keeps_its_riff_header_and_junk() {
    let dir = tmp();
    let path = dir.path().join("small.wav");
    write_file(
        Fmt::Wav,
        &path,
        44_100,
        stereo(),
        &Tags::default(),
        pcm(16),
        &sweep(16, 200),
    );
    let b = std::fs::read(&path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize,
        b.len() - 8
    );
    assert_eq!(&b[12..16], b"JUNK");
}

/// 4 GiB + 1 frame of 24-bit 7.1 silence. Writes 4 GiB to the temp dir and
/// removes it afterwards, so it only runs on request (the release workflow does).
#[test]
#[ignore = "writes 4 GiB"]
fn big_wav_becomes_rf64() {
    let dir = tmp();
    let path = dir.path().join("big.wav");
    let ch = Channels {
        count: 8,
        mask: 0x63F,
    };
    let frames: u64 = (1u64 << 32) / 24 + 1; // 24 bytes a frame: just past 4 GiB
    assert!(frames * 24 > (1u64 << 32));
    let mut sink = make_sink(Fmt::Wav, &path, 48_000, ch, &Tags::default(), pcm(24));
    let mut left = frames;
    while left > 0 {
        let n = left.min(BLOCK as u64) as usize;
        sink.write(&Block {
            channels: 8,
            rate: 48_000,
            samples: vec![0.0; n * 8],
        })
        .unwrap();
        left -= n as u64;
    }
    sink.finish().unwrap();

    let mut head = [0u8; 4];
    std::io::Read::read_exact(&mut File::open(&path).unwrap(), &mut head).unwrap();
    assert_eq!(&head, b"RF64");
    let mut src = open(Fmt::Wav, &path).unwrap();
    assert_eq!(src.info().frames, Some(frames));
    assert_eq!(src.info().channels, ch);
    let mut b = Block {
        channels: 0,
        rate: 0,
        samples: Vec::new(),
    };
    assert!(src.read(&mut b).unwrap());
    assert_eq!(b.frames(), BLOCK);
    assert_eq!(
        src.position().total_bytes,
        std::fs::metadata(&path).unwrap().len()
    );
}
