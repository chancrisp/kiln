use super::*;
use crate::held::{stale_runs, Holding, LOCK_NAME, TEMPS_NAME};
use std::time::Duration;

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// A run folder under `root` as a run that ended without a word left it:
/// its lock closed, `files` in it, and `sources` as its list.
fn left_run(root: &Path, name: &str, files: &[&str], sources: Option<&str>) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    write(&dir.join(LOCK_NAME), "");
    for f in files {
        write(&dir.join(f), f);
    }
    if let Some(text) = sources {
        write(&dir.join(SOURCES_NAME), text);
    }
    dir
}

/// A scratch PC: Kiln's local folder, a Documents folder (never the real
/// one), and the run folders under `kiln\held`.
struct Pc {
    kiln: tempfile::TempDir,
    docs: tempfile::TempDir,
}

impl Pc {
    fn new() -> Pc {
        Pc {
            kiln: tempfile::tempdir().unwrap(),
            docs: tempfile::tempdir().unwrap(),
        }
    }
    fn places(&self) -> Places {
        Places::new(self.kiln.path())
    }
    fn held(&self) -> PathBuf {
        self.places().local_held().to_path_buf()
    }
    fn fallback(&self) -> PathBuf {
        kiln_results_in(self.docs.path())
    }
    /// Find, lock and deliver what is in the local folder, as a start does.
    fn start(&self) -> Report {
        let runs = stale_runs(&self.held());
        deliver_left(
            runs,
            &self.places(),
            Some(&self.fallback()),
            &CancelToken::new(),
        )
    }
}

fn line(name: &str, dir: &Path) -> String {
    format!("{name}\t{}\n", dir.display())
}

#[test]
fn a_crashed_runs_results_are_delivered_next_to_their_originals() {
    let pc = Pc::new();
    let a_dir = tempfile::tempdir().unwrap();
    let b_dir = tempfile::tempdir().unwrap();
    let sources = line("a.jpg", a_dir.path()) + &line("b.png", b_dir.path());
    let run = left_run(&pc.held(), "1-1", &["a.jpg", "b.png"], Some(&sources));
    let report = pc.start();
    assert_eq!(
        fs::read_to_string(a_dir.path().join("a.jpg")).unwrap(),
        "a.jpg"
    );
    assert_eq!(
        fs::read_to_string(b_dir.path().join("b.png")).unwrap(),
        "b.png"
    );
    assert!(!run.exists(), "the run folder goes once it is empty");
    assert_eq!((report.beside, report.fallback, report.kept), (2, 0, 0));
    assert_eq!(report.folders.len(), 2);
    assert!(
        !pc.fallback().exists(),
        "the fallback is made only when needed"
    );
}

#[test]
fn a_run_from_before_sources_were_noted_goes_to_kiln_results() {
    let pc = Pc::new();
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], None);
    let report = pc.start();
    assert_eq!(
        fs::read_to_string(pc.fallback().join("a.jpg")).unwrap(),
        "a.jpg"
    );
    assert!(!run.exists());
    assert_eq!((report.beside, report.fallback, report.kept), (0, 1, 0));
    assert_eq!(report.folders, [pc.fallback()]);
}

#[test]
fn a_missing_documents_folder_is_never_made_and_the_result_is_kept() {
    let pc = Pc::new();
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], None);
    let docs = pc.docs.path().join("not there");
    let report = deliver_left(
        stale_runs(&pc.held()),
        &pc.places(),
        Some(&kiln_results_in(&docs)),
        &CancelToken::new(),
    );
    assert!(!docs.exists(), "Documents itself is never made");
    assert_eq!(report.kept, 1);
    assert_eq!(fs::read_to_string(run.join("a.jpg")).unwrap(), "a.jpg");
}

#[test]
fn a_gone_origin_folder_sends_the_result_to_kiln_results_and_is_never_made() {
    let pc = Pc::new();
    let gone = pc.docs.path().join("Camera").join("DCIM");
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], Some(&line("a.jpg", &gone)));
    let report = pc.start();
    assert!(!gone.exists(), "a folder that went is never made again");
    assert!(!gone.parent().unwrap().exists());
    assert_eq!(report.fallback, 1);
    assert!(pc.fallback().join("a.jpg").is_file());
    assert!(!run.exists());
}

#[cfg(windows)]
#[test]
fn a_read_only_origin_sends_the_result_to_kiln_results() {
    let pc = Pc::new();
    let ro = tempfile::tempdir().unwrap();
    let deny = |on: bool| {
        let mut c = std::process::Command::new("icacls");
        c.arg(ro.path());
        if on {
            c.args(["/deny", "*S-1-1-0:(WD,AD)"]);
        } else {
            c.args(["/remove:d", "*S-1-1-0"]);
        }
        assert!(c.output().unwrap().status.success());
    };
    deny(true);
    let run = left_run(
        &pc.held(),
        "1-1",
        &["a.jpg"],
        Some(&line("a.jpg", ro.path())),
    );
    let report = pc.start();
    deny(false);
    assert_eq!(report.fallback, 1, "{report:?}");
    assert!(names(ro.path()).is_empty());
    assert!(pc.fallback().join("a.jpg").is_file());
    assert!(!run.exists());
}

#[test]
fn a_name_clash_takes_a_new_name_and_leaves_the_file_there_untouched() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let original = origin.path().join("photo.jpg");
    write(&original, "the original");
    let before = fs::metadata(&original).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    left_run(
        &pc.held(),
        "1-1",
        &["photo.jpg"],
        Some(&line("photo.jpg", origin.path())),
    );
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert_eq!(fs::read_to_string(&original).unwrap(), "the original");
    assert_eq!(fs::metadata(&original).unwrap().modified().unwrap(), before);
    assert_eq!(
        fs::read_to_string(origin.path().join("photo (1).jpg")).unwrap(),
        "photo.jpg"
    );
}

#[test]
fn two_runs_with_the_same_result_name_both_arrive_under_free_names() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let l = line("a.jpg", origin.path());
    left_run(&pc.held(), "1-1", &["a.jpg"], Some(&l));
    left_run(&pc.held(), "2-2", &["a.jpg"], Some(&l));
    let report = pc.start();
    assert_eq!(report.beside, 2);
    assert_eq!(names(origin.path()), ["a (1).jpg", "a.jpg"]);
    assert_eq!(report.folders, [origin.path().to_path_buf()]);
}

#[test]
fn a_copy_to_another_volume_is_checked_before_the_held_copy_goes() {
    test_hooks::FORCE_COPY.with(|c| c.set(true));
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let run = left_run(
        &pc.held(),
        "1-1",
        &["a.jpg"],
        Some(&line("a.jpg", origin.path())),
    );
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert_eq!(
        fs::read_to_string(origin.path().join("a.jpg")).unwrap(),
        "a.jpg"
    );
    assert_eq!(names(origin.path()), ["a.jpg"], "no temp copy is left");
    assert!(!run.exists());

    // A copy that comes out wrong: nothing Kiln made is left at either
    // place, and the held copy is whole for the next start.
    test_hooks::FAIL_CHECK.with(|c| c.set(true));
    let other = tempfile::tempdir().unwrap();
    let bytes = "a held result long enough to cut in half";
    let run = pc.held().join("2-2");
    fs::create_dir_all(&run).unwrap();
    write(&run.join(LOCK_NAME), "");
    write(&run.join("b.jpg"), bytes);
    write(&run.join(SOURCES_NAME), &line("b.jpg", other.path()));
    let report = pc.start();
    test_hooks::FAIL_CHECK.with(|c| c.set(false));
    assert_eq!(report.kept, 1, "{report:?}");
    assert!(names(other.path()).is_empty());
    assert!(!pc.fallback().exists() || names(&pc.fallback()).is_empty());
    assert_eq!(fs::read_to_string(run.join("b.jpg")).unwrap(), bytes);
    // And the next start delivers it.
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert_eq!(
        fs::read_to_string(other.path().join("b.jpg")).unwrap(),
        bytes
    );
    assert!(!run.exists());
}

#[test]
fn a_result_that_cant_be_delivered_stays_held_and_the_next_start_delivers_it() {
    let pc = Pc::new();
    let gone = pc.docs.path().join("gone");
    // Documents\Kiln results is a file: no fallback.
    write(&pc.fallback(), "someone's file");
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], Some(&line("a.jpg", &gone)));
    let report = pc.start();
    assert_eq!((report.beside, report.fallback, report.kept), (0, 0, 1));
    assert_eq!(fs::read_to_string(run.join("a.jpg")).unwrap(), "a.jpg");
    assert_eq!(
        fs::read_to_string(pc.fallback()).unwrap(),
        "someone's file",
        "a file is never moved or removed to make room"
    );
    // Let go: another look can take it.
    assert!(crate::held::probe_lock(&run.join(LOCK_NAME)).is_ok());
    fs::remove_file(pc.fallback()).unwrap();
    let report = pc.start();
    assert_eq!(report.fallback, 1);
    assert!(pc.fallback().join("a.jpg").is_file());
    assert!(!run.exists());
}

#[test]
fn an_older_builds_discard_note_is_ignored_and_its_results_delivered() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let run = left_run(
        &pc.held(),
        "1-1",
        &["a.jpg"],
        Some(&line("a.jpg", origin.path())),
    );
    // As an older version's quit wrote it.
    write(&run.join(crate::held::DISCARD_NAME), "1-1");
    let report = pc.start();
    assert_eq!((report.beside, report.fallback, report.kept), (1, 0, 0));
    assert_eq!(
        fs::read_to_string(origin.path().join("a.jpg")).unwrap(),
        "a.jpg"
    );
    assert!(!run.exists(), "the note goes with the emptied folder");
    assert!(!pc.fallback().exists());
}

#[test]
fn a_live_runs_results_are_never_delivered() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let live = Holding::open(&pc.held()).unwrap();
    let a = live.dir().join("a.jpg");
    write(&a, "a");
    live.record(&a, origin.path()).unwrap();
    let report = pc.start();
    assert!(report.is_empty());
    assert!(a.is_file());
    assert!(names(origin.path()).is_empty());
    drop(live);
}

#[test]
fn a_delivery_cut_short_by_a_crash_leaves_the_held_result_and_the_next_start_clears_its_temp() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let run = left_run(
        &pc.held(),
        "1-1",
        &["a.jpg"],
        Some(&line("a.jpg", origin.path())),
    );
    let cut = origin.path().join("a.jpg.1-1.kiln-export.tmp");
    write(&cut, "half");
    write(&run.join(TEMPS_NAME), &format!("+\t{}\n", cut.display()));
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert!(!cut.exists(), "the copy cut short goes");
    assert_eq!(names(origin.path()), ["a.jpg"]);
}

#[test]
fn a_held_copy_already_delivered_is_removed_not_delivered_twice() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let to = origin.path().join("a.jpg");
    write(&to, "a.jpg");
    let sources = line("a.jpg", origin.path()) + &format!("a.jpg\t\t{}\n", to.display());
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], Some(&sources));
    let report = pc.start();
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(names(origin.path()), ["a.jpg"]);
    assert!(!run.exists());

    // The delivered copy was removed by the person since: delivered again.
    fs::remove_file(&to).unwrap();
    let run = left_run(&pc.held(), "2-2", &["a.jpg"], Some(&sources));
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert!(to.is_file());
    assert!(!run.exists());
}

#[test]
fn a_new_result_of_a_delivered_name_is_delivered() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let to = origin.path().join("a.jpg");
    write(&to, "a.jpg");
    // Delivered, then a redo made a new a.jpg and noted its folder.
    let sources = line("a.jpg", origin.path())
        + &format!("a.jpg\t\t{}\n", to.display())
        + &line("a.jpg", origin.path());
    left_run(&pc.held(), "1-1", &["a.jpg"], Some(&sources));
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert_eq!(names(origin.path()), ["a (1).jpg", "a.jpg"]);
}

#[test]
fn a_cancelled_delivery_keeps_everything_for_the_next_start() {
    test_hooks::FORCE_COPY.with(|c| c.set(true));
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let bytes: Vec<u8> = (0..48u32 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let held = src.path().join("big.bin");
    fs::write(&held, &bytes).unwrap();
    let cancel = CancelToken::new();
    let mut progress = |done: u64, size: u64| {
        if done > 0 && done < size {
            cancel.cancel();
        }
    };
    let fallback = kiln_results_in(docs.path());
    let d = deliver_with(
        &held,
        Some(out.path()),
        Some(&fallback),
        &mut progress,
        &cancel,
    );
    assert_eq!(d, Delivery::Kept);
    assert_eq!(fs::read(&held).unwrap(), bytes);
    assert!(names(out.path()).is_empty());
    assert!(!fallback.exists(), "a cancel tries nothing more");
}

#[test]
fn a_destination_with_no_room_falls_back_and_leaves_no_partial() {
    test_hooks::FORCE_COPY.with(|c| c.set(true));
    test_hooks::FAIL_COPY.with(|c| c.set(true));
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let held = src.path().join("a.jpg");
    write(&held, "data");
    let fallback = kiln_results_in(docs.path());
    // Every copy fails here: kept, nothing left anywhere.
    let d = deliver_with(
        &held,
        Some(out.path()),
        Some(&fallback),
        &mut |_, _| {},
        &CancelToken::new(),
    );
    assert_eq!(d, Delivery::Kept);
    assert!(names(out.path()).is_empty());
    assert!(names(&fallback).is_empty());
    assert_eq!(fs::read_to_string(&held).unwrap(), "data");
    // Only the destination fails (a rename to the fallback on this
    // volume): delivered there.
    test_hooks::FAIL_COPY.with(|c| c.set(false));
    test_hooks::FORCE_COPY.with(|c| c.set(false));
    let missing = out.path().join("not there");
    let d = deliver_with(
        &held,
        Some(&missing),
        Some(&fallback),
        &mut |_, _| {},
        &CancelToken::new(),
    );
    assert_eq!(d, Delivery::Fallback(fallback.join("a.jpg")));
    assert!(!held.exists());
}

#[cfg(windows)]
#[test]
fn a_long_path_destination_is_delivered() {
    let pc = Pc::new();
    let base = tempfile::tempdir().unwrap();
    let mut deep = base.path().to_path_buf();
    while deep.as_os_str().len() < 300 {
        deep.push("a folder with a long name");
    }
    fs::create_dir_all(&deep).unwrap();
    left_run(&pc.held(), "1-1", &["a.jpg"], Some(&line("a.jpg", &deep)));
    let report = pc.start();
    assert_eq!(report.beside, 1, "{report:?}");
    assert!(deep.join("a.jpg").is_file());
}

#[test]
fn a_run_holding_more_than_plain_files_is_left_whole_and_not_counted() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let run = left_run(
        &pc.held(),
        "1-1",
        &["a.jpg"],
        Some(&line("a.jpg", origin.path())),
    );
    fs::create_dir(run.join("inner")).unwrap();
    let report = pc.start();
    assert!(report.is_empty());
    assert!(run.join("a.jpg").is_file());
    assert!(names(origin.path()).is_empty());
}

#[test]
fn an_empty_placeholder_is_never_delivered_and_goes_with_its_run() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let sources = line("a.jpg", origin.path()) + &line("b.jpg", origin.path());
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], Some(&sources));
    write(&run.join("b.jpg"), "");
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert_eq!(names(origin.path()), ["a.jpg"]);
    assert!(!run.exists());
}

#[test]
fn a_destination_inside_a_holding_place_goes_to_kiln_results() {
    let pc = Pc::new();
    let inside = pc.held().join("9-9");
    fs::create_dir_all(&inside).unwrap();
    left_run(&pc.held(), "1-1", &["a.jpg"], Some(&line("a.jpg", &inside)));
    let report = pc.start();
    assert_eq!(report.fallback, 1);
    assert!(names(&inside).is_empty());
}

#[test]
fn a_damaged_sources_line_gives_kiln_results_never_a_guess() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let o = origin.path().display();
    let run = left_run(&pc.held(), "1-1", &["a.jpg", "b.jpg", "c.jpg"], None);
    let mut bytes = "a.jpg\trelative\\folder\nb.jpg\t".to_string().into_bytes();
    bytes.extend_from_slice(&[0xff, 0xfe]);
    bytes.extend_from_slice(format!("{o}\nc.jpg\t{o}").as_bytes());
    fs::write(run.join(SOURCES_NAME), bytes).unwrap();
    let report = pc.start();
    assert_eq!((report.beside, report.fallback), (0, 3), "{report:?}");
    assert!(names(origin.path()).is_empty());
}

#[test]
fn this_versions_line_names_where_the_result_was_to_go() {
    let pc = Pc::new();
    let chosen = tempfile::tempdir().unwrap();
    let h = Holding::open(&pc.held()).unwrap();
    let a = h.dir().join("a.jpg");
    write(&a, "a");
    // The folder chosen in Settings, not the original's.
    h.record(&a, chosen.path()).unwrap();
    drop(h);
    let report = pc.start();
    assert_eq!(report.beside, 1);
    assert!(chosen.path().join("a.jpg").is_file());
}

#[test]
fn a_quit_removes_only_run_folders_with_nothing_in_them() {
    let pc = Pc::new();
    let full = Holding::open(&pc.held()).unwrap();
    let empty = Holding::open(&pc.held()).unwrap();
    let (full_dir, empty_dir) = (full.dir().to_path_buf(), empty.dir().to_path_buf());
    write(&full_dir.join("a.jpg"), "a");
    // Bookkeeping only: its list and an empty placeholder.
    write(&empty_dir.join(SOURCES_NAME), "");
    write(&empty_dir.join("b.jpg"), "");
    assert!(!full.close_if_empty());
    assert!(empty.close_if_empty());
    assert!(!empty_dir.exists());
    assert!(full_dir.join("a.jpg").is_file());
    // Let go: the next start can take it.
    assert!(crate::held::probe_lock(&full_dir.join(LOCK_NAME)).is_ok());
    let report = pc.start();
    assert_eq!(report.fallback, 1);
}

#[test]
fn kiln_results_lives_in_documents() {
    let docs = Path::new(r"C:\Users\someone\Documents");
    assert_eq!(
        kiln_results_in(docs),
        docs.join("Kiln results"),
        "the folder the words name"
    );
}

#[test]
fn a_result_moved_on_one_volume_is_checked_whole() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let held = src.path().join("a.jpg");
    write(&held, "data");
    let to = deliver_into_with(&held, out.path(), &mut |_, _| {}, &CancelToken::new()).unwrap();
    assert_eq!(to, out.path().join("a.jpg"));
    assert_eq!(fs::read_to_string(&to).unwrap(), "data");
    assert!(!held.exists());
    assert!(deliver_into_with(&held, out.path(), &mut |_, _| {}, &CancelToken::new()).is_err());
}

#[test]
fn a_different_result_of_a_delivered_name_and_length_is_delivered_never_removed() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    // Result A was delivered and marked; then result B took the free name
    // in the holding folder, and the run ended before B's line was written.
    let to = origin.path().join("photo.bmp");
    let a: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let b: Vec<u8> = a.iter().map(|x| x ^ 0xff).collect();
    assert_eq!(a.len(), b.len());
    fs::write(&to, &a).unwrap();
    let sources = line("photo.bmp", origin.path()) + &format!("photo.bmp\t\t{}\n", to.display());
    let run = left_run(&pc.held(), "1-1", &[], Some(&sources));
    fs::write(run.join("photo.bmp"), &b).unwrap();
    let report = pc.start();
    assert_eq!(fs::read(&to).unwrap(), a, "A is untouched");
    let others: Vec<String> = names(origin.path())
        .into_iter()
        .filter(|n| n != "photo.bmp")
        .collect();
    let b_went = others
        .iter()
        .any(|n| fs::read(origin.path().join(n)).unwrap() == b)
        || (pc.fallback().exists()
            && names(&pc.fallback())
                .iter()
                .any(|n| fs::read(pc.fallback().join(n)).unwrap() == b))
        || fs::read(run.join("photo.bmp")).is_ok_and(|got| got == b);
    assert!(b_went, "B was removed without being delivered: {report:?}");
    assert_eq!(report.beside, 1, "{report:?}");
    assert_eq!(fs::read(origin.path().join("photo (1).bmp")).unwrap(), b);
}

#[test]
fn a_rename_whose_length_check_fails_is_reported_where_it_went() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let held = src.path().join("a.jpg");
    write(&held, "a held result");
    // Something changes the file as the rename lands: it is still the one
    // file, now at its destination.
    test_hooks::FAIL_CHECK.with(|c| c.set(true));
    let d = deliver_with(
        &held,
        Some(out.path()),
        Some(&kiln_results_in(docs.path())),
        &mut |_, _| {},
        &CancelToken::new(),
    );
    test_hooks::FAIL_CHECK.with(|c| c.set(false));
    assert_eq!(d, Delivery::Beside(out.path().join("a.jpg")));
    assert!(!held.exists());
    assert!(out.path().join("a.jpg").is_file());
}

#[test]
fn a_mark_naming_the_held_file_itself_never_removes_it() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    // A hand-edited or damaged list whose mark names the held file.
    let run = pc.held().join("1-1");
    let held = run.join("a.jpg");
    let sources = line("a.jpg", origin.path()) + &format!("a.jpg\t\t{}\n", held.display());
    left_run(&pc.held(), "1-1", &["a.jpg"], Some(&sources));
    let report = pc.start();
    assert_eq!(report.beside, 1, "{report:?}");
    assert_eq!(
        fs::read_to_string(origin.path().join("a.jpg")).unwrap(),
        "a.jpg"
    );
    assert!(!run.exists());
}

#[test]
fn a_mark_naming_a_link_to_the_held_file_never_removes_it() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], None);
    // The same file under a second name, outside any holding place.
    let link = elsewhere.path().join("a.jpg");
    fs::hard_link(run.join("a.jpg"), &link).unwrap();
    let sources = line("a.jpg", origin.path()) + &format!("a.jpg\t\t{}\n", link.display());
    write(&run.join(SOURCES_NAME), &sources);
    let report = pc.start();
    assert_eq!(report.beside, 1, "{report:?}");
    assert_eq!(
        fs::read_to_string(origin.path().join("a.jpg")).unwrap(),
        "a.jpg"
    );
    assert!(!run.exists());
}

#[test]
fn a_mark_naming_a_file_in_a_holding_place_never_removes_the_held_one() {
    let pc = Pc::new();
    let origin = tempfile::tempdir().unwrap();
    // The same bytes, held by another run: no delivered copy.
    let other = pc.held().join("0-0");
    fs::create_dir_all(&other).unwrap();
    let twin = other.join("a.jpg");
    write(&twin, "a.jpg");
    let sources = line("a.jpg", origin.path()) + &format!("a.jpg\t\t{}\n", twin.display());
    let run = left_run(&pc.held(), "1-1", &["a.jpg"], Some(&sources));
    let report = pc.start();
    assert_eq!(report.beside, 1, "{report:?}");
    assert_eq!(
        fs::read_to_string(origin.path().join("a.jpg")).unwrap(),
        "a.jpg"
    );
    assert!(!run.exists());
    assert!(twin.is_file(), "the other run's file is untouched");
}

/// The DACL a file made in `dir` the plain way gets.
#[cfg(windows)]
fn fresh_dacl(dir: &Path) -> String {
    let fresh = dir.join("made here.txt");
    write(&fresh, "fresh");
    let sddl = crate::held::win::dacl_sddl(&fresh).unwrap();
    fs::remove_file(&fresh).unwrap();
    sddl
}

#[cfg(windows)]
#[test]
fn a_result_renamed_out_of_a_private_folder_takes_its_new_folders_permissions() {
    delivered_from_a_private_folder_takes_its_new_folders_permissions();
}

#[cfg(windows)]
#[test]
fn a_result_copied_out_of_a_private_folder_takes_its_new_folders_permissions() {
    test_hooks::FORCE_COPY.with(|c| c.set(true));
    delivered_from_a_private_folder_takes_its_new_folders_permissions();
    test_hooks::FORCE_COPY.with(|c| c.set(false));
}

/// A result delivered out of a folder only the user can open, into one
/// others can read: it has the permissions a file made there has.
#[cfg(windows)]
fn delivered_from_a_private_folder_takes_its_new_folders_permissions() {
    use crate::held::win::{create_private_dir, dacl_sddl, user_sid};
    let sid = user_sid().expect("this process's SID");
    let scratch = tempfile::tempdir().unwrap();
    // The holding folder: the user, SYSTEM and Administrators only.
    let private = scratch.path().join(&sid);
    create_private_dir(&private, &sid).unwrap();
    let held = private.join("a.jpg");
    write(&held, "a held result");
    // A folder others can read: Users may read what is made in it.
    let shared = scratch.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let grant = std::process::Command::new("icacls")
        .arg(&shared)
        .args(["/grant", "*S-1-5-32-545:(OI)(CI)(RX)"])
        .output()
        .unwrap();
    assert!(grant.status.success());
    let wanted = fresh_dacl(&shared);
    assert!(wanted.contains(";;;BU)"), "{wanted}");
    let d = deliver_with(
        &held,
        Some(&shared),
        None,
        &mut |_, _| {},
        &CancelToken::new(),
    );
    let to = shared.join("a.jpg");
    assert_eq!(d, Delivery::Beside(to.clone()));
    assert!(!held.exists());
    // Only what its new folder hands down: nothing protected, nothing of
    // its own, and the same entries as a file made there.
    let sddl = dacl_sddl(&to).unwrap();
    assert!(!sddl.starts_with("D:P"), "{sddl}");
    assert_eq!(
        sddl.matches("(A;").count(),
        sddl.matches(";ID;").count(),
        "{sddl}"
    );
    assert_eq!(sddl, wanted);
    assert_eq!(fs::read_to_string(&to).unwrap(), "a held result");
}

#[cfg(windows)]
#[test]
fn letting_a_file_take_its_folders_permissions_fails_cleanly_for_a_file_not_there() {
    let scratch = tempfile::tempdir().unwrap();
    assert!(crate::held::win::inherit_dacl(&scratch.path().join("not there")).is_err());
}

/// Open `path` so no one else can read, write, rename or remove it, as a
/// program that has it open does.
#[cfg(windows)]
fn locked(path: &Path) -> File {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[cfg(windows)]
#[test]
fn a_result_open_in_another_program_stays_held_and_never_goes_to_kiln_results() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let held = src.path().join("a.jpg");
    write(&held, "a held result");
    let fallback = kiln_results_in(docs.path());
    let lock = locked(&held);
    let d = deliver_with(
        &held,
        Some(out.path()),
        Some(&fallback),
        &mut |_, _| {},
        &CancelToken::new(),
    );
    drop(lock);
    // Its folder can take files: it is kept for the next try, not saved
    // where it wasn't meant to go.
    assert_eq!(d, Delivery::Kept);
    assert!(names(out.path()).is_empty());
    assert!(!fallback.exists());
    assert_eq!(fs::read_to_string(&held).unwrap(), "a held result");
}

#[cfg(windows)]
#[test]
fn a_copy_to_another_volume_of_a_result_open_elsewhere_stays_held() {
    test_hooks::FORCE_COPY.with(|c| c.set(true));
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let held = src.path().join("a.jpg");
    write(&held, "a held result");
    let fallback = kiln_results_in(docs.path());
    let lock = locked(&held);
    let d = deliver_with(
        &held,
        Some(out.path()),
        Some(&fallback),
        &mut |_, _| {},
        &CancelToken::new(),
    );
    drop(lock);
    test_hooks::FORCE_COPY.with(|c| c.set(false));
    assert_eq!(d, Delivery::Kept);
    assert!(names(out.path()).is_empty());
    assert!(!fallback.exists());
    assert_eq!(fs::read_to_string(&held).unwrap(), "a held result");
}

#[cfg(windows)]
#[test]
fn a_result_let_go_of_a_moment_later_is_delivered_where_it_was_meant_to_go() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let docs = tempfile::tempdir().unwrap();
    let held = src.path().join("a.jpg");
    write(&held, "a held result");
    let lock = locked(&held);
    let letting_go = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(lock);
    });
    let d = deliver_with(
        &held,
        Some(out.path()),
        Some(&kiln_results_in(docs.path())),
        &mut |_, _| {},
        &CancelToken::new(),
    );
    letting_go.join().unwrap();
    assert_eq!(d, Delivery::Beside(out.path().join("a.jpg")));
    assert!(!held.exists());
}

#[cfg(windows)]
#[test]
fn a_cancel_while_a_result_is_open_elsewhere_keeps_it_at_once() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let held = src.path().join("a.jpg");
    write(&held, "a held result");
    let lock = locked(&held);
    let cancel = CancelToken::new();
    let stop = cancel.clone();
    let stopping = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        stop.cancel();
    });
    let started = std::time::Instant::now();
    let d = deliver_with(&held, Some(out.path()), None, &mut |_, _| {}, &cancel);
    stopping.join().unwrap();
    drop(lock);
    assert_eq!(d, Delivery::Kept);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(fs::read_to_string(&held).unwrap(), "a held result");
}

#[test]
fn only_a_lasting_failure_says_the_folder_cant_take_files() {
    let os = |code: i32| Error::io(Path::new("x"), io::Error::from_raw_os_error(code));
    #[cfg(windows)]
    {
        // Access denied, not found, path not found, write-protected, no
        // disk, disk full, the share isn't there.
        for code in [5, 2, 3, 19, 21, 112, 39, 53, 67] {
            assert!(lasting(&os(code)), "{code}");
        }
        // Sharing and lock violations, a network that timed out once.
        for code in [32, 33, 121, 64] {
            assert!(!lasting(&os(code)), "{code}");
        }
    }
    let kind = |k: io::ErrorKind| Error::io(Path::new("x"), io::Error::from(k));
    assert!(lasting(&kind(io::ErrorKind::PermissionDenied)));
    assert!(lasting(&kind(io::ErrorKind::NotFound)));
    assert!(lasting(&kind(io::ErrorKind::ReadOnlyFilesystem)));
    assert!(lasting(&kind(io::ErrorKind::StorageFull)));
    assert!(!lasting(&kind(io::ErrorKind::TimedOut)));
    assert!(!lasting(&Error::io(
        Path::new("x"),
        io::Error::other("odd")
    )));
    // Every free name taken is no sign the folder can't take files.
    assert!(!lasting(&Error::NoFreeName {
        base: PathBuf::from("x"),
        attempts: 1
    }));
}
