# Suggestions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kiln looks inside added files and, when one choice clearly helps, marks the convert target or tool most people want, with a one-line reason and, where possible, an estimate.

**Architecture:** Two new kiln-core units. `facts.rs` (`inspect`) reads a file once and does all the input and output. `suggest.rs` (`suggest`) is a pure function from facts, offered choices and habits to at most one convert and one tool suggestion. `Kiln::offered` lists only targets whose engines are installed. The CLI calls them directly. The app runs `inspect` on one low-priority thread and shows the result in the row, the wheel and the footer.

**Tech Stack:** Rust 2021 workspace (kiln-core, kiln-cli, kiln-app), with `image` 0.25, `lopdf`, `pdf_extract`, `serde`/`toml`, `clap`, FFmpeg/FFprobe through `process::run`, and Win32/Direct2D through the `windows` crate. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-19-suggestions-design.md`

## Global Constraints

- No new crates. Everything here uses dependencies the workspace already has.
- Reason strings, row text and footer text are copied verbatim from the spec. The only additions are the numberless forms this plan names (Task 4).
- Sizes use decimal units (1000), like `model::human_size`. Numbers in reasons are rounded to two significant figures.
- `inspect` never fails or panics. Images over 50,000,000 pixels are not decoded. PDFs over 200,000,000 bytes skip the text check. Every FFmpeg run and PDF read stops after 2 s or on `cancel`.
- Habits: a habit wins at 3 or more picks when it is strictly the most picked target for that source. At most 64 source types are kept.
- `WM_KILN_FACTS = WM_APP + 7`.
- settings.toml gains `suggestions = true` and a `[habits]` table.
- Motion: the row line and the wheel dot fade in over 160 ms, ease-out. Nothing moves or pulses.
- Celadon (the accent) is used only for the dot, among new elements.
- `kiln suggest` exits with 0, or with 2 when a file doesn't exist. It ignores habits.
- Git: commit as the owner's own identity (`chancrisp`), with no Co-Authored-By or any other AI attribution. Before each commit, check the new commit with `git log -1 --format='%an <%ae> | %cn <%ce>%n%B'` and stop if "Claude" or "anthropic" appears. Never push. Never `rm` with a variable or wildcard.
- Gates for every task: `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets -- -D warnings`.

## Decisions this plan makes where the spec is open

1. `Facts` gains `estimates: BTreeMap<String, u64>` (likely result bytes per target extension) and `compressed: Option<u64>`. `suggest` is pure and has no preview or registry, so `inspect` computes the numbers.
2. For several files, a shared conversion uses the reason's numberless form, and its estimate is the sum when every file has one.
3. A tie for most picked target means no habit applies, and the rules decide.
4. A tool suggestion for a set needs every file to qualify.
5. The row shows the convert suggestion, or the tool suggestion when there is no convert suggestion.
6. Fixtures: `rgba_half_transparent.png` and `letterbox.mp4` stand in for the spec's `graphic.png` and `bars.mp4`. Only `photo.png` and `scan.pdf` are new. The 2× estimate check runs on a 1024 × 768 picture the test draws at run time, so the fixtures stay a few KB.
7. A tool is available unless an input is audio or video and FFmpeg is missing.
8. The dot is centred 6 DIP above the top edge of its icon. On the outer ring, where segments have no icon, it sits 6 DIP above the code's text.
9. `inspect` returns `Option<Facts>`. It is `None` only for an extension Kiln doesn't know or a file it can't stat, since `Facts` needs a kind.

## Review Focus

1. **Files whose contents don't match their extension, or are cut short** (`not_really.png`, `truncated.png`, `corrupt.wav`). Expect type and size facts only, no panic and no suggestion that depends on the picture. Tested in Tasks 2 and 3.
2. **An engine set in Settings that no longer exists.** Expect no target or tool that needs it to be offered, and `inspect` to return no media facts for that file instead of hanging. Tested in Tasks 1 and 3.
3. **A row removed, or the list cleared, while its inspect is queued or running.** Expect the result to be dropped, and no other row to pick it up. Tested in Task 7.
4. **A wheel whose suggestion is in neither ring** (a target not shown for a mixed set). Expect no dot, and the keyboard to start on `Inner(0)` as it does today. Tested in Task 8.
5. **A hand-edited settings.toml with a `[habits]` entry that isn't a table of counts.** Expect the same handling as any other bad value in settings.toml, and `suggestions` to default to `true` when missing. Tested in Task 6.

---

### Task 1: Engine availability and `Kiln::offered`

**Files:**
- Modify: `crates/kiln-core/src/process.rs` (gains `find_on_path`, moved from the app)
- Modify: `crates/kiln-core/src/job.rs` (gains `offered` and backend availability)
- Create: `crates/kiln-core/src/suggest.rs` (only the `Offered` struct for now)
- Modify: `crates/kiln-core/src/lib.rs` (`pub mod suggest;`, `pub use process::find_on_path;`)
- Modify: `crates/kiln-app/src/settings.rs:278` (drop its own `find_on_path`; `pub use kiln_core::find_on_path;` so callers keep working)
- Test: `crates/kiln-core/tests/offered.rs`

**Interfaces:**
- Produces:
  - `kiln_core::find_on_path(path_var: &OsStr, program: &str) -> Option<PathBuf>`
  - `kiln_core::suggest::Offered { pub targets: Vec<String>, pub tools: Vec<Tool> }`, deriving `Debug, Clone, Default, PartialEq, Eq`. Targets are extensions, in registry order.
  - `Kiln::offered(&self, paths: &[&Path], opts: &Options) -> Offered`

- [ ] **Step 1: Write the failing tests** in `crates/kiln-core/tests/offered.rs`, using `support::fixture`:

```rust
fn no_ffmpeg() -> Options {
    let mut o = Options::default();
    o.engines.ffmpeg = Some(PathBuf::from(r"C:\nowhere\ffmpeg.exe"));
    o
}

#[test]
fn image_targets_need_no_engine() {
    let k = Kiln::new();
    let png = fixture("rgb_gradient.png");
    let o = k.offered(&[png.as_path()], &no_ffmpeg());
    assert!(o.targets.contains(&"jpg".to_owned()));
}

#[test]
fn a_missing_engine_offers_nothing_it_runs() {
    let k = Kiln::new();
    let mov = fixture("sample.mov");
    let o = k.offered(&[mov.as_path()], &no_ffmpeg());
    assert!(o.targets.is_empty());
    assert!(o.tools.is_empty());
}

#[test]
fn several_files_share_their_common_targets() {
    let k = Kiln::new();
    let (a, b) = (fixture("rgb_gradient.png"), fixture("sample.mp3"));
    assert!(k.offered(&[a.as_path(), b.as_path()], &Options::default()).targets.is_empty());
}

#[test]
fn merge_is_offered_for_two_pdfs() {
    let k = Kiln::new();
    let p = fixture("sample.pdf");
    let o = k.offered(&[p.as_path(), p.as_path()], &no_ffmpeg());
    assert!(o.tools.contains(&Tool::Merge));
}
```

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p kiln-core --test offered`
Expected: compile error, because `offered` doesn't exist.

- [ ] **Step 3: Implement**
  - `find_on_path` moves verbatim from the app into `process.rs` as `pub fn`. The app re-exports it, and its existing tests move with it.
  - In `job.rs`, add a private `fn available(&self, opts: &Options, backend: &str) -> bool`:
    - `"image"` and `"pdf"`: always true.
    - `"ffmpeg"` and `"pandoc"`: the configured path `is_file()`, or, when none is configured, `find_on_path(&PATH, name)` is `Some`.
    - `"libreoffice"`: a configured path that `is_file()`, else `find_libreoffice().is_some()`.
    - `"calibre"`: a configured folder that `exists()`, else `find_calibre().is_some()`.
  - `offered` works out each backend's availability once per call, in a small map. Targets are the extensions shared by every path's `targets_for`, kept when every `Step` of `registry().route(from, to)` uses an available backend, and listed in the first file's order. Tools are `tools_for(paths)`, minus all of them when any input is audio or video and FFmpeg is unavailable.

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p kiln-core --test offered` and `cargo test -p kiln-app settings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kiln-core crates/kiln-app/src/settings.rs
git commit -m "Suggestions: what's on offer, by installed engine"
```

---

### Task 2: `inspect` for images, with image estimates

**Files:**
- Create: `crates/kiln-core/src/facts.rs`
- Modify: `crates/kiln-core/src/lib.rs` (`pub mod facts; pub use facts::{inspect, Facts, Look, PdfFacts, Picture};`)
- Modify: `crates/kiln-core/src/raster.rs` (the WIC module gains `pub fn size(input: &Path) -> Result<(u32, u32)>`, from the frame's `GetSize` with no pixel copy)
- Modify: `crates/kiln-core/examples/gen_fixtures.rs` (the `images` group writes `photo.png`)
- Create: `tests/fixtures/photo.png` (generated)
- Test: `crates/kiln-core/tests/facts.rs`, plus unit tests in `facts.rs`

**Interfaces:**
- Consumes: `raster::{reads, decode, fit}`, `Kiln::probe`, `Kiln::registry().format_of`.
- Produces (all `#[derive(Debug, Clone, PartialEq)]`, `Look` also `Copy, Eq`):

```rust
pub struct Facts {
    pub ext: String,            // canonical, lowercase
    pub kind: Kind,
    pub bytes: u64,
    pub picture: Option<Picture>,
    pub media: Option<MediaInfo>,
    pub pdf: Option<PdfFacts>,
    pub bars: Option<bool>,
    /// Likely size of the result, by target extension, for targets a rule may pick.
    pub estimates: BTreeMap<String, u64>,
    /// Likely size after Compress.
    pub compressed: Option<u64>,
}
pub struct Picture { pub width: u32, pub height: u32, pub alpha: bool, pub animated: bool, pub look: Look }
pub enum Look { Photo, Graphic }
pub struct PdfFacts { pub pages: u32, pub has_text: Option<bool> }
pub fn inspect(kiln: &Kiln, path: &Path, opts: &Options, cancel: &CancelToken) -> Option<Facts>;
```

`inspect` returns `None` only when the extension isn't a format Kiln knows or the file can't be stat'ed. Otherwise it always returns `Some`, with fewer facts when reading fails.

- [ ] **Step 1: Add the fixture.** In `gen_fixtures`, the `images` group writes `photo.png`: 64 × 48 RGB, each channel a smooth gradient plus deterministic noise of ±40 from a fixed-seed LCG, so the picture has many colours and few equal neighbours. Run `cargo run -p kiln-core --example gen_fixtures -- images`. Check that `photo.png` is under 16 KB and holds no `c2pa` bytes (`grep -c c2pa tests/fixtures/photo.png` prints `0`).

- [ ] **Step 2: Write the failing tests**

Unit tests in `facts.rs`:

```rust
#[test] fn noise_reads_as_a_photo()   { assert_eq!(look_of(&noise(64, 48)), Look::Photo); }
#[test] fn stripes_read_as_a_graphic() { assert_eq!(look_of(&stripes(64, 48)), Look::Graphic); }
#[test]
fn huge_pictures_are_not_decoded() {
    assert!(decodes(7000, 7000));
    assert!(!decodes(7072, 7072));
}
```

In `tests/facts.rs`, with `let f = |name| inspect(&Kiln::new(), &fixture(name), &Options::default(), &CancelToken::new()).unwrap();`:

```rust
#[test]
fn a_noisy_picture_is_a_photo() {
    let p = f("photo.png").picture.unwrap();
    assert_eq!((p.width, p.height, p.look, p.alpha, p.animated), (64, 48, Look::Photo, false, false));
}
#[test] fn transparency_is_seen()      { let p = f("rgba_half_transparent.png").picture.unwrap(); assert!(p.alpha); assert_eq!(p.look, Look::Graphic); }
#[test] fn apng_frames_are_counted()    { assert!(f("sample.apng").picture.unwrap().animated); }
#[test] fn a_still_gif_is_not_animated() { assert!(!f("sample.gif").picture.unwrap().animated); }
#[test]
fn a_two_frame_gif_is_animated() {
    // Written to a temp folder with image::codecs::gif::GifEncoder, two 8 × 8 frames.
    assert!(inspect_temp_gif(2).picture.unwrap().animated);
}
#[test] fn svg_is_a_graphic()           { assert_eq!(f("sample.svg").picture.unwrap().look, Look::Graphic); }
#[test] fn heic_is_a_photo_by_its_type() { let p = f("sample.heic").picture.unwrap(); assert_eq!(p.look, Look::Photo); assert!(p.width > 0); }
#[test]
fn unreadable_images_keep_type_and_size() {
    for name in ["not_really.png", "truncated.png"] {
        let facts = f(name);
        assert_eq!(facts.kind, Kind::Image);
        assert_eq!(facts.bytes, std::fs::metadata(fixture(name)).unwrap().len());
        assert!(facts.picture.is_none());
        assert!(facts.estimates.is_empty());
    }
}
#[test]
fn image_estimates_land_within_2x() {
    // A 1024 × 768 smooth-plus-noise picture drawn into a temp folder, then inspected.
    // Each estimate is compared with a real Kiln::convert to that target.
    for to in ["jpg", "png"] {
        let (est, real) = estimate_and_real(to);
        assert!(est * 2 >= real && est <= real * 2, "{to}: {est} vs {real}");
    }
}
#[test]
#[ignore = "needs KILN_EXTRA_SAMPLES"]
fn real_photos_and_screenshots_are_told_apart() {
    // $KILN_EXTRA_SAMPLES/photos/* must be Photo, $KILN_EXTRA_SAMPLES/graphics/* Graphic,
    // and every jpg estimate in photos/ within 2× of a real encode.
}
```

- [ ] **Step 3: Run them and see them fail**

Run: `cargo test -p kiln-core --test facts` and `cargo test -p kiln-core --lib facts`
Expected: compile errors, because `facts` doesn't exist.

- [ ] **Step 4: Implement** `inspect`, `look_of(&RgbaImage) -> Look` and `decodes(w: u32, h: u32) -> bool` in `facts.rs`.
  - Constants: `MAX_PIXELS: u64 = 50_000_000`, `PREVIEW: u32 = 256`, `GRAPHIC_COLOURS: usize = 64`, `GRAPHIC_FLAT: f32 = 0.5`, `ESTIMATE_EXP: f64 = 1.0`. Tune the last three with the ignored test when samples are available, and leave a comment saying so.
  - Raster types (`raster::reads(ext)`, plus `apng`): read the size with `image::image_dimensions`. When `decodes`, decode with `raster::decode` and shrink with `thumbnail` to at most 256 px on the long side.
    - `look`: `Graphic` when the distinct RGB colours are ≤ `GRAPHIC_COLOURS`, or when the share of pixels equal to their right-hand neighbour is ≥ `GRAPHIC_FLAT`; otherwise `Photo`.
    - `alpha`: any alpha below 255.
    - `animated`: for GIF, the `GifDecoder`'s frames iterator yields a second frame; for PNG/APNG, `PngDecoder::is_apng()` and the decoder's frame count is above 1. Count frames lazily and stop at 2.
  - When decoding fails, `picture` stays `None`.
  - Too large to decode: `picture` with the header size, `look: Photo`, and `alpha: false`.
  - RAW (dng, cr2, cr3, nef, arw, orf, rw2, raf, pef, srw): size from `raster::wic::size` (whatever the WIC module is named), `look: Photo`.
  - HEIC, AVIF, JXL: size from `kiln.probe`, `look: Photo`.
  - SVG: the size `usvg` reports for the tree, `look: Graphic`, with alpha set from the decoded preview.
  - Estimates, only for a decoded preview: encode the preview, flattened to RGB8, in memory to JPG (at `opts.jpeg_quality`) and PNG. Each byte count is multiplied by `(full_px / preview_px).powf(ESTIMATE_EXP)` and stored under `"jpg"` and `"png"`.

- [ ] **Step 5: Run them and see them pass**

Run: `cargo test -p kiln-core --test facts` and `cargo test -p kiln-core --lib facts`
Expected: PASS, with one ignored test.

- [ ] **Step 6: Commit**

```bash
git add crates/kiln-core tests/fixtures/photo.png
git commit -m "Suggestions: inspect images, photo or graphic, with estimates"
```

---

### Task 3: `inspect` for PDF, audio and video, with time limits

**Files:**
- Modify: `crates/kiln-core/src/facts.rs`
- Modify: `crates/kiln-core/src/tools.rs:609` (`last_crop` becomes `pub(crate)`)
- Modify: `crates/kiln-core/src/pdf.rs` (`load` and `extract_text` become `pub(crate)`, or a `pub(crate) fn text_in_first(input, pages: usize) -> Result<bool>` is added beside them)
- Modify: `crates/kiln-core/examples/gen_fixtures.rs` (the `images` group converts `photo.png` to PDF with the same `convert` helper the `office` group uses, then names it `scan.pdf`)
- Create: `tests/fixtures/scan.pdf` (generated)
- Test: `crates/kiln-core/tests/facts.rs`

**Interfaces:**
- Consumes: `ffmpeg::analyse(opts, ctx, input, "-vf", filter, Some(secs))`, `tools::last_crop`, `Kiln::probe`, `quality::estimate(to: &Format, q: &Quality, info: &MediaInfo) -> Option<u64>`.
- Produces: the same `inspect`, now filling `pdf`, `media`, `bars`, video and AVIF/JXL `picture`, and `estimates["mp4"]`, `estimates["mp3"]` and `compressed`.

- [ ] **Step 1: Generate `scan.pdf`.** Run `cargo run -p kiln-core --example gen_fixtures -- images`. Check that it is under 32 KB and has no `c2pa` bytes.

- [ ] **Step 2: Write the failing tests** in `tests/facts.rs`:

```rust
#[test] fn a_pdf_with_text()  { assert_eq!(f("sample.pdf").pdf.unwrap().has_text, Some(true)); }
#[test] fn a_scanned_pdf()    { assert_eq!(f("scan.pdf").pdf.unwrap(), PdfFacts { pages: 1, has_text: Some(false) }); }
#[test] fn black_bars_are_found()  { assert_eq!(f("letterbox.mp4").bars, Some(true)); }
#[test] fn no_bars_are_found()     { assert_eq!(f("sample.mp4").bars, Some(false)); }
#[test]
fn video_has_a_frame_and_estimates() {
    let v = f("sample.mov");
    assert!(v.media.unwrap().duration.is_some());
    assert!(v.picture.is_some());
    assert!(v.estimates.contains_key("mp4"));
    assert!(v.compressed.is_some());
}
#[test]
fn audio_has_no_picture() {
    let a = f("sample.wav");
    assert!(a.media.is_some() && a.picture.is_none() && a.bars.is_none());
    assert!(a.estimates.contains_key("mp3"));
}
#[test]
fn a_missing_ffmpeg_gives_type_and_size() {
    let v = inspect(&Kiln::new(), &fixture("sample.mov"), &no_ffmpeg(), &CancelToken::new()).unwrap();
    assert!(v.media.is_none() && v.bars.is_none() && v.picture.is_none());
    assert!(v.bytes > 0);
}
#[test]
fn a_cancelled_inspect_returns_at_once() {
    let c = CancelToken::new();
    c.cancel();
    let t = Instant::now();
    let v = inspect(&Kiln::new(), &fixture("sample.mov"), &Options::default(), &c).unwrap();
    assert!(t.elapsed() < Duration::from_secs(1));
    assert!(v.bars.is_none());
}
#[test] fn broken_audio_keeps_type_and_size() { let a = f("corrupt.wav"); assert!(a.media.is_none() && a.bytes > 0); }
```

- [ ] **Step 3: Run them and see them fail**

Run: `cargo test -p kiln-core --test facts`
Expected: the new tests fail.

- [ ] **Step 4: Implement**
  - **Time limit.** Add `const LIMIT: Duration = Duration::from_secs(2)` and a private `Deadline` that owns a fresh `CancelToken` and spawns one watcher thread. The watcher polls every 25 ms and cancels the fresh token when the caller's token is cancelled or `LIMIT` has passed. Dropping the `Deadline` stops the watcher. Each FFmpeg/FFprobe call gets its own `Deadline`. Return early, before any call, when `cancel.is_cancelled()`.
  - **Media.** Use `crate::ffmpeg::probe_media` with a `Context` holding the deadline token (as `Kiln::probe` does, but cancellable).
    - Video: `picture` from its width and height, with `look: Photo`, `alpha: false` and `animated: false`.
    - Bars: run `analyse` with `"cropdetect=limit=24:round=2:reset=0"` over `BAR_SCAN_SECONDS: f64 = 3.0`. `bars` is `Some(true)` when `last_crop` differs from the frame size and is non-zero, `Some(false)` when the run succeeds otherwise, and `None` when it fails or is cut off.
    - Estimates: `estimates["mp4"]` comes from `quality::estimate(mp4, &opts.quality, &info)` and `estimates["mp3"]` for audio likewise. `compressed` uses `Quality { preset: Preset::Small, ..opts.quality }` against the source's own format.
  - **PDF.** `pages` comes from `lopdf::Document::load_mem(..).get_pages().len()`. On failure, `pdf` stays `None`.
    - When the file is at most `MAX_PDF_BYTES: u64 = 200_000_000`, run the text check on a spawned thread and wait with `recv_timeout(LIMIT)`. The check is whether `extract_text_from_mem_by_pages` (inside `catch_unwind`, as `pdf.rs` already does) gives any non-whitespace in the first 3 pages. A timeout, an error or an encrypted file gives `has_text: None`, and a thread that timed out is left to finish on its own.
  - **AVIF and JXL.** `picture` comes from the probe's width and height, `look: Photo`.

- [ ] **Step 5: Run them and see them pass**

Run: `cargo test -p kiln-core --test facts` and `cargo test -p kiln-core --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/kiln-core tests/fixtures/scan.pdf
git commit -m "Suggestions: inspect PDF text, media and black bars, within 2 s"
```

---

### Task 4: `suggest`, its rules and habits

**Files:**
- Modify: `crates/kiln-core/src/suggest.rs`
- Modify: `crates/kiln-core/src/lib.rs` (`pub use suggest::{about, suggest, Habits, Offered, Suggested, Suggestion, Suggestions};`)
- Test: unit tests in `suggest.rs`

**Interfaces:**
- Consumes: `Facts`, `Picture`, `Look`, `PdfFacts`, `MediaInfo`, `Offered`, `Tool`, `Kind`.
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)] pub enum Suggested { Convert(String), Tool(Tool) }
#[derive(Debug, Clone, PartialEq, Eq)] pub struct Suggestion { pub action: Suggested, pub reason: String, pub estimate: Option<u64> }
#[derive(Debug, Clone, Default, PartialEq, Eq)] pub struct Suggestions { pub convert: Option<Suggestion>, pub tool: Option<Suggestion> }
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Habits(BTreeMap<String, BTreeMap<String, u32>>);
impl Habits {
    pub const MAX_SOURCES: usize = 64;
    pub fn record(&mut self, from: &str, to: &str);   // adds 1; before adding a 65th source, drops the one with the fewest picks (then the first alphabetically)
    pub fn favourite(&self, from: &str) -> Option<&str>; // ≥ 3 picks and strictly the most picked
    pub fn is_empty(&self) -> bool;
    pub fn clear(&mut self);
}
pub fn suggest(files: &[Facts], offered: &Offered, habits: &Habits) -> Suggestions;
/// Two significant figures, decimal units: "2 MB", "2.1 MB", "180 MB", "950 KB".
pub fn about(bytes: u64) -> String;
```

- [ ] **Step 1: Write the failing tests.**
  - A `facts(ext, kind)` builder and a `with_picture`, `with_media`, `with_pdf` and `with_bars` helper set build the cases.
  - `all()` is an `Offered` holding every target and tool used below.
  - Table test `each_rule_suggests_its_target`, one row per rule. Each row asserts the action, the exact reason and the estimate:

| Case | Expect |
|---|---|
| gif, animated | `Convert("mp4")`, "Same motion at a fraction of the size." |
| heic; and cr2 | `Convert("jpg")`, "Opens anywhere. The original stays as it is." |
| png photo, no alpha, `estimates["jpg"] = 2_140_000` | `Convert("jpg")`, "A photo. JPG keeps the look at about 2.1 MB.", `Some(2_140_000)` |
| png photo, no estimate | "A photo. JPG keeps the look.", `None` |
| bmp graphic; and webp photo with alpha | `Convert("png")`, "Sharp edges and transparency, and it opens anywhere." |
| mov video | `Convert("mp4")`, "Plays on any phone, PC or browser." |
| wav | `Convert("mp3")`, "Plays anywhere at a fraction of the size." |
| md; and doc | `Convert("docx")`, "Opens in Word and Google Docs." |
| csv | `Convert("xlsx")`, "The spreadsheet format everyone can open." |
| key | `Convert("pptx")`, "The slide format everyone can open." |
| mobi | `Convert("epub")`, "Opens in most e-book readers." |
| vtt | `Convert("srt")`, "Works with almost every player." |

  - Tool table test `each_tool_rule`:
    - An mp4 of 600 MB over 60 s with `compressed: Some(183_400_000)` gives `Tool(Compress)` and "Large for its length. About 180 MB after."
    - An mp4 at 9 Mbit/s with no `compressed` gives "Large for its length."
    - An mp4 at 4 Mbit/s and 50 MB gives no compress suggestion.
    - Two PDFs give `Merge` and "One PDF, in the order shown."
    - Two images give `Merge` and "One PDF, a page each, in the order shown."
    - An mp4 with `bars: Some(true)` gives `Crop` and "Black bars on the sides."
  - More tests:
    - `already_in_place`: jpg photo, mp4 and mp3 give `convert: None`.
    - `scanned_pdf_gets_nothing`: `has_text: Some(false)` gives `Suggestions::default()`.
    - `nothing_matches`: a txt file gives nothing.
    - `missing_target_is_never_suggested`: a mov with `Offered { targets: vec!["avi".into()], .. }` gives `convert: None`.
    - `habits_win_at_three_not_two`: heic with `png` picked 2 times gives jpg; 3 times gives `Convert("png")` and "You usually pick PNG."
    - `a_tie_is_no_habit`: png ×3 and webp ×3 for heic gives the rule's jpg.
    - `several_files_must_agree`: two png photos with estimates 1 MB and 2 MB give jpg, "A photo. JPG keeps the look." and `Some(3_000_000)`. A png photo plus a png graphic gives `convert: None`.
    - `habits_keep_64_sources`: record 65 sources, the first one once and the rest twice. Expect 64 kept, without the first.
    - `about_rounds_to_two_figures`:
      - `about(2_040_000) == "2 MB"`
      - `about(2_140_000) == "2.1 MB"`
      - `about(183_400_000) == "180 MB"`
      - `about(950_400) == "950 KB"`
      - `about(999_600) == "1 MB"`
      - `about(9_960_000) == "10 MB"`
      - `about(500) == "1 KB"`
    - `rule_extensions_are_real_formats`: every extension named in the rule lists is `Some` under `Registry::builtin().format(ext)`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p kiln-core --lib suggest`
Expected: compile errors.

- [ ] **Step 3: Implement** `suggest`, `Habits` and `about`.
  - Rule order and extension lists are exactly the spec's tables.
    - RAW is dng, cr2, cr3, nef, arw, orf, rw2, raf, pef, srw.
    - The photo rule also needs `!animated`.
    - The graphic rule is: ext ≠ png, `Kind::Image`, not animated, and (`look == Graphic` or `alpha`).
    - Picture rules need `picture: Some`. The HEIC and RAW rule goes by extension alone.
  - Per file: the habit (when its target is offered), otherwise the first matching rule whose target is in `offered.targets`. With several files, a convert suggestion is returned only when every file gives `Some` with the same action. Then use the numberless reason and the sum of estimates when every file has one.
  - Habit reasons: `format!("You usually pick {}.", ext.to_ascii_uppercase())`. The estimate is `estimates.get(ext)`.
  - Tools: the first rule every file qualifies for, when `offered.tools` holds the tool.
    - Compress needs `Kind::Video`, plus `bytes * 8 / duration > 8_000_000` or `bytes > 500_000_000`.
    - The compress estimate is the sum of `compressed`; when any is missing, use the numberless reason.

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p kiln-core --lib suggest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kiln-core
git commit -m "Suggestions: rules, habits and shared suggestions for sets"
```

---

### Task 5: `kiln suggest`

**Files:**
- Modify: `crates/kiln-cli/src/main.rs` (a `Command::Suggest { inputs: Vec<PathBuf>, ffmpeg, pandoc, libreoffice, calibre }` arm, following `Command::Tool`)
- Test: `crates/kiln-cli/tests/suggest.rs`

**Interfaces:**
- Consumes: `inspect`, `Kiln::offered`, `suggest`, `Habits::default()`.
- Produces: the `kiln suggest FILE...` output.

- [ ] **Step 1: Write the failing tests** (run the binary the way `tests/tool.rs` does, on staged copies):

```rust
#[test]
fn one_line_per_file_then_the_set() {
    let out = kiln(&["suggest", &staged("photo.png"), &staged("sample.mov")]);
    assert!(out.status.success());
    assert_eq!(stdout(&out),
        "photo.png    JPG  A photo. JPG keeps the look at about 1 KB.\n\
         sample.mov   MP4  Plays on any phone, PC or browser.\n");
}
#[test]
fn a_file_with_nothing_to_suggest() {
    assert_eq!(stdout(&kiln(&["suggest", &staged("sample.jpg")])), "sample.jpg   No suggestion.\n");
}
#[test]
fn two_pdfs_suggest_merge() {
    let out = stdout(&kiln(&["suggest", &staged("sample.pdf"), &staged("scan.pdf")]));
    assert!(out.ends_with("(all)        Merge  One PDF, in the order shown.\n"), "{out}");
}
#[test]
fn a_missing_file_exits_2() { assert_eq!(kiln(&["suggest", "nope.png"]).status.code(), Some(2)); }
```

Write the expected strings from the layout rule in Step 3 and the real estimate. If the estimate for `photo.png` isn't "1 KB", fix the expected text to what `about` gives and say so in the commit.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p kiln-cli --test suggest`
Expected: FAIL (no `suggest` subcommand).

- [ ] **Step 3: Implement**
  - When any input doesn't exist, print `kiln: {path} doesn't exist.` to stderr and exit 2, before inspecting anything.
  - Rows:
    - One per file: its file name and `suggest(&[facts], &offered_for_that_file, &Habits::default()).convert`.
    - Then `(all)` with the set's tool suggestion.
    - Then, for more than one file, `(all)` with the set's shared convert suggestion.
  - Columns:
    - Names are left-aligned, padded to the longest name plus 3 spaces. `(all)` counts toward the longest name when it is printed.
    - Labels are format codes (`Format::code()`) or `Tool::label()`, padded to the longest label plus 2 spaces.
    - Then the reason.
  - A file with no suggestion prints its padded name, then `No suggestion.`.

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p kiln-cli --test suggest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kiln-cli
git commit -m "Suggestions: kiln suggest"
```

---

### Task 6: App settings: Suggestions on or off, habits, Forget picks

**Files:**
- Modify: `crates/kiln-app/src/settings.rs` (fields, `HEADER` comment lines, tests)
- Modify: `crates/kiln-app/src/layout.rs:346` (General row 3) and `Control`
- Modify: `crates/kiln-app/src/win/mod.rs` (control handling; habits recorded in `start`, line 819)
- Modify: `crates/kiln-app/src/win/render.rs` (draw the row, with Forget picks disabled when there are no habits)

**Interfaces:**
- Consumes: `kiln_core::Habits`.
- Produces:
  - `Settings.suggestions: bool` (`#[serde(default = "yes")]`, default `true`)
  - `Settings.habits: Habits` (`#[serde(default, skip_serializing_if = "Habits::is_empty")]`)
  - `Control::Suggestions(bool)` and `Control::ForgetPicks`
  - `App::record_habit(&mut self, action: &Action, paths: &[PathBuf])`, called from `start`

- [ ] **Step 1: Write the failing tests**
  - `settings.rs`:
    - `habits_and_suggestions_read_back`: save a `Settings` with `suggestions: false` and heic→png ×3, load it back, and compare equal. Also assert that the text contains `suggestions = false` and `[habits]`.
    - `missing_suggestions_means_on`: loading `theme = 'dark'` gives `suggestions == true` and empty habits.
    - `a_bad_habit_is_a_settings_error`: `[habits]\nheic = 3` gives the same `Err(SettingsError(..))` path as any bad value.
  - `layout.rs`, `settings_pages_hold_their_rows`: General rows are `["Output folder", "Theme", "Wheel keys", "Suggestions"]`, and the control count is `3 + 1 + 3 + 3 + 2 + 1 + 1 + 1`. `Control::Suggestions(true)`, `Control::Suggestions(false)` and `Control::ForgetPicks` come right after the three `Key` controls, in that order, and every control rect sits inside `panel`.
  - `win/mod.rs` (pure helper `fn habit_picks(action: &Action, paths: &[PathBuf]) -> Vec<(String, String)>`): converting three PNGs and a JPG to webp gives `[("jpg","webp"), ("png","webp")]`, once per source type. A tool or file action gives `[]`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p kiln-app`
Expected: the new tests fail.

- [ ] **Step 3: Implement.**
  - The General row 3 is an On/Off segmented control, `SEGMENT_WIDTH` each, then Forget picks as a button 8 DIP after the segments, sized like `OutputChoose`.
  - `HEADER` gains `# suggestions = true` and `# [habits]   # picks per source type, counted on this PC`.
  - Clicking On or Off sets the field and saves. Off cancels and forgets inspects (Task 7 provides `Files::forget_facts`; until then, just save).
  - Forget picks calls `habits.clear()` and saves.
  - `start` records `habit_picks` into `settings.habits` and saves, but only for `Action::Convert`.

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p kiln-app`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kiln-app
git commit -m "Suggestions: on/off and Forget picks in Settings, habits counted"
```

---

### Task 7: Inspecting rows, and the row's second line

**Files:**
- Create: `crates/kiln-app/src/win/inspect.rs` (the inspect thread)
- Modify: `crates/kiln-app/src/win/mod.rs` (`WM_KILN_FACTS = WM_APP + 7`, queueing on add, cancelling on remove and clear, handling the message)
- Modify: `crates/kiln-app/src/model.rs` (`Entry` fields, `detail`, `Files` helpers)
- Modify: `crates/kiln-app/src/quality.rs:428` (`clock` becomes `pub(crate)` for reuse)
- Modify: `crates/kiln-app/src/win/render.rs` (160 ms crossfade of the second line)

**Interfaces:**
- Consumes: `inspect`, `suggest`, `Kiln::offered`, `Habits`, `Suggestion`, `Facts`.
- Produces:
  - `Entry.facts: Option<Facts>`, `Entry.suggestion: Option<Suggestion>` and `Entry.facts_at: Option<Instant>`
  - `pub fn describe(f: &Facts) -> Option<String>` in `model.rs`
  - `Files::set_facts(&mut self, path: &Path, facts: Facts, suggestion: Option<Suggestion>) -> bool` (false when no row has that path)
  - `Files::forget_facts(&mut self)`
  - `Files::facts_for(&self, paths: &[PathBuf]) -> Option<Vec<Facts>>` (`None` until every path has facts)
  - `pub struct Inspector`, with `Inspector::new(run: impl Fn(&Path, &CancelToken) -> Option<Facts> + Send + 'static, done: impl Fn(PathBuf, Option<Facts>) + Send + 'static) -> Inspector`, `queue(&mut self, path: PathBuf)`, `cancel(&mut self, path: &Path)` and `cancel_all(&mut self)`

- [ ] **Step 1: Write the failing tests**
  - `model.rs`:
    - `describe` gives these, in order: `"4032 × 3024 photo"`, `"1200 × 800 graphic"`, `"1280 × 720 video, 1:30"`, `"audio, 3:42"`, `"12 pages"`, `"1 page"` and `"12 pages, no text"`. A txt file gives `None`.
    - `row_line_before_and_after_facts`:
      - Before: `"photos  ·  3.1 MB  ·  Converts to 12 formats"`.
      - After facts with a jpg suggestion estimated at 2,040,000: `"photos  ·  4032 × 3024 photo  ·  3.1 MB  ·  Suggested: JPG, about 2 MB"`.
      - With no suggestion: `"photos  ·  4032 × 3024 photo  ·  3.1 MB"`.
      - A txt file with facts but no description or suggestion keeps the "Converts to" line.
    - `row_shows_tool_when_no_convert`: an mp4 with only a Compress suggestion at 183,400,000 reads `"…  ·  Suggested: Compress, about 180 MB"`.
    - `facts_for_a_removed_row_are_dropped`: `set_facts` on a path not in the list returns `false` and changes nothing.
    - `facts_for_waits_for_every_file`.
  - `inspect.rs`:
    - `cancelled_paths_never_report`: the `run` closure blocks on a channel. Queue a, b and c, cancel b, then release them. `done` sees a and c only.
    - `cancel_all_stops_the_queue`.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p kiln-app`
Expected: compile errors.

- [ ] **Step 3: Implement**
  - **Inspector.**
    - One thread, set to `THREAD_PRIORITY_BELOW_NORMAL` on Windows.
    - A FIFO `VecDeque<(PathBuf, CancelToken)>` behind a `Mutex` plus `Condvar`.
    - `cancel` cancels the path's token and removes it from the queue.
    - `done` is skipped when the token was cancelled.
  - **Window.**
    - `done` posts `WM_KILN_FACTS` with the path and facts, the way `Probed` is posted.
    - Rows added while `settings.suggestions` is on are queued.
    - `remove_selected` and `clear` cancel their paths.
    - Turning Suggestions off calls `cancel_all` and `forget_facts`. Turning it on queues every row.
  - **On `WM_KILN_FACTS`.** Compute `suggest(&[facts], &kiln.offered(&[path], &opts), &settings.habits)`. Take `convert`, or else `tool`, and call `set_facts`, with `facts_at = Some(now)`.
  - **When habits change** (a recorded pick, or Forget picks), recompute every row's suggestion from its cached facts without a new fade.
  - **`detail`.** Join with the existing `"  ·  "`:
    - folder,
    - then the description when there is one,
    - then the size,
    - then `Suggested: {label}` plus `, about {about(e)}` when there's an estimate.
    - With no facts, or with neither a description nor a suggestion, keep today's "Converts to …" line.
  - **Render.** While `facts_at` is within 160 ms, draw the old line fading out and the new one fading in, ease-out, both in the muted tone.

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p kiln-app`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kiln-app
git commit -m "Suggestions: inspect rows in the background and show what Kiln found"
```

---

### Task 8: The wheel: dot, keyboard start and footer

**Files:**
- Modify: `crates/kiln-app/src/wheel.rs` (pure helpers and tests)
- Modify: `crates/kiln-app/src/win/mod.rs` (`WheelUi` gains `suggestion` and `dot_at`; `open_wheel` at 482; Enter on More; `wheel_hint` at 577; `WM_KILN_FACTS` updates an open wheel)
- Modify: `crates/kiln-app/src/win/render.rs` (draw the dot)

**Interfaces:**
- Consumes: `Files::facts_for`, `suggest`, `Kiln::offered`, `Suggested`, `Suggestion`, `Spot`, `Wheel`, `Action`.
- Produces:
  - `wheel::marks(w: &Wheel, s: &Suggested) -> Vec<Spot>`: `[Inner(i)]`, or `[Inner(w.items.len()), Outer(j)]` when the suggestion is in More, or `[]`
  - `wheel::start_spot(w: &Wheel, s: Option<&Suggested>) -> Spot`
  - `wheel::suggestion_footer(label: &str, reason: &str) -> String`
  - `wheel::shows_suggestion(hover: Option<Spot>, marks: &[Spot]) -> bool`

- [ ] **Step 1: Write the failing tests** in `wheel.rs`:
  - `the_dot_goes_on_an_inner_target`: `Convert("jpg")` on a PNG wheel gives `[Inner(i)]`, where `items[i].action == Action::Convert("jpg")`.
  - `a_target_in_more_marks_more_and_its_slot`: pick a target in `more`. Expect `[Inner(items.len()), Outer(j)]`.
  - `a_tool_marks_its_segment`: `Tool(Crop)` on a Tools wheel for an mp4.
  - `a_suggestion_not_on_the_wheel_marks_nothing`: expect `[]`, and `start_spot` gives `Inner(0)`.
  - `the_keyboard_starts_on_the_suggestion`: `start_spot` gives the first mark.
  - `suggestion_footer("JPG", "A photo. JPG keeps the look at about 2 MB.")` gives `"Suggested: JPG. A photo. JPG keeps the look at about 2 MB."`.
  - `shows_suggestion`:
    - `(None, [Inner(2)])` is true.
    - `(Some(Inner(2)), [Inner(2)])` is true.
    - `(Some(Outer(5)), [Inner(8), Outer(5)])` is true.
    - `(Some(Inner(1)), [Inner(2)])` is false.
    - `(None, [])` is false.

- [ ] **Step 2: Run them and see them fail**

Run: `cargo test -p kiln-app wheel`
Expected: compile errors.

- [ ] **Step 3: Implement**
  - **`open_wheel`.** When `settings.suggestions` is on and `files.facts_for(&paths)` is `Some`, compute the suggestion with `suggest(&facts, &kiln.offered(&paths, &opts), &settings.habits)`. Convert wheels take `convert`, Tools wheels take `tool`, and the File wheel takes none. `dot_at` stays `None`, so the dot shows fully. The keyboard hover is `start_spot`.
  - **When facts arrive** for a path of the open wheel and `facts_for` now returns `Some`, compute the suggestion, set `dot_at = Some(now)` and call `animate()`. Nothing else changes, and hover is not moved.
  - **Enter on More.** It opens the ring as today. When the marks include `Outer(j)`, the ring opens with hover `Outer(j)` rather than its usual first slot.
  - **`wheel_hint`.**
    - When a suggestion exists and `shows_suggestion(hover, &marks)`, the hint is `suggestion_footer(label, &reason)`, where `label` is the item's label (the code or the tool name).
    - Otherwise the hint is today's "Tab or right-click for … options." With nothing lit and no suggestion, keep today's hint.
  - **Render.** A filled circle of radius 2 DIP in the accent, centred 6 DIP above the top of the segment's icon (inner ring) or of its code text (outer ring). Its opacity follows the ring's own opacity, times an ease-out of `dot_at` over 160 ms.

- [ ] **Step 4: Run them and see them pass**

Run: `cargo test -p kiln-app`
Expected: PASS. Then run the app with a scratch APPDATA, drop `C:\kiln\target\samples` files, and confirm the dot and footer by eye.

- [ ] **Step 5: Commit**

```bash
git add crates/kiln-app
git commit -m "Suggestions: the dot, the keyboard start and the reason in the wheel"
```

---

### Task 9: Docs, screenshots and the full check

**Files:**
- Modify: `DESIGN.md` (accent uses gain "the suggestion dot". The wheel section gains a **Suggestion** bullet: the dot, its 160 ms fade, More plus its slot, and where the keyboard starts. The row section gains the second-line text.)
- Modify: `README.md` (a short **Suggestions** section saying what Kiln looks at, that it stays on the PC and how to turn it off; `kiln suggest` in the CLI section; `suggestions` and `[habits]` in the settings.toml example)
- Modify: `CHANGELOG.md` (an Unreleased entry)
- Create: `docs/screenshots/m9/*.png` (row hint, wheel dot and dot in More, each in dark and light)

- [ ] **Step 1: Write the docs.** Use the calm voice of the rest of the docs. Never mention AI vendors or assistants.

- [ ] **Step 2: Take the screenshots** with the m8 shoot script approach: Kiln running under a scratch APPDATA, inputs only from `C:\kiln\target\samples` (copy `photo.png`, `sample.mov` and `letterbox.mp4` there), keystrokes guarded to Kiln's window, at 100% scale. Check every shot for personal paths before keeping it.

- [ ] **Step 3: Run the full check**

Run: `cargo fmt --all --check`, then `cargo clippy --workspace --all-targets -- -D warnings`, then `cargo test --workspace` (in the background, about 11 minutes with pairs).
Expected: all clean and passing, with the one `KILN_EXTRA_SAMPLES` test ignored.

Run: `grep -c c2pa tests/fixtures/photo.png tests/fixtures/scan.pdf docs/screenshots/m9/*.png`
Expected: every count is `0`.

- [ ] **Step 4: Commit**

```bash
git add DESIGN.md README.md CHANGELOG.md docs/screenshots/m9
git commit -m "Suggestions: docs and screenshots"
git log --format='%an <%ae> | %cn <%ce>%n%B' origin/main..HEAD
```

Expected: only `chancrisp` and no "Claude" or "anthropic" anywhere. Don't push; the owner decides.
