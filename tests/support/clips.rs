//! Short generated videos for the size estimate and Make smaller, made with
//! FFmpeg's own test sources (the pictures are drawn, not filmed) and kept in
//! the build folder, so a test run makes each at most once. Nothing here is
//! checked in. Included with
//! `#[path = "../../../tests/support/clips.rs"] mod clips;` or through
//! `support`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A kind of picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clip {
    /// A screen recording: flat colours, sharp edges, one small thing
    /// moving. Very few bits a pixel. 960x540, 30 fps, 10 s, silent.
    Screen,
    /// Film-like: a zoom into detail that keeps changing, with stereo AAC
    /// sound. 960x540, 30 fps, 10 s.
    Film,
    /// Noisy: a moving pattern under heavy grain. About 30 MB. 960x540,
    /// 30 fps, 10 s, silent.
    Noisy,
    /// Full HD with grain and stereo AAC sound, for a smaller picture and no
    /// sound. 1920x1080, 30 fps, 3 s.
    FullHd,
}

impl Clip {
    pub const ESTIMATED: [Clip; 3] = [Clip::Screen, Clip::Film, Clip::Noisy];

    pub fn name(self) -> &'static str {
        match self {
            Clip::Screen => "screen",
            Clip::Film => "film",
            Clip::Noisy => "noisy",
            Clip::FullHd => "full-hd",
        }
    }

    /// FFmpeg's arguments after its first options, up to the output name.
    fn args(self) -> Vec<String> {
        let screen = [
            "color=c=0xf3f3f3:s=960x540:r=30:d=10",
            "drawbox=x=40:y=40:w=420:h=300:color=0x2b6cb0:t=fill",
            "drawbox=x=500:y=60:w=400:h=20:color=0x333333:t=fill",
            "drawbox=x=500:y=100:w=380:h=12:color=0x777777:t=fill",
            "drawbox=x=500:y=130:w=360:h=12:color=0x777777:t=fill",
            "drawbox=x='40+mod(t*90\\,820)':y='400+20*sin(t)':w=24:h=24:color=0x000000:t=fill",
        ]
        .join(",");
        let (picture, sound, crf): (String, Option<u32>, &str) = match self {
            Clip::Screen => (screen, None, "23"),
            Clip::Film => (
                "mandelbrot=s=960x540:r=30:end_scale=0.01,trim=duration=10".into(),
                Some(10),
                "23",
            ),
            Clip::Noisy => (
                "testsrc2=s=960x540:r=30:d=10,noise=alls=24:allf=t+u".into(),
                None,
                "20",
            ),
            Clip::FullHd => (
                "testsrc2=s=1920x1080:r=30:d=3,noise=alls=12:allf=t+u".into(),
                Some(3),
                "23",
            ),
        };
        let mut args: Vec<String> = vec!["-f".into(), "lavfi".into(), "-i".into(), picture];
        if let Some(secs) = sound {
            args.extend(["-f".into(), "lavfi".into(), "-i".into()]);
            args.push(format!(
                "sine=frequency=330:sample_rate=48000:duration={secs}"
            ));
        }
        args.extend(
            [
                "-c:v", "libx264", "-preset", "veryfast", "-crf", crf, "-pix_fmt", "yuv420p",
                "-threads", "4",
            ]
            .map(String::from),
        );
        if sound.is_some() {
            args.extend(["-c:a", "aac", "-b:a", "128k", "-ac", "2", "-shortest"].map(String::from));
        }
        args
    }
}

/// Whether FFmpeg is on PATH with libx264 and AAC, which the clips are made
/// with; the test says it is skipped when not.
pub fn can_make_clips(test: &str) -> bool {
    let listed = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .ok()
        .filter(|o| o.status.success());
    let Some(listed) = listed else {
        eprintln!("{test}: skipped, FFmpeg isn't on PATH to make its videos");
        return false;
    };
    let list = String::from_utf8_lossy(&listed.stdout);
    for name in ["libx264", "aac"] {
        if !list
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(name))
        {
            eprintln!("{test}: skipped, FFmpeg has no {name} encoder to make its videos");
            return false;
        }
    }
    true
}

/// Where the clips are kept: the build folder's own temp folder.
fn kept() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("kiln-clips-1")
}

/// The clip as an MP4, made the first time it is asked for (call
/// [`can_make_clips`] first). Tests that run at once may each make it; each
/// moves its copy into place, and a copy that can't be moved (another test
/// is reading the clip) is removed.
pub fn clip(kind: Clip) -> PathBuf {
    let dir = kept();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}.mp4", kind.name()));
    if path.is_file() {
        return path;
    }
    let part = dir.join(format!(
        "{}-{}-{:?}.part.mp4",
        kind.name(),
        std::process::id(),
        std::thread::current().id()
    ));
    let made = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(kind.args())
        .arg(&part)
        .output()
        .expect("ffmpeg starts");
    assert!(
        made.status.success(),
        "ffmpeg failed making the {} clip: {}",
        kind.name(),
        String::from_utf8_lossy(&made.stderr)
    );
    if std::fs::rename(&part, &path).is_err() {
        // Another test's copy is in place and open.
        let _ = std::fs::remove_file(&part);
        assert!(path.is_file(), "the {} clip wasn't made", kind.name());
    }
    path
}
