# The quality harness

Converts each clip to each lossy target at each preset, with Kiln and with
FFmpeg at the same nominal setting, and compares the two. It writes
`target/harness/report.md` and exits non-zero on any miss.

    cargo test --release -p kiln-media --features harness --test harness

`--release` matters for the speed column: the debug build of the resampler and
the Windows encoders is many times slower than FFmpeg and the speed warnings
mean nothing there.

## What it needs

| | |
|---|---|
| `KILN_HARNESS_CLIPS` | A folder of clips (WAV, FLAC, AIFF, MP3, Ogg, Opus, M4A, WavPack, MKV, WebM, MP4, AC3, WMA), not searched below the folder. The maintainer uses about a dozen short public-domain clips: speech, piano, rock, orchestral and 5.1 film audio. Nothing is downloaded. Unset, the harness reports "no clips" and passes. |
| `KILN_HARNESS_MAX_SECS` | Optional. Uses only the first this many seconds of each longer clip (from a trimmed copy made in a temp folder; the clip isn't touched). The report says which clips were cut. |
| `KILN_HARNESS_CALIBRATE` | Optional, with `KILN_HARNESS_CLIPS`. Instead of the comparison, converts every music clip at each V level of MP3 and each whole quality step of Vorbis with Kiln alone, and writes `target/harness/calibration.md`: the median kbps per setting, which is what the two tables in `tune.rs` hold. |
| `ffmpeg` on PATH | Only the reference. When it isn't found the report says "FFmpeg not found", and with clips only Kiln's own checks run (the length against the source, the size against the request, the estimate's error): a miss there still fails the run. |
| `KILN_VISQOL` | The path to `visqol.exe`. When it isn't set the report says "ViSQOL: skipped (KILN_VISQOL not set)". |

Before the clips, the run checks its own metrics and then sends two synthetic
clips (stereo and 5.1, made in a temp folder) through the whole pipeline. That
table goes to `target/harness/synthetic-report.md`; quality misses on synthetic
audio are shown but don't fail the run, only a Kiln error does. With FFmpeg
missing that step is skipped.

## What is compared

The same setting on both sides: the MP3 V level (`-q:a`), the Vorbis quality
(`-q:a`), Opus's bitrate (`-b:a`), and the bitrate (`-b:a`) for AAC, WMA and
AC3. FFmpeg's encoders are `libmp3lame`, `libvorbis`, `libopus`, `aac`,
`wmav2` and `ac3`. The setting is the one `tune` gives the preset for what Kiln
wrote. M4A is AAC with another container, so it isn't run on its own. FFmpeg
writes at the rate and channel count Kiln wrote. When that folds channels
down, FFmpeg is asked to scale the fold to stay under full scale, as Kiln's
fold does; by default it only does that for integer encoders.

Both encoders' files are read back with Kiln's own readers.

| Metric | Pass when |
|---|---|
| Length, in decoded frames | within one codec frame of the source's length, after any rate conversion (MP3 1,152, AAC 1,024, Vorbis 2,048, Opus 960, WMA 2,048, AC3 1,536). FFmpeg's length is shown for information only: its `wmav2` drops 2,000 to 4,000 frames of tail |
| Loudness, EBU R128 integrated (BS.1770 K-weighting, 400 ms blocks, gates at -70 LUFS and -10 LU) | within 0.5 LU of FFmpeg's |
| High-frequency cutoff: the highest frequency whose long-term average spectrum is within 30 dB of the 1 to 4 kHz average | at least FFmpeg's minus 500 Hz for MP3 (LAME on both sides), Opus (libopus on both sides) and Kiln's own AC3 5.1 encoder. For AAC, WMA and AC3 stereo (Windows' encoders, whose bandwidth Kiln doesn't set) and Vorbis (aoTuV against libvorbis, tuned differently on purpose) a gap is a warning with both numbers |
| Size, from the file (for WMA, its data packets) | within 5% of the requested bitrate (CBR and ABR only) |
| ViSQOL, against the clip | at least FFmpeg's minus 0.1 |
| Speed, wall time | only a warning, when Kiln takes over twice FFmpeg's time |

The size is the whole file, except for WMA: its ASF header and index are a
few KB whatever the length, which on a clip of a few seconds is more than the
tolerance, so the size is the ASF data object's packets. Those still hold
about 0.6 s more than the clip, whatever its length, so under 30 s a WMA size
outside the tolerance is a warning rather than a miss.

ViSQOL is called as `visqol --reference_file ref.wav --degraded_file deg.wav`,
with all three made 48 kHz mono 16-bit WAVs by FFmpeg, and the number after
`MOS-LQO` is the score. That call has not been run against a real ViSQOL.
When a score can't be had, the report's notes say why, for that clip, target
and preset.

A clip that makes the harness panic is a failed row naming the clip, and the
clips after it still run.

## Estimate error

For the VBR rows (MP3 V levels, Vorbis quality, Opus) the report gives
(actual size - `tune::estimate_bytes`) / actual size, and ends with the median
per target, once for music and once for speech (a clip whose name has "speech"
is speech). A music target whose median absolute error is over 15% is marked
"recalibrate": its kbps table in `tune.rs` is then recalibrated from the
harness data (rulings R36 and R42). Speech is only reported: sparse speech
comes out far under any music table, so it isn't calibrated to.

The MP3 and Vorbis tables hold the median measured kbps per setting on the
music clips. To redo them, run with `KILN_HARNESS_CALIBRATE=1` and copy the
two lists it ends with into `MP3_VBR_KBPS` and `VORBIS_KBPS`. Between the whole
levels the slider's half and tenth steps are interpolated.

## Video mode

With `KILN_HARNESS_VIDEO_CLIPS` set to a folder of video clips (MP4, MOV,
M4V, MKV, WebM; a name with `.tmp.` in it is left out), the run also holds
Kiln's video encodes to FFmpeg's and writes `target/harness/video-report.md`.

    KILN_HARNESS_VIDEO_CLIPS=C:\clips KILN_HARNESS_MAX_SECS=20 cargo test --release -p kiln-media --features harness --test harness

| | |
|---|---|
| `KILN_HARNESS_VIDEO_CLIPS` | The folder, not searched below. A clip Kiln can't read (Theora, say, or HEVC without the extension) is listed as skipped. Unset, the video mode reports "no clips" and passes. Set, a run that measures nothing is a miss. |
| `KILN_HARNESS_MAX_SECS` | As for audio: a longer clip is cut to its first seconds, here by a packet-for-packet copy (`ffmpeg -t -c copy`) that is then the source for every row. |
| `KILN_HARNESS_VIDEO_CALIBRATE=1` | Optional. Kiln alone encodes every clip at quality 20, 45, 65, 80 and 90 for each codec and encoder, with no FFmpeg encode and no VMAF, and only the calibration is written, to `target/harness/video-calibration.md`. A few minutes instead of hours. |
| `KILN_HARNESS_VIDEO_ONLY` | Optional. A comma-separated list of `processor-h264`, `processor-hevc`, `card-h264` and `card-hevc`: only those encode rows run, and no copies, for trying one encoder's settings. The report then carries a miss saying it is a partial run, so it never passes as a whole one. Any other name stops the run. Unset, every row runs. |
| `KILN_HARNESS_VIDEO_KEEP=1` | Optional. Keeps every file made in `target/harness/video-work`, a folder per clip. Otherwise each clip's folder is removed when the clip is done. |
| `ffmpeg` and `ffprobe` on PATH | Needed, with `libvmaf`, `zscale`, `libx264` and `libx265` (the gyan.dev full build has them). The run checks for them first; one missing, or FFmpeg not found, is a miss. Every FFmpeg and ffprobe call has a time limit. |

Each clip is converted to MP4 for H.264 and HEVC at Small, High and Best
(Kiln's quality 45, 65 and 80), once on the processor and once on the
graphics card when it has an encoder for the codec. Each of Kiln's encodes is
then paired with FFmpeg's (`libx264` or `libx265 -preset medium`, into MP4,
the sound copied when it is AAC) at the CRF whose video stream leaves Kiln's
within ±15% of it (R67), measured as the verdict measures it: Kiln's size
against FFmpeg's (R83). The search uses fractional CRFs, starts from the
preset's (23, 20 and 17 for x264; 28, 24 and 20 for x265), steps along the
log of the size against CRF, and shares its encodes between the rows of a
clip. It stops within ±5% when that is cheap, settles for ±15% after five
encodes and gives up after eight; when no CRF in 0 to 51 reaches the band the
row is a miss that says so. Only the chosen encode is scored, and only its
time counts. Sizes are the video stream's (ffprobe's packet sizes), so the
sound and the container don't count.

| Metric | Pass when |
|---|---|
| VMAF and size | Kiln's VMAF is at least FFmpeg's minus 2 at a video size within ±15% of FFmpeg's, or Kiln is better on both |
| Length, the span of the decoded frames' times | within one frame of the source's |
| The first frame | is the source's first: of the source's first four frames, made small and gray, the nearest to the output's first is the first |
| Colour, which VMAF doesn't read | the mean U and V over ten frames, a second apart, within 12 levels (8-bit) of the source's; for a tone-mapped output, of the source through Hable's curve, since Kiln's own reference could carry the same fault |
| Colour tags (R40, R68) | Kiln wrote BT.2020 with PQ or HLG for HDR kept; BT.601 (SMPTE 170M) for an untagged source under 720 lines; BT.709 otherwise; limited range |
| The HDR path (R35) | HEVC on the graphics card stays HDR; everything else from HDR is tone-mapped to SDR |
| Copies | see below |
| Speed, wall time | only a warning, when Kiln takes over twice FFmpeg's time |

Anything that can't be measured is a miss, never a pass (R72): a VMAF that
fails on either side, an FFmpeg encode that fails, a missing FFmpeg feature,
and a graphics-card row the processor wrote because the card fell back.

VMAF is FFmpeg's `libvmaf` with its default model. Both sides are scaled to
the reference's upright size and pixel format, and their frames are synced
by their times from the first (`setpts=PTS-STARTPTS`, `-fps_mode
passthrough`), which also lines up variable frame rates. FFmpeg turns rotated
inputs upright as it reads them. Colour (R73): a file that says nothing of
its colour is labelled the way players read it (R68: BT.601 under 720 lines,
else BT.709); every tagged file keeps its own tags and `scale` converts it
into the reference's matrix (its input matrix named from the stream's tags,
not left to the frames, which may not say), so a wrong tag costs what it
would cost a viewer.

For an HDR source, an output kept HDR is compared with the clip in PQ (or
HLG) at 10 bits; those scores are indicative. An output tone-mapped to SDR is
compared with Kiln's own tone mapping (R71): Kiln's quality-90 H.264 encode
of the clip on the same encoder is the reference, FFmpeg encodes from that
reference at the matched size, and both are scored against it. How far Kiln's
tone curve is from Hable's (zscale) is a note, scored with the
`vmaf_v0.6.1neg` model, which doesn't reward a picture with more contrast
than its reference; no row is judged on it.

Lengths and frame counts come from the decoded frames, not the packets: a
file without an edit list may hold packets that are never shown.

### Copies

Original must copy the picture. Each packet's payload, the picture's and the
copied sound's, is hashed (`ffprobe -show_data_hash MD5`) on both sides, and
the copy must be the source's packets from some packet on, every one after
it: only hidden lead-in before the first shown frame may be dropped from the
front (R69), and the sound may start up to two packets late (R76). The
picture's span is within one frame of the source's and its first frame is
the source's.

A clip goes into MKV and back to MP4. A turned picture, or an MP4 whose edit
list starts the picture a frame or more after the key frame a copy begins at,
goes into MOV and back to MP4 instead (R74), and then Kiln must say MKV
can't copy it (`can_copy_video`) and its MKV encode must keep the source's
length and first frame. Kiln saying the opposite of what ffprobe shows is a
miss. An MKV clip goes to MP4 and back.

## Video calibration

Every Kiln encode is measured for the size estimate: the preset rows, and
encodes at quality 20 and 90 made for the calibration alone, so the ends of
the slider and Fit under's ceiling are measured. Each counts for the encoder
that wrote it (`JobReport.hardware`). The report gives the median bits per
pixel per encoder, codec and quality, and fits one table per encoder (R70),
in `tune.rs`'s shape (H.264 a + b·(q/100)², HEVC a factor of H.264's, times
the content term (source bpp / reference)^k for a source whose bitrate is
known, R81), to the encodes' own errors: the model whose larger of the two
codecs' median absolute errors is least. The reference is the median of the
sources' own bits per pixel (their video bitrate over width × height × frame
rate); k is searched from 0 to 1. The table alone (k = 0) is fitted beside
it, to show what the content term buys. It then gives the median absolute
error of the whole file's estimate per encoder and codec, with `tune.rs` as
it is, with the fit and with the table alone; over 15% with `tune.rs` as it
is is a miss.

`tune.rs` holds the two tables as `CARD` and `PROCESSOR`; the estimate uses
the one for the encoder the plan resolves to: the processor's for Processor,
and for Graphics card or Automatic when no graphics card encoder starts. It
never starts one to find out (R82): until a job or the app's worker has
asked, it takes the graphics card's. To recalibrate, run with
`KILN_HARNESS_VIDEO_CALIBRATE=1` and copy each encoder's a, b, factor, k and
reference into its table.
