# Suggestions: design

Date: 2026-10-19. Status: approved in conversation, awaiting review of this
written spec.

## Where this fits

Kiln is getting help that makes it easier for people who don't know file
formats. The work is split into five sub-projects, each with its own spec,
plan and build, in this order:

1. **Suggestions** (this spec): rules over what Kiln sees in a file.
2. OCR: scanned PDFs and images to text, with Windows' offline recognizer.
3. Speech to subtitles: whisper.cpp as an optional engine.
4. Image tools: upscale and remove background, with local ONNX models.
5. A cloud assistant: opt-in, the user's own API key, a previewed plan.

Sub-projects 1 to 4 keep the README's promise that nothing leaves the PC.

## Goal

When files are added, Kiln looks at them and, when one choice clearly helps,
says what it would do and why. The convert or tool that most people want
should be the one Kiln points at, with a one-line reason and, where it can,
an estimate of the result.

Success looks like this:

- The suggested target is usually what the user picks.
- Kiln stays quiet when nothing clearly helps.
- Formats keep their places in the wheel, so muscle memory still works.
- Nothing slows down: the wheel opens as fast as today, and running jobs
  aren't held up.

## Decisions made

| Question | Decision |
|---|---|
| How a suggestion shows in the wheel | A small celadon dot on the suggested segment. Places never change. |
| How closely Kiln looks | Inside the file, in the background, as files are added. |
| Which wheels suggest | Convert and Tools. |
| The file list | The row's second line shows the suggestion before any wheel opens. |
| Learning | Rules plus the user's habits, counted on this PC only. |
| Architecture | `inspect` and `suggest` in kiln-core; the app and CLI call them. |

## Architecture

Two new units in kiln-core, with one job each:

- `facts.rs`: `inspect` reads a file once and returns `Facts`. It does input
  and output and nothing else.
- `suggest.rs`: `suggest` turns facts, what is on offer and habits into at
  most one convert and one tool suggestion. It is a pure function with no
  input or output.

The app runs `inspect` off the UI thread and caches the facts per row. The
CLI calls both directly. The later assistant can call the same functions.

### Facts

```rust
pub struct Facts {
    pub ext: String,
    pub kind: Kind,
    pub bytes: u64,
    /// Images, and the frame size of video.
    pub picture: Option<Picture>,
    /// Audio and video: what ffprobe reports.
    pub media: Option<MediaInfo>,
    pub pdf: Option<PdfFacts>,
    /// Video only: whether black bars were found.
    pub bars: Option<Bars>,
}

pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub alpha: bool,
    pub animated: bool,
    pub look: Look,
}

pub enum Look {
    Photo,
    Graphic,
}

pub struct PdfFacts {
    pub pages: u32,
    /// `None` when the PDF is encrypted or can't be read.
    pub has_text: Option<bool>,
}

pub fn inspect(kiln: &Kiln, path: &Path, opts: &Options, cancel: &CancelToken) -> Facts;
```

How each kind is read:

- **Raster images** (PNG, JPG, WEBP, BMP, GIF, TIFF, ICO, TGA, QOI, PPM):
  decoded and shrunk to at most 256 px on the long side.
  - `look` is `Graphic` when the preview has few distinct colours or most
    pixels match their right-hand neighbour; otherwise `Photo`. The
    thresholds are constants tuned on the fixtures and on real photos and
    screenshots kept outside the repo.
  - `alpha` is true when any pixel is not fully opaque.
  - `animated` is true for a GIF or APNG with more than one frame.
- **Camera RAW and HEIC**: always `Photo`, sized from the header, with no
  full decode.
- **SVG**: always `Graphic`, sized from its view box.
- **AVIF and JXL**: size from ffprobe; `look` is `Photo`, the common case.
- **PDF**: page count, and whether the first three pages draw any text.
- **Audio and video**: the existing `Kiln::probe`. For video, FFmpeg's
  black-bar detector runs on a few sampled seconds to set `bars`.
- **Everything else**: type and size only.

Limits:

- `inspect` never fails. A file it can't read gives fewer facts.
- Images over 50 megapixels skip the decode and keep only header facts.
- PDFs over 200 MB skip the text check.
- Every FFmpeg run and PDF read honours `cancel` and stops after 2 s.

### Suggestion

```rust
pub struct Suggestion {
    pub action: Suggested,
    /// One calm sentence, shown as is.
    pub reason: String,
    /// The likely size of the result in bytes, when it can be estimated.
    pub estimate: Option<u64>,
}

pub enum Suggested {
    Convert(String),
    Tool(Tool),
}

/// What the files can become and which tools apply, counting only
/// targets whose engine is installed.
pub struct Offered {
    pub targets: Vec<String>,
    pub tools: Vec<Tool>,
}

/// Picks per source type: from extension to (target extension to count).
pub struct Habits { /* BTreeMap<String, BTreeMap<String, u32>> */ }

pub struct Suggestions {
    pub convert: Option<Suggestion>,
    pub tool: Option<Suggestion>,
}

impl Kiln {
    /// The common targets and tools for `paths`, without targets whose
    /// engine can't be found.
    pub fn offered(&self, paths: &[&Path], opts: &Options) -> Offered;
}

pub fn suggest(files: &[Facts], offered: &Offered, habits: &Habits) -> Suggestions;
```

The convert wheel shows `convert`, the Tools wheel shows `tool`.

Ground rules, in order:

1. Only something in `offered` can be suggested, so a suggestion never
   fails for a missing engine.
2. For several files, a conversion is suggested only when every file gets
   the same answer.
3. Habits come first. When the user has picked the same target for a source
   type at least 3 times, and it is their most picked target for that type,
   it is the suggestion. The reason reads "You usually pick PNG."
4. Otherwise the first matching rule below wins.
5. When nothing matches, there is no suggestion.

### Rules

Within each table, rules are checked from the top and the first match
wins.

Convert:

| File | Suggests | Reason |
|---|---|---|
| Animated GIF | MP4 | "Same motion at a fraction of the size." |
| HEIC, camera RAW | JPG | "Opens anywhere. The original stays as it is." |
| PNG, BMP, TIFF, WEBP, AVIF, JXL, TGA, QOI or PPM photo without transparency | JPG | "A photo. JPG keeps the look at about 2 MB." |
| A flat graphic, or any transparency, in a type other than PNG | PNG | "Sharp edges and transparency, and it opens anywhere." |
| Video other than MP4 | MP4 | "Plays on any phone, PC or browser." |
| WAV, AIFF, WMA, AC3, OGG, OPUS, AAC, WV | MP3 | "Plays anywhere at a fraction of the size." |
| DOC, RTF, ODT, PAGES, WPD, and markup (MD, RST, ORG, ADOC, TEX, TYP, WIKI, TEXTILE, DBK) | DOCX | "Opens in Word and Google Docs." |
| XLS, ODS, NUMBERS, CSV | XLSX | "The spreadsheet format everyone can open." |
| PPT, ODP, KEY | PPTX | "The slide format everyone can open." |
| MOBI, AZW3, FB2 | EPUB | "Opens in most e-book readers." |
| ASS, VTT | SRT | "Works with almost every player." |

Tools:

| Files | Suggests | Reason |
|---|---|---|
| Video above 8 Mbit/s on average, or over 500 MB | Compress | "Large for its length. About 180 MB after." |
| Two or more PDFs | Merge | "One PDF, in the order shown." |
| Two or more images | Merge | "One PDF, a page each, in the order shown." |
| Video with black bars | Crop | "Black bars on the sides.", "Black bars above and below." or "Black bars around the picture." |

Crop is worded by the bars found: left and right only, top and bottom only,
or both. A bar counts only when it takes at least 4% of the frame on its
axis, about 2% a side; thinner bars get no Crop. Several videos whose bars
differ share the numberless form "Black bars to crop."

Numbers in reasons come from `estimate` and are rounded to two significant
figures. A JPG photo, an MP4 video and an MP3 get no convert suggestion:
they are already in the format the rules would pick.

A scanned PDF (`has_text == Some(false)`) gets no suggestion in this
sub-project. OCR adds "Get the text" for it.

### Estimates

- **Images**: the 256 px preview is encoded at the target's preset, and the
  byte count is scaled by the ratio of full to preview pixels. It must land
  within 2× of a real encode on the fixtures. Shown as "about".
- **Video and audio**: the existing `quality::estimate`.
- **Everything else**: none, and the reason leaves the number out.

## The app

### Inspecting

- When rows are added and suggestions are on, each new row queues an
  `inspect` on one dedicated low-priority thread, separate from the job
  pool, so it never slows a conversion.
- The result is posted to the window as `WM_KILN_FACTS` (`WM_APP + 7`) with
  the row's path. The row stores `facts` and its own `suggestion`.
- Removing a row, or clearing the list, cancels its inspect.

### The row

- Before facts arrive, the second line is unchanged:
  `folder · 3.1 MB · Converts to 12 formats`.
- After, it reads `folder · 4032 × 3024 photo · 3.1 MB · Suggested: JPG,
  about 2 MB`, fading in over 160 ms. It stays muted text.
- Descriptions by kind: `4032 × 3024 photo`, `1200 × 800 graphic`,
  `1280 × 720 video, 1:30`, `audio, 3:42`, `12 pages`, `12 pages, no text`.
- With no suggestion, the description replaces "Converts to N formats" and
  nothing else is added.

### The wheel

- The suggested segment gets a 4 px celadon dot centred 6 px above its
  icon. Places never change.
- When the suggestion is in More, the More segment and the suggestion's
  slot in the outer ring both get the dot.
- Opened from the keyboard, the wheel starts on the suggestion. When the
  suggestion is in More, it starts on More, and Enter opens the ring with
  the suggestion lit.
- When facts arrive while the wheel is open, the dot fades in over 160 ms.
  Nothing moves or pulses.
- The Tools wheel marks Compress, Merge or Crop the same way.
- The wheel's suggestion uses the facts of every file it was opened for.
  Until all of them are in, it shows no dot.
- Footer: while nothing is lit, or while the suggested segment is lit, it
  reads "Suggested: JPG. " followed by the reason. While another segment is
  lit it keeps "Tab or right-click for … options."

### Habits

- Each conversion started from the wheel or the options sheet adds one to
  `habits[from][to]`. Batches count once per source type.
- They are saved in `settings.toml`:

  ```toml
  [habits]
  heic = { png = 3 }
  ```

- At most 64 source types are kept; the least used goes first.

### Settings

- **Settings > General** gains a **Suggestions** row: an On / Off segmented
  control and a **Forget picks** button, which is disabled when there are
  no habits.
- `settings.toml` gains `suggestions = true`.
- Off hides the dots, the row hints and the footer reasons, and skips
  inspecting entirely.

### Design notes

DESIGN.md's list of accent uses gains "the suggestion dot". The wheel
section describes the dot, its fade and where the keyboard starts.

## Command line

`kiln suggest FILE...` prints each file's convert suggestion, then the
tool suggestion for the whole set, and the set's shared convert suggestion
when there is more than one file:

```text
photo.png   JPG       A photo. JPG keeps the look at about 2 MB.
clip.mov    MP4       Plays on any phone, PC or browser.
(all)       Compress  Large for its length. About 180 MB after.
```

- It prints "No suggestion." for a file with none.
- It ignores habits, so its output is the same every time.
- It exits with 0, or with 2 when a file doesn't exist.

## Errors

There is nothing for the user to see. `inspect` returns fewer facts instead
of failing, and `suggest` returns nothing instead of guessing.

## Testing

- `suggest.rs`: a table of hand-built `Facts`, one case per rule, plus:
  - habits win at 3 picks and not at 2;
  - several files suggest a conversion only when every file agrees;
  - a target whose engine is missing is never suggested;
  - a file already in the rules' format gets no convert suggestion.
- `facts.rs`, against fixtures. `gen_fixtures` makes four new ones of a few
  KB each:
  - `photo.png`: a noisy, many-coloured picture;
  - `graphic.png`: flat shapes with transparency;
  - `scan.pdf`: one page that is only an image;
  - `bars.mp4`: a short video with black bars on the left and right.

  Each is read with the expected `look`, `alpha`, `has_text` or `bars`, and
  image estimates land within 2× of a real encode.
- Real photos and screenshots in `KILN_EXTRA_SAMPLES` check the photo and
  graphic thresholds. They stay out of the repo.
- kiln-cli: `kiln suggest` output for the fixtures.
- kiln-app:
  - the row's second line before and after facts;
  - where the dot goes, in the inner ring and in More;
  - where the keyboard starts;
  - `[habits]` and `suggestions` reading and writing;
  - the General page layout with the new row.
- Screenshots, dark and light: the row hint, the wheel dot and the dot in
  More.

## Out of scope

- OCR, and the "Get the text" suggestion for scanned PDFs (sub-project 2).
- Anything the cloud assistant does (sub-project 5).
- Suggestions for audio trimming or silence.
- Habits for tools.
