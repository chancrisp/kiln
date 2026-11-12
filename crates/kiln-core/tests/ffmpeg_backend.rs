//! The FFmpeg backend, with the video fixtures. Sound from audio files and
//! from MP4, M4V, MOV, MKV and WebM is Kiln's own (tests/audio.rs), and so
//! is video among MP4, M4V, MOV and MKV (tests/video.rs); FFmpeg keeps the
//! other video and the sound of the other video types.

#[path = "../../../tests/support/mod.rs"]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use kiln_core::{CancelToken, Engines, Error, Kiln, Options, Progress};
use support::{assert_output, files_in, probe, sample_for, stage};

/// Options with a private temp root, so tests can check it is cleaned up.
fn options(temp: &tempfile::TempDir) -> Options {
    Options {
        temp_root: Some(temp.path().join("work")),
        ..Options::default()
    }
}

fn assert_no_work_left(temp: &tempfile::TempDir) {
    let work = temp.path().join("work");
    if work.exists() {
        assert!(
            files_in(&work).is_empty(),
            "left behind: {:?}",
            files_in(&work)
        );
    }
}

#[test]
fn avi_to_mp3() {
    let (dir, input) = stage(&sample_for("avi"));
    let out = Kiln::new().convert(&input, "mp3", &options(&dir)).unwrap();
    assert_eq!(out, dir.path().join("sample.mp3"));
    assert_output(&out, "mp3");
    assert_no_work_left(&dir);
}

#[test]
fn mp4_to_webm_keeps_size_and_audio() {
    let (dir, input) = stage(&sample_for("mp4"));
    let out = Kiln::new().convert(&input, "webm", &options(&dir)).unwrap();
    assert_output(&out, "webm");
}

#[test]
fn video_to_audio_drops_the_picture() {
    let (dir, input) = stage(&sample_for("wmv"));
    let out = Kiln::new().convert(&input, "m4a", &options(&dir)).unwrap();
    assert_output(&out, "m4a");
    assert_eq!(probe(&out).video, 0);
}

#[test]
fn progress_rises_to_one() {
    let (dir, input) = stage(&sample_for("mov"));
    let mut seen = Vec::new();
    Kiln::new()
        .convert_with(
            &input,
            "webm",
            &options(&dir),
            &mut |p| seen.push(p),
            &CancelToken::new(),
        )
        .unwrap();
    let fractions: Vec<f32> = seen
        .iter()
        .filter_map(|p| match p {
            Progress::Fraction(f) => Some(*f),
            _ => None,
        })
        .collect();
    assert_eq!(fractions.first(), Some(&0.0), "{seen:?}");
    assert_eq!(fractions.last(), Some(&1.0), "{seen:?}");
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert!(fractions.iter().all(|f| (0.0..=1.0).contains(f)));
}

#[test]
fn cancel_stops_the_job_and_cleans_up() {
    // Ten minutes of sound in an AVI is long enough to cancel mid-way.
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("long.avi");
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=220:sample_rate=48000:duration=600")
        .args(["-c:a", "pcm_s16le"])
        .arg(&input)
        .status()
        .unwrap();
    assert!(ok.success());

    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let mut calls = 0;
    let err = Kiln::new()
        .convert_with(
            &input,
            "mp3",
            &options(&dir),
            &mut |_| {
                calls += 1;
                if calls == 3 {
                    trigger.cancel();
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    assert_eq!(files_in(dir.path()), ["long.avi", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn missing_ffmpeg_says_where_to_get_it() {
    let (dir, input) = stage(&sample_for("avi"));
    let opts = Options {
        engines: Engines {
            ffmpeg: Some(dir.path().join("no-such-dir").join("ffmpeg")),
            ..Engines::default()
        },
        ..options(&dir)
    };
    let err = Kiln::new().convert(&input, "mp3", &opts).unwrap_err();
    // FFmpeg comes with Kiln: there is no Get it, but another copy can be
    // chosen.
    assert_eq!(
        err.to_string(),
        "FFmpeg isn't installed. Choose a copy in Settings > Helper apps."
    );
    assert_eq!(files_in(dir.path()), ["sample.avi", "work"]);
    assert_no_work_left(&dir);
}

/// An AVI that starts like a RIFF file and holds nothing FFmpeg can read.
fn corrupt_avi(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("corrupt.avi");
    fs::copy(support::fixture("corrupt.wav"), &path).unwrap();
    path
}

#[test]
fn corrupt_media_reports_the_first_stderr_line() {
    let dir = tempfile::tempdir().unwrap();
    let input = corrupt_avi(&dir);
    let err = Kiln::new()
        .convert(&input, "mp3", &options(&dir))
        .unwrap_err();
    match &err {
        Error::EngineFailed {
            engine,
            summary,
            details,
        } => {
            assert_eq!(*engine, "FFmpeg");
            assert!(!summary.is_empty() && !summary.contains('\n'));
            assert!(details.contains(summary.as_str()));
        }
        other => panic!("expected EngineFailed, got {other:?}"),
    }
    assert!(err
        .to_string()
        .starts_with("FFmpeg couldn't convert the file: "));
    assert_eq!(files_in(dir.path()), ["corrupt.avi", "work"]);
    assert_no_work_left(&dir);
}

#[test]
fn hostile_file_names_are_just_names() {
    // Looks like options, protocols and shell syntax. None of it may act.
    let dir = tempfile::tempdir().unwrap();
    let name = "-y; echo hi & http:x $(ls).avi";
    let input = dir.path().join(name);
    fs::copy(sample_for("avi"), &input).unwrap();
    let out = Kiln::new().convert(&input, "flac", &options(&dir)).unwrap();
    assert_eq!(
        out.file_name().unwrap().to_string_lossy(),
        "-y; echo hi & http_x $(ls).flac"
    );
    assert_output(&out, "flac");
}

/// Find ffmpeg on PATH the way the OS would, without a shell tool.
fn ffmpeg_on_path() -> PathBuf {
    let exe = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(&exe))
        .find(|p| p.is_file())
        .expect("ffmpeg must be on PATH for these tests")
}

#[test]
fn a_configured_engine_path_is_used() {
    let (dir, input) = stage(&sample_for("ogv"));
    let opts = Options {
        engines: Engines {
            ffmpeg: Some(ffmpeg_on_path()),
            ..Engines::default()
        },
        ..options(&dir)
    };
    let out = Kiln::new().convert(&input, "wav", &opts).unwrap();
    assert_output(&out, "wav");
}

/// A one-second file in `dir` with two pictures (64x48, then 128x96) when
/// `pictures` is 2, and two sound tracks: a mono one first, then a stereo
/// one. The second of each is flagged as the default, as a file with its
/// main sound second is. FFmpeg's own pick is the flagged track, or else the
/// larger picture and the sound with more channels, so it takes the second
/// of each unless it is told the first.
fn two_of_each(dir: &Path, name: &str, pictures: usize, sound_codec: &str) -> PathBuf {
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"]
        .map(String::from)
        .to_vec();
    for size in ["64x48", "128x96"].into_iter().take(pictures) {
        args.extend(["-f", "lavfi", "-i"].map(String::from));
        args.push(format!("testsrc=size={size}:rate=10:duration=1"));
    }
    for hz in [440, 880] {
        args.extend(["-f", "lavfi", "-i"].map(String::from));
        args.push(format!("sine=frequency={hz}:duration=1"));
    }
    for n in 0..pictures + 2 {
        args.extend(["-map".into(), n.to_string()]);
    }
    args.extend(["-ac:a:0", "1", "-ac:a:1", "2", "-c:v", "mpeg4", "-c:a"].map(String::from));
    args.push(sound_codec.to_owned());
    if pictures == 2 {
        args.extend(["-disposition:v:0", "0", "-disposition:v:1", "default"].map(String::from));
    }
    args.extend(["-disposition:a:0", "0", "-disposition:a:1", "default"].map(String::from));
    let out = dir.join(name);
    args.push(out.display().to_string());
    let made = Command::new("ffmpeg").args(&args).output().unwrap();
    assert!(
        made.status.success(),
        "ffmpeg {args:?}: {}",
        String::from_utf8_lossy(&made.stderr)
    );
    out
}

/// The channel count of each sound stream of `path`, in order.
fn sound_channels(path: &Path) -> Vec<u32> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a"])
        .args(["-show_entries", "stream=channels", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    assert!(out.status.success(), "ffprobe failed on {}", path.display());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

#[test]
fn ffmpeg_takes_the_first_video_and_sound() {
    let dir = tempfile::tempdir().unwrap();
    // MKV to WebM is FFmpeg's. The first picture is the small one and the
    // first sound is mono.
    let mkv = two_of_each(dir.path(), "two.mkv", 2, "aac");
    assert_eq!(sound_channels(&mkv), [1, 2]);
    let out = Kiln::new().convert(&mkv, "webm", &options(&dir)).unwrap();
    assert_output(&out, "webm");
    let p = probe(&out);
    assert_eq!((p.video, p.audio), (1, 1));
    assert_eq!((p.width, p.height), (Some(64), Some(48)));
    assert_eq!(sound_channels(&out), [1]);

    // An AVI's sound goes to MP3 through FFmpeg too.
    let avi = two_of_each(dir.path(), "two.avi", 1, "pcm_s16le");
    assert_eq!(sound_channels(&avi), [1, 2]);
    let out = Kiln::new().convert(&avi, "mp3", &options(&dir)).unwrap();
    assert_output(&out, "mp3");
    assert_eq!(sound_channels(&out), [1]);
}

#[test]
fn a_cover_picture_is_never_taken_as_the_video() {
    let dir = tempfile::tempdir().unwrap();
    // A sound with its cover attached, as a tagged MKV carries it. FFmpeg
    // reads the cover as a picture stream, flagged as an attached picture.
    let cover = dir.path().join("cover.jpg");
    let mkv = dir.path().join("song.mkv");
    let ffmpeg = || {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
        cmd
    };
    let made = |cmd: &mut Command| {
        let out = cmd.output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "ffmpeg {cmd:?}: {err}");
    };
    made(
        ffmpeg()
            .args([
                "-f",
                "lavfi",
                "-i",
                "color=red:size=64x64",
                "-frames:v",
                "1",
            ])
            .arg(&cover),
    );
    made(
        ffmpeg()
            .args(["-f", "lavfi", "-i", "sine=duration=1", "-c:a", "libopus"])
            .arg("-attach")
            .arg(&cover)
            .args(["-metadata:s:t", "mimetype=image/jpeg"])
            .arg(&mkv),
    );
    assert_eq!(probe(&mkv).video, 1, "the cover reads as a picture");

    // MKV to WebM is FFmpeg's: the sound, and no one-frame video of the
    // cover.
    let out = Kiln::new().convert(&mkv, "webm", &options(&dir)).unwrap();
    let p = probe(&out);
    assert_eq!((p.video, p.audio), (0, 1));
    assert_no_work_left(&dir);
}

/// R10: the FFmpeg chosen in Settings, then the one that comes with Kiln
/// (`ffmpeg\bin` beside the program; a temp folder stands in for the
/// program's), then the one on PATH.
#[test]
fn ffmpeg_is_looked_for_in_settings_then_beside_kiln_then_on_path() {
    use kiln_core::ffmpeg::{bundled_dir_in, ffprobe_in, find, program_in};
    let root = tempfile::tempdir().unwrap();
    let file = |p: &Path| {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"").unwrap();
        p.to_path_buf()
    };
    let app = root.path().join("app");
    let bin = app.join("ffmpeg").join("bin");
    let bundled_exe = file(&bin.join("ffmpeg.exe"));
    let on_path = file(&root.path().join("path").join("ffmpeg.exe"));
    let chosen = file(&root.path().join("chosen").join("ffmpeg.exe"));
    let path_var = std::env::join_paths([on_path.parent().unwrap()]).unwrap();
    let empty = std::ffi::OsString::new();

    let bundled = bundled_dir_in(&app);
    assert_eq!(bundled.as_deref(), Some(bin.as_path()));
    let bundled = bundled.as_deref();
    assert_eq!(
        find(Some(&chosen), bundled, &path_var),
        Some(chosen.clone())
    );
    assert_eq!(find(None, bundled, &path_var), Some(bundled_exe.clone()));
    assert_eq!(find(None, None, &path_var), Some(on_path.clone()));
    assert_eq!(find(None, None, &empty), None);
    // A chosen copy that is gone isn't swapped for another.
    let gone = root.path().join("gone").join("ffmpeg.exe");
    assert_eq!(find(Some(&gone), bundled, &path_var), None);
    // A program folder with no ffmpeg\bin\ffmpeg.exe has no bundled copy.
    assert_eq!(bundled_dir_in(root.path()), None);
    assert_eq!(bundled_dir_in(&root.path().join("path")), None);

    // What a job starts, with FFprobe from the same place.
    assert_eq!(program_in(Some(&chosen), bundled), chosen);
    assert_eq!(
        ffprobe_in(Some(&chosen), bundled),
        chosen.with_file_name("ffprobe.exe")
    );
    assert_eq!(program_in(None, bundled), bundled_exe);
    assert_eq!(ffprobe_in(None, bundled), bin.join("ffprobe.exe"));
    assert_eq!(program_in(None, None), PathBuf::from("ffmpeg"));
    assert_eq!(ffprobe_in(None, None), PathBuf::from("ffprobe"));
}
