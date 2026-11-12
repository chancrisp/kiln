//! A held result moved to a real second volume, `E:`, when this PC has one:
//! the move is a `CopyFileExW` copy with progress, and a cancel part-way
//! leaves the held result whole and nothing at the destination.
//!
//! It makes only its own folder, `E:\kiln-phase2-test-<pid>\`, and removes
//! only what is in it, then the folder. It never makes `E:\.kiln-held`.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};

use kiln_core::{
    export_into_with, CancelToken, Error, Holding, SystemVolumes, Volumes, HELD_DIR_NAME,
};

/// This test's own folder on `E:`, emptied and removed when dropped: its
/// plain files one by one, then the folder, with no recursive remove.
struct OnE(PathBuf);

impl OnE {
    fn make() -> Option<OnE> {
        let e = Path::new(r"E:\");
        if !e.is_dir() {
            println!("skipped: no E: volume on this PC");
            return None;
        }
        let temp = std::env::temp_dir();
        let vols = SystemVolumes;
        let (Some(here), Some(there)) = (vols.volume_root(&temp), vols.volume_root(e)) else {
            println!("skipped: the volumes of TEMP and E: can't be told");
            return None;
        };
        if here == there {
            println!("skipped: TEMP is on E:, so a move there isn't across volumes");
            return None;
        }
        let dir = e.join(format!("kiln-phase2-test-{}", std::process::id()));
        if let Err(err) = fs::create_dir(&dir) {
            println!("skipped: can't make {}: {err}", dir.display());
            return None;
        }
        Some(OnE(dir))
    }

    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for OnE {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file()) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        let _ = fs::remove_dir(&self.0);
    }
}

#[test]
fn a_move_to_another_volume_copies_with_progress_and_cancels_cleanly() {
    let Some(on_e) = OnE::make() else {
        return;
    };
    let had_held_root = Path::new(r"E:\").join(HELD_DIR_NAME).exists();
    let root = tempfile::tempdir().unwrap();
    let holding = Holding::open(root.path()).unwrap();

    // 32 MiB, so the copy reports progress part-way.
    let bytes: Vec<u8> = (0..32u32 * 1024 * 1024).map(|i| (i % 253) as u8).collect();
    let total = bytes.len() as u64;
    let held = holding.dir().join("photo.jpg");
    fs::write(&held, &bytes).unwrap();
    // A file of that name is there already: the result takes the next name.
    fs::write(on_e.0.join("photo.jpg"), "theirs").unwrap();

    // Cancelled part-way: the held result whole, no temp, no new file.
    let cancel = CancelToken::new();
    let mut midway = false;
    let mut progress = |done: u64, size: u64| {
        if done > 0 && done < size {
            midway = true;
            cancel.cancel();
        }
    };
    let err = export_into_with(&held, &on_e.0, &mut progress, &cancel).unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err}");
    assert!(midway, "the copy never reported progress part-way");
    assert_eq!(fs::read(&held).unwrap(), bytes);
    assert_eq!(on_e.files(), ["photo.jpg"]);
    assert_eq!(
        fs::read_to_string(on_e.0.join("photo.jpg")).unwrap(),
        "theirs"
    );

    // Let run: progress to the end, the copy in place, the held file gone.
    let mut last = (0, 0);
    let got = export_into_with(
        &held,
        &on_e.0,
        &mut |d, s| last = (d, s),
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(got, on_e.0.join("photo (1).jpg"));
    assert_eq!(last, (total, total));
    assert_eq!(fs::read(&got).unwrap(), bytes);
    assert!(!held.exists());
    assert_eq!(on_e.files(), ["photo (1).jpg", "photo.jpg"]);
    assert_eq!(
        fs::read_to_string(on_e.0.join("photo.jpg")).unwrap(),
        "theirs"
    );

    // No holding root was made on E: by any of it.
    assert_eq!(
        Path::new(r"E:\").join(HELD_DIR_NAME).exists(),
        had_held_root
    );
    assert!(holding.close_if_empty());
}
