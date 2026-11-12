//! Photos made smaller to a size, resized, and saved without where they
//! were taken.
//! Kiln only reads the photo it is given. Trial encodes stay in memory;
//! only what is kept is written, to the job's own folder, and from there
//! placed next to the original under a new name.

use std::fs;
use std::path::{Path, PathBuf};

use image::DynamicImage;

use crate::converter::{CancelToken, Options};
use crate::error::{Error, Result};
use crate::metadata::{self, Kept};
use crate::registry::Kind;
use crate::tools::Tool;
use crate::{facts, raster, suggest};

/// The lowest and highest JPG and WEBP quality Make smaller tries.
const QUALITY_LOW: u8 = 40;
const QUALITY_HIGH: u8 = 92;
/// How many times Make smaller scales a photo down when its lowest quality,
/// or for a PNG the photo itself, is still over the size.
const SCALINGS: usize = 3;
/// A scaled-down photo aims this far under the size, by area, so that one
/// round usually lands under it.
const MARGIN: f64 = 0.95;
/// The JPG and WEBP quality for Make smaller with no size, as Compress had.
const PLAIN_QUALITY: u8 = 75;

/// What a photo of type `ext` is saved as when it is saved again: the same
/// type, or JPG for HEIC and AVIF, which Kiln can't write.
pub(crate) fn saved_as(ext: &str) -> &str {
    if raster::HEIF.contains(&ext) {
        "jpg"
    } else {
        ext
    }
}

/// `bytes` written to a new file in the job's folder `work`, named for
/// `ext`.
fn write_out(work: &Path, ext: &str, bytes: &[u8]) -> Result<PathBuf> {
    use std::io::Write;
    let tmp = work.join(format!("out.{ext}"));
    // A new file only: nothing is written over.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| Error::io(&tmp, e))?;
    Ok(tmp)
}

/// The photo at `input`, of type `ext`, without where it was taken or the
/// camera details, written in the job's folder `work`. Returns the file
/// and its type. A JPG, PNG, WEBP or GIF keeps its picture byte for byte,
/// its colour profile and its orientation; other types are saved again at
/// the Photos quality, keeping the colour profile, and a HEIC or AVIF
/// becomes a JPG.
pub(crate) fn without_location(
    input: &Path,
    ext: &str,
    opts: &Options,
    work: &Path,
    cancel: &CancelToken,
) -> Result<(PathBuf, String)> {
    if metadata::strips_in_place(ext) {
        raster::check_signature(input, ext)?;
        let bytes = fs::read(input).map_err(|e| Error::io(input, e))?;
        let stripped = metadata::strip(input, &bytes, ext)?;
        if stripped == bytes {
            return Err(Error::NothingToDo(
                "There's no location data or camera details to remove.".to_owned(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        return Ok((write_out(work, ext, &stripped)?, ext.to_owned()));
    }
    let out_ext = saved_as(ext);
    let fallback = raster::Fallback {
        options: opts,
        work,
    };
    let decoded = raster::decode_with(input, ext, cancel, Some(fallback))?;
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let tmp = work.join(format!("out.{out_ext}"));
    raster::encode(
        &decoded.image,
        &tmp,
        out_ext,
        opts.jpeg_quality,
        decoded.icc.as_deref(),
    )?;
    Ok((tmp, out_ext.to_owned()))
}

/// Whether the picture at `input` moves: an animated GIF, PNG or WEBP.
/// Saved again, it would keep only its first frame.
fn moving(input: &Path, ext: &str) -> bool {
    match ext {
        "gif" | "png" => facts::animated(input, ext),
        "webp" => {
            // An extended WEBP says so in its flags.
            let mut head = [0u8; 21];
            fs::File::open(input)
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
                .is_ok_and(|()| &head[12..16] == b"VP8X" && head[20] & 0x02 != 0)
        }
        _ => false,
    }
}

/// A photo ready to be saved again: upright, at the size asked, with the
/// colour profile and the metadata it keeps.
struct Prepared {
    image: DynamicImage,
    icc: Option<Vec<u8>>,
    kept: Kept,
    /// The type it is saved as.
    ext: String,
}

impl Prepared {
    /// The photo at `input`, of type `ext`, turned upright and its long
    /// side brought down to `long_side`. Its metadata is kept unless
    /// `remove_location` is set, or it becomes another type.
    /// Without a job's folder `work`, a photo Windows can't read isn't
    /// handed to FFmpeg, which would need one.
    fn read(
        input: &Path,
        ext: &str,
        long_side: Option<u32>,
        remove_location: bool,
        opts: &Options,
        work: Option<&Path>,
        cancel: &CancelToken,
    ) -> Result<Prepared> {
        let fallback = work.map(|work| raster::Fallback {
            options: opts,
            work,
        });
        let decoded = raster::decode_with(input, ext, cancel, fallback)?;
        let out = saved_as(ext);
        let kept = if remove_location || out != ext {
            Kept::default()
        } else {
            Kept::read(input)
        };
        Ok(Prepared {
            image: match long_side {
                Some(side) => scaled(&decoded.image, side),
                None => decoded.image,
            },
            icc: decoded.icc,
            kept,
            ext: out.to_owned(),
        })
    }

    /// `img`, this photo at some size, encoded at `quality` with the
    /// metadata it keeps.
    fn encode(&self, img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
        let bytes = raster::encode_bytes(img, &self.ext, quality, self.icc.as_deref())?;
        self.kept.embed(bytes, &self.ext)
    }
}

/// `img` with its long side at most `side`, scaled down with Lanczos; never
/// enlarged.
pub(crate) fn scaled(img: &DynamicImage, side: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= side {
        return img.clone();
    }
    let (nw, nh) = crate::quality::scaled(w, h, Some(side), false);
    img.resize_exact(nw.max(1), nh.max(1), image::imageops::FilterType::Lanczos3)
}

/// What one try at a size came to: a file that fits, or the smallest it got.
#[derive(Debug, PartialEq, Eq)]
enum Trial {
    Fits(Vec<u8>),
    Over(u64),
}

/// The highest quality between [`QUALITY_LOW`] and [`QUALITY_HIGH`] at
/// which `encode` gives `max` bytes or fewer, found by halving: at most
/// six encodes, the last at the lowest quality when none fits.
fn highest_fitting(max: u64, mut encode: impl FnMut(u8) -> Result<Vec<u8>>) -> Result<Trial> {
    let (mut low, mut high) = (QUALITY_LOW, QUALITY_HIGH);
    let mut best = None;
    let mut smallest = u64::MAX;
    while low <= high {
        let q = low + (high - low).div_ceil(2);
        let bytes = encode(q)?;
        let n = bytes.len() as u64;
        if n <= max {
            best = Some(bytes);
            low = q + 1;
        } else {
            smallest = smallest.min(n);
            high = q - 1;
        }
    }
    Ok(best.map_or(Trial::Over(smallest), Trial::Fits))
}

/// The first file of `max` bytes or fewer that `trial` makes of `base`,
/// trying `base` as it is and then up to [`SCALINGS`] times scaled down by
/// the root of how far over it was. When none fits, the error says the
/// smallest it reached.
fn shrink_to_fit(
    base: &DynamicImage,
    max: u64,
    mut trial: impl FnMut(&DynamicImage) -> Result<Trial>,
) -> Result<Vec<u8>> {
    let mut smallest = u64::MAX;
    let mut current = None;
    for round in 0..=SCALINGS {
        let img = current.as_ref().unwrap_or(base);
        let over = match trial(img)? {
            Trial::Fits(bytes) => return Ok(bytes),
            Trial::Over(n) => n,
        };
        smallest = smallest.min(over);
        let long = img.width().max(img.height());
        let factor = (max as f64 / over as f64 * MARGIN).sqrt();
        let next = (f64::from(long) * factor).floor() as u32;
        if round == SCALINGS || next == 0 || next >= long {
            break;
        }
        current = Some(scaled(base, next));
    }
    Err(Error::OutOfReach {
        max_bytes: max,
        smallest,
    })
}

/// `Err(Cancelled)` once `cancel` is set.
fn going(cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// The photo at `input`, of type `ext` (JPG, PNG or WEBP), made smaller in
/// the same type and written in the job's folder `work`; returns the file
/// and its type. With a size in `opts.tool.max_bytes`, a JPG or WEBP takes
/// the highest quality that fits, scaled down if even the lowest doesn't,
/// and a PNG is scaled down until it fits, keeping every colour. With no
/// size, it is saved once at a fixed quality. Either way it is smaller than
/// the original, or nothing is written.
pub(crate) fn smaller(
    input: &Path,
    ext: &str,
    opts: &Options,
    work: &Path,
    cancel: &CancelToken,
) -> Result<(PathBuf, String)> {
    let (bytes, out) = smaller_bytes(input, ext, opts, Some(work), cancel)?;
    Ok((write_out(work, &out, &bytes)?, out))
}

/// How big [`smaller`] would make the photo at `input`, worked out in
/// memory with the same trial encodes, writing nothing: the Make smaller
/// dialog's estimate. Its errors are `smaller`'s. A JPG only FFmpeg can
/// read is the exception: FFmpeg's picture of it goes to a private folder
/// of Kiln's, removed once it is read.
pub(crate) fn smaller_size(
    input: &Path,
    ext: &str,
    opts: &Options,
    cancel: &CancelToken,
) -> Result<u64> {
    going(cancel)?;
    smaller_bytes(input, ext, opts, None, cancel).map(|(bytes, _)| bytes.len() as u64)
}

/// [`smaller`]'s file, in memory, and its type.
fn smaller_bytes(
    input: &Path,
    ext: &str,
    opts: &Options,
    work: Option<&Path>,
    cancel: &CancelToken,
) -> Result<(Vec<u8>, String)> {
    if moving(input, ext) {
        return Err(Error::ToolUnavailable(format!(
            "Kiln can't {}.",
            Tool::Smaller.phrase(Kind::Image, "animated pictures")
        )));
    }
    let original = fs::metadata(input).map_err(|e| Error::io(input, e))?.len();
    let tool = &opts.tool;
    if let Some(max) = tool.max_bytes.filter(|&max| max >= original) {
        // A photo under the size is left alone, unless a long side it is
        // over was asked for too: then it is brought down to it.
        let over_side = tool.long_side.is_some_and(|side| {
            raster::upright_size(input, ext).is_some_and(|(w, h)| w.max(h) > side)
        });
        if !over_side {
            return Err(Error::NothingToDo(format!(
                "This file is already smaller than {}.",
                suggest::about(max)
            )));
        }
    }
    let read = |work| {
        Prepared::read(
            input,
            ext,
            tool.long_side,
            tool.remove_location,
            opts,
            work,
            cancel,
        )
    };
    let photo = match read(work) {
        // The estimate has no job folder, and FFmpeg's picture of a JPG
        // Kiln's own reader can't read needs one: a private one, gone
        // when the photo is read (D21). Without FFmpeg nothing is made.
        Err(Error::Decode { .. })
            if work.is_none() && ext == "jpg" && crate::ffmpeg::installed(opts) =>
        {
            let scratch = crate::job::WorkDir::for_options(opts)?;
            read(Some(scratch.path()))?
        }
        read => read?,
    };
    let bytes = match tool.max_bytes {
        None => {
            going(cancel)?;
            let bytes = photo.encode(&photo.image, PLAIN_QUALITY)?;
            if bytes.len() as u64 >= original {
                return Err(Error::NothingToDo(
                    "This file is already as small as Kiln can make it.".to_owned(),
                ));
            }
            bytes
        }
        Some(max) => {
            // Never larger than the original, whatever the size asked.
            let aim = max.min(original.saturating_sub(1));
            let shrunk = shrink_to_fit(&photo.image, aim, |img| {
                if photo.ext == "png" {
                    going(cancel)?;
                    let bytes = photo.encode(img, PLAIN_QUALITY)?;
                    let n = bytes.len() as u64;
                    Ok(if n <= aim {
                        Trial::Fits(bytes)
                    } else {
                        Trial::Over(n)
                    })
                } else {
                    highest_fitting(aim, |q| {
                        going(cancel)?;
                        photo.encode(img, q)
                    })
                }
            });
            let bytes = match shrunk {
                // Under the size, but not under the original's.
                Err(Error::OutOfReach { .. }) if aim < max => {
                    return Err(Error::NothingToDo(
                        "This file is already as small as Kiln can make it.".to_owned(),
                    ))
                }
                other => other?,
            };
            // Never a file over the size asked, or one larger than the
            // original.
            let n = bytes.len() as u64;
            if n > max || n >= original {
                return Err(Error::OutOfReach {
                    max_bytes: max,
                    smallest: n,
                });
            }
            bytes
        }
    };
    going(cancel)?;
    Ok((bytes, photo.ext))
}

/// The photo at `input`, of type `ext`, with its long side brought down to
/// `opts.tool.long_side` and saved again at the Photos quality, written in
/// the job's folder `work`. It is turned upright and keeps its colour
/// profile, and its metadata unless Remove location data is set; a HEIC or
/// AVIF becomes a JPG. Returns the file, its type and the long side.
pub(crate) fn resize(
    input: &Path,
    ext: &str,
    opts: &Options,
    work: &Path,
    cancel: &CancelToken,
) -> Result<(PathBuf, String, u32)> {
    let Some(side) = opts.tool.long_side.filter(|&side| side > 0) else {
        return Err(Error::ToolUnavailable(
            "Resize needs a size for the long side.".to_owned(),
        ));
    };
    if moving(input, ext) {
        return Err(Error::ToolUnavailable(format!(
            "Kiln can't {}.",
            Tool::Resize.phrase(Kind::Image, "animated pictures")
        )));
    }
    let photo = Prepared::read(
        input,
        ext,
        None,
        opts.tool.remove_location,
        opts,
        Some(work),
        cancel,
    )?;
    let long = photo.image.width().max(photo.image.height());
    if long <= side {
        return Err(Error::NothingToDo(format!(
            "This picture's long side is already {long} px."
        )));
    }
    going(cancel)?;
    let img = scaled(&photo.image, side);
    let tmp = if matches!(photo.ext.as_str(), "jpg" | "png" | "webp") {
        let bytes = photo.encode(&img, opts.jpeg_quality)?;
        write_out(work, &photo.ext, &bytes)?
    } else {
        let tmp = work.join(format!("out.{}", photo.ext));
        raster::encode(
            &img,
            &tmp,
            &photo.ext,
            opts.jpeg_quality,
            photo.icc.as_deref(),
        )?;
        tmp
    };
    Ok((tmp, photo.ext, side))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quality_search_takes_the_highest_that_fits_in_six_encodes_at_most() {
        for max in [
            0, 39_000, 40_000, 40_500, 66_000, 70_500, 91_999, 92_000, 200_000,
        ] {
            let mut tried = vec![];
            let found = highest_fitting(max, |q| {
                tried.push(q);
                Ok(vec![0; usize::from(q) * 1_000])
            })
            .unwrap();
            assert!(tried.len() <= 6, "{max}: {tried:?}");
            assert!(tried
                .iter()
                .all(|q| (QUALITY_LOW..=QUALITY_HIGH).contains(q)));
            let best = (QUALITY_LOW..=QUALITY_HIGH)
                .rev()
                .find(|&q| u64::from(q) * 1_000 <= max);
            match best {
                Some(q) => assert_eq!(found, Trial::Fits(vec![0; usize::from(q) * 1_000])),
                None => {
                    assert_eq!(found, Trial::Over(40_000), "{max}");
                    assert_eq!(tried.last(), Some(&QUALITY_LOW));
                }
            }
        }
    }

    #[test]
    fn a_result_is_never_written_over_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_out(dir.path(), "jpg", b"first").unwrap();
        assert!(write_out(dir.path(), "jpg", b"second").is_err());
        assert_eq!(fs::read(first).unwrap(), b"first");
    }

    #[test]
    fn a_photo_is_scaled_down_until_it_fits_three_times_at_most() {
        let base = DynamicImage::new_rgb8(400, 300);
        // One byte a pixel, and a header of 100.
        let bytes = |img: &DynamicImage| u64::from(img.width() * img.height()) + 100;
        let mut sizes = vec![];
        let fits = shrink_to_fit(&base, 30_000, |img| {
            sizes.push((img.width(), img.height()));
            let n = bytes(img);
            Ok(if n <= 30_000 {
                Trial::Fits(vec![0; n as usize])
            } else {
                Trial::Over(n)
            })
        })
        .unwrap();
        assert!(
            fits.len() <= 30_000 && fits.len() > 20_000,
            "{}",
            fits.len()
        );
        assert_eq!(sizes[0], (400, 300));
        assert!(sizes.len() <= 4, "{sizes:?}");
        // Out of reach: the header alone is too big.
        let mut tries = 0;
        let err = shrink_to_fit(&base, 50, |img| {
            tries += 1;
            Ok(Trial::Over(bytes(img)))
        })
        .unwrap_err();
        assert!(tries <= 4, "{tries}");
        // Always a little over: tried as it is and three times smaller.
        let mut sizes = vec![];
        shrink_to_fit(&base, 50, |img| {
            sizes.push(img.width());
            Ok(Trial::Over(60))
        })
        .unwrap_err();
        assert_eq!(sizes.len(), 4, "{sizes:?}");
        assert!(sizes.windows(2).all(|w| w[1] < w[0]), "{sizes:?}");
        match err {
            Error::OutOfReach {
                max_bytes: 50,
                smallest,
            } => assert!(smallest > 100 && smallest < 120_100, "{smallest}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn scaling_never_enlarges() {
        let img = DynamicImage::new_rgb8(40, 20);
        assert_eq!(scaled(&img, 100).width(), 40);
        let s = scaled(&img, 10);
        assert_eq!((s.width(), s.height()), (10, 5));
    }
}
