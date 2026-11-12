//! WebP written by libwebp, linked in from its C source. The `image` crate
//! reads WebP and writes it only lossless; this writes the lossy kind a
//! quality setting means, keeping transparency and the colour profile, and
//! animations frame by frame.

use std::ffi::{c_int, c_void, CStr};
use std::ptr;

use image::{DynamicImage, RgbaImage};
use libwebp_sys as ffi;

use crate::error::{Error, Result};

/// The most pixels a WebP can have along either side.
const MAX_SIDE: u32 = 16383;

fn fail(reason: impl Into<String>) -> Error {
    Error::Encode {
        code: "WEBP".to_owned(),
        reason: reason.into(),
    }
}

/// A `WebPPicture` that frees what libwebp gave it when it goes out of scope.
struct Picture(ffi::WebPPicture);

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: the picture was made by `WebPPicture::new`, and freeing it
        // is allowed whether or not anything was allocated for it.
        unsafe { ffi::WebPPictureFree(&mut self.0) }
    }
}

/// A `WebPMemoryWriter`, cleared when it goes out of scope.
struct Writer(ffi::WebPMemoryWriter);

impl Drop for Writer {
    fn drop(&mut self) {
        // SAFETY: the writer was set up by `WebPMemoryWriterInit`, and
        // clearing it frees the buffer it grew, if any.
        unsafe { ffi::WebPMemoryWriterClear(&mut self.0) }
    }
}

/// A `WebPMux`, deleted when it goes out of scope.
struct Mux(*mut ffi::WebPMux);

impl Drop for Mux {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a successful create and is deleted
        // only here.
        unsafe { ffi::WebPMuxDelete(self.0) }
    }
}

/// A `WebPAnimEncoder`, deleted when it goes out of scope.
struct AnimEncoder(*mut ffi::WebPAnimEncoder);

impl Drop for AnimEncoder {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a successful create and is deleted
        // only here.
        unsafe { ffi::WebPAnimEncoderDelete(self.0) }
    }
}

/// A `WebPData` that libwebp allocated, cleared when it goes out of scope.
struct Data(ffi::WebPData);

impl Drop for Data {
    fn drop(&mut self) {
        // SAFETY: the data is empty or was made by libwebp, and clearing
        // empty data is allowed.
        unsafe { ffi::WebPDataClear(&mut self.0) }
    }
}

/// The error for a picture of `width` x `height`, if WebP can't hold one
/// that size.
fn check_size(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(fail(format!(
            "WebP pictures are at most {MAX_SIDE} pixels on a side, and this one is {width} x {height}."
        )));
    }
    Ok(())
}

/// The encoder settings for a lossy picture at `quality`. The alpha plane is
/// not thinned, and the colour under clear pixels is free to change.
fn lossy_config(quality: u8) -> Result<ffi::WebPConfig> {
    let mut config = ffi::WebPConfig::new().map_err(|()| fail("the encoder did not start."))?;
    config.quality = f32::from(quality);
    config.alpha_quality = 100;
    config.exact = 0;
    // SAFETY: `config` was filled in by libwebp.
    if unsafe { ffi::WebPValidateConfig(&config) } == 0 {
        return Err(fail(format!("quality {quality} is not one it can use.")));
    }
    Ok(config)
}

/// `img` as a lossy WebP at `quality`, 1 to 100. Transparency is kept
/// exactly (the alpha plane is not thinned), and the colour under clear
/// pixels is free to change, as it is not seen. WebP holds colour only, so
/// a gray picture is written as RGB. The colour profile `icc`, if given, is
/// stored in an `ICCP` chunk; the caller has already checked it suits an
/// RGB picture.
pub(crate) fn encode_lossy(img: &DynamicImage, quality: u8, icc: Option<&[u8]>) -> Result<Vec<u8>> {
    let (width, height) = (img.width(), img.height());
    check_size(width, height)?;
    let config = lossy_config(quality)?;

    let mut picture =
        Picture(ffi::WebPPicture::new().map_err(|()| fail("the encoder did not start."))?);
    picture.0.width = width as c_int;
    picture.0.height = height as c_int;
    // The import copies the pixels, so these only have to outlive it.
    let imported = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        // SAFETY: `rgba` holds `height` rows of `width` * 4 bytes, and the
        // picture has its size set.
        unsafe { ffi::WebPPictureImportRGBA(&mut picture.0, rgba.as_ptr(), (width * 4) as c_int) }
    } else {
        let rgb = img.to_rgb8();
        // SAFETY: `rgb` holds `height` rows of `width` * 3 bytes, and the
        // picture has its size set.
        unsafe { ffi::WebPPictureImportRGB(&mut picture.0, rgb.as_ptr(), (width * 3) as c_int) }
    };
    if imported == 0 {
        return Err(fail(explain(picture.0.error_code)));
    }

    let mut writer = Writer(ffi::WebPMemoryWriter {
        mem: ptr::null_mut(),
        size: 0,
        max_size: 0,
        pad: [0],
    });
    // SAFETY: the writer is a plain struct in place on the stack, which it
    // stays until the picture is done with.
    unsafe { ffi::WebPMemoryWriterInit(&mut writer.0) };
    picture.0.writer = Some(ffi::WebPMemoryWrite);
    picture.0.custom_ptr = ptr::from_mut(&mut writer.0).cast::<c_void>();
    // SAFETY: the config is valid, the picture holds pixels, and its writer
    // is `WebPMemoryWrite` with a `WebPMemoryWriter` behind `custom_ptr`.
    if unsafe { ffi::WebPEncode(&config, &mut picture.0) } == 0 {
        return Err(fail(explain(picture.0.error_code)));
    }
    // SAFETY: after a successful encode the writer holds `size` bytes at
    // `mem`.
    let encoded = unsafe { std::slice::from_raw_parts(writer.0.mem, writer.0.size) };
    match icc {
        Some(profile) => with_profile(encoded, profile),
        None => Ok(encoded.to_vec()),
    }
}

/// The WebP file `webp` with the colour profile `profile` added, as an
/// `ICCP` chunk. The file becomes the extended kind that can hold one.
fn with_profile(webp: &[u8], profile: &[u8]) -> Result<Vec<u8>> {
    with_chunks(webp, &[(c"ICCP", Some(profile))])
        .map_err(|_| fail("it could not add the colour profile."))
}

/// The WebP file `webp` with each chunk named in `chunks` (`ICCP`, `EXIF`
/// or `XMP `) set to the bytes given; `None` leaves that chunk alone. The
/// file becomes the extended kind that can hold them, with its flags set to
/// match, and its picture is copied as it was.
pub(crate) fn with_chunks(webp: &[u8], chunks: &[(&CStr, Option<&[u8]>)]) -> Result<Vec<u8>> {
    let file = ffi::WebPData {
        bytes: webp.as_ptr(),
        size: webp.len(),
    };
    // SAFETY: `file` points at `webp`, which is copied, and the ABI
    // version is the one the bindings were made for.
    let mux = Mux(unsafe { ffi::WebPMuxCreateInternal(&file, 1, ffi::WebPGetMuxABIVersion()) });
    if mux.0.is_null() {
        return Err(fail("it could not read the picture it made."));
    }
    for (name, bytes) in chunks {
        let Some(bytes) = bytes else { continue };
        let chunk = ffi::WebPData {
            bytes: bytes.as_ptr(),
            size: bytes.len(),
        };
        // SAFETY: the mux is live, the name is four characters, and the
        // chunk points at `bytes`, which is copied.
        let set = unsafe { ffi::WebPMuxSetChunk(mux.0, name.as_ptr(), &chunk, 1) };
        if set != ffi::WebPMuxError::WEBP_MUX_OK {
            return Err(fail("it could not add the picture's details."));
        }
    }
    let mut assembled = ffi::WebPData::default();
    // SAFETY: the mux is live and `assembled` is empty.
    let built = unsafe { ffi::WebPMuxAssemble(mux.0, &mut assembled) };
    // SAFETY: on success `assembled` holds `size` bytes at `bytes`, which
    // stay until they are cleared.
    let result = if built == ffi::WebPMuxError::WEBP_MUX_OK {
        Ok(unsafe { std::slice::from_raw_parts(assembled.bytes, assembled.size) }.to_vec())
    } else {
        Err(fail("it could not add the picture's details."))
    };
    // SAFETY: `assembled` is empty or was made by the assemble above.
    unsafe { ffi::WebPDataClear(&mut assembled) };
    result
}

/// An animation encoder for a canvas of `width` x `height` that plays
/// `loops` times, or for ever when that is 0.
fn new_encoder(width: u32, height: u32, loops: u16) -> Result<AnimEncoder> {
    let mut options = ffi::WebPAnimEncoderOptions {
        anim_params: ffi::WebPMuxAnimParams {
            bgcolor: 0,
            loop_count: 0,
        },
        minimize_size: 0,
        kmin: 0,
        kmax: 0,
        allow_mixed: 0,
        verbose: 0,
        padding: [0; 4],
    };
    // SAFETY: `options` is a plain struct, which libwebp fills in; the ABI
    // version is the one the bindings were made for.
    if unsafe { ffi::WebPAnimEncoderOptionsInitInternal(&mut options, ffi::WebPGetMuxABIVersion()) }
        == 0
    {
        return Err(fail("the encoder did not start."));
    }
    options.anim_params.loop_count = c_int::from(loops);
    // The canvas starts clear, as a GIF's does.
    options.anim_params.bgcolor = 0;
    // SAFETY: the options were set up above, and the caller has checked the
    // size.
    let encoder = AnimEncoder(unsafe {
        ffi::WebPAnimEncoderNewInternal(
            width as c_int,
            height as c_int,
            &options,
            ffi::WebPGetMuxABIVersion(),
        )
    });
    if encoder.0.is_null() {
        return Err(fail("the encoder did not start."));
    }
    Ok(encoder)
}

/// An animated lossy WebP of `frames` at `quality`, 1 to 100, each a picture
/// of `size` shown for its delay in ms. The animation plays `loops` times,
/// or for ever when that is 0. Frames are taken one at a time, so only the
/// one being added is held here; libwebp keeps what it has encoded until the
/// file is put together. Transparency is kept as in [`encode_lossy`].
pub(crate) fn encode_animation(
    frames: impl Iterator<Item = Result<(RgbaImage, u32)>>,
    size: (u32, u32),
    loops: u16,
    quality: u8,
) -> Result<Vec<u8>> {
    let (width, height) = size;
    check_size(width, height)?;
    let config = lossy_config(quality)?;

    let encoder = new_encoder(width, height, loops)?;

    let mut timestamp: c_int = 0;
    let mut count = 0u32;
    for frame in frames {
        let (pixels, delay) = frame?;
        if pixels.dimensions() != size {
            return Err(fail("a frame is not the size of the picture."));
        }
        let mut picture =
            Picture(ffi::WebPPicture::new().map_err(|()| fail("the encoder did not start."))?);
        picture.0.width = width as c_int;
        picture.0.height = height as c_int;
        // The animation encoder works on ARGB; asking for it here saves it
        // converting a frame from YUV, which loses some colour.
        picture.0.use_argb = 1;
        // SAFETY: `pixels` holds `height` rows of `width` * 4 bytes, and the
        // picture has its size set; the import copies them.
        let imported = unsafe {
            ffi::WebPPictureImportRGBA(&mut picture.0, pixels.as_ptr(), (width * 4) as c_int)
        };
        if imported == 0 {
            return Err(fail(explain(picture.0.error_code)));
        }
        drop(pixels);
        // SAFETY: the encoder is live, the picture is ARGB and the canvas's
        // size, and the config is valid.
        let added =
            unsafe { ffi::WebPAnimEncoderAdd(encoder.0, &mut picture.0, timestamp, &config) };
        if added == 0 {
            let said = match picture.0.error_code {
                ffi::WebPEncodingError::VP8_ENC_OK => "a frame could not be written.".to_owned(),
                code => explain(code),
            };
            return Err(fail(with_its_message(&encoder, said)));
        }
        timestamp = c_int::try_from(delay)
            .ok()
            .and_then(|d| timestamp.checked_add(d))
            .ok_or_else(|| fail("the animation is too long for a WebP."))?;
        count += 1;
    }
    if count == 0 {
        return Err(fail("there are no frames to write."));
    }

    // A frame lasts until the next one starts, so the last frame's delay is
    // given by a final call with no picture, timed at the end.
    // SAFETY: the encoder is live; no picture and no config is what ends it.
    if unsafe { ffi::WebPAnimEncoderAdd(encoder.0, ptr::null_mut(), timestamp, ptr::null()) } == 0 {
        return Err(fail(with_its_message(
            &encoder,
            "the animation could not be finished.".to_owned(),
        )));
    }
    let mut assembled = Data(ffi::WebPData::default());
    // SAFETY: the encoder is live and `assembled` is empty.
    if unsafe { ffi::WebPAnimEncoderAssemble(encoder.0, &mut assembled.0) } == 0 {
        return Err(fail(with_its_message(
            &encoder,
            "the animation could not be put together.".to_owned(),
        )));
    }
    // SAFETY: after a successful assemble `assembled` holds `size` bytes at
    // `bytes`, which stay until it is cleared.
    Ok(unsafe { std::slice::from_raw_parts(assembled.0.bytes, assembled.0.size) }.to_vec())
}

/// `said`, a sentence about what failed, followed by what the animation
/// encoder itself reported for its last call, if it reported anything.
fn with_its_message(encoder: &AnimEncoder, said: String) -> String {
    // SAFETY: the encoder is live. The string it returns is its own and
    // stays until its next call; it is copied here before any other.
    let reported = unsafe {
        let text = ffi::WebPAnimEncoderGetError(encoder.0);
        if text.is_null() {
            String::new()
        } else {
            CStr::from_ptr(text).to_string_lossy().into_owned()
        }
    };
    let reported = reported.trim().trim_end_matches('.');
    if reported.is_empty() {
        said
    } else {
        format!("{said} libwebp said: {reported}.")
    }
}

/// What libwebp's error code means, in a few words.
fn explain(code: ffi::WebPEncodingError) -> String {
    use ffi::WebPEncodingError as E;
    match code {
        E::VP8_ENC_ERROR_OUT_OF_MEMORY | E::VP8_ENC_ERROR_BITSTREAM_OUT_OF_MEMORY => {
            "there was not enough memory.".to_owned()
        }
        E::VP8_ENC_ERROR_BAD_DIMENSION => "the picture is too large for WebP.".to_owned(),
        E::VP8_ENC_ERROR_PARTITION0_OVERFLOW | E::VP8_ENC_ERROR_PARTITION_OVERFLOW => {
            "the picture is too detailed to fit a WebP at this quality.".to_owned()
        }
        E::VP8_ENC_ERROR_FILE_TOO_BIG => "the file would be too big.".to_owned(),
        other => format!("the encoder stopped with error {}.", other as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::webp::WebPDecoder;
    use image::metadata::LoopCount;
    use image::{AnimationDecoder, ImageFormat, Rgba};

    fn chunk_names(bytes: &[u8]) -> Vec<[u8; 4]> {
        let mut names = Vec::new();
        let mut at = 12;
        while at + 8 <= bytes.len() {
            names.push(bytes[at..at + 4].try_into().unwrap());
            let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            at += 8 + len + (len & 1);
        }
        names
    }

    #[test]
    fn a_picture_with_no_transparency_has_no_alpha_chunk() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([200, 30, 30, 255])));
        let webp = encode_lossy(&img, 90, None).unwrap();
        assert_eq!(chunk_names(&webp), [*b"VP8 "]);
        let back = image::load_from_memory_with_format(&webp, ImageFormat::WebP).unwrap();
        assert_eq!((back.width(), back.height()), (8, 8));
    }

    #[test]
    fn a_profile_makes_the_file_the_extended_kind() {
        let img = DynamicImage::new_rgb8(8, 8);
        let mut profile = vec![0u8; 128];
        profile[16..20].copy_from_slice(b"RGB ");
        let webp = encode_lossy(&img, 90, Some(&profile)).unwrap();
        assert_eq!(chunk_names(&webp), [*b"VP8X", *b"ICCP", *b"VP8 "]);
    }

    #[test]
    fn a_gray_picture_is_written_as_colour() {
        let img = DynamicImage::new_luma8(8, 8);
        let webp = encode_lossy(&img, 75, None).unwrap();
        let back = image::load_from_memory_with_format(&webp, ImageFormat::WebP).unwrap();
        assert_eq!(back.color(), image::ColorType::Rgb8);
    }

    fn flat(side: u32, grey: u8) -> RgbaImage {
        RgbaImage::from_pixel(side, side, Rgba([grey, grey, grey, 255]))
    }

    #[test]
    fn an_animation_holds_its_frames_with_their_delays_and_loops() {
        let frames = [(flat(8, 0), 100), (flat(8, 128), 250), (flat(8, 255), 40)];
        let webp = encode_animation(frames.into_iter().map(Ok), (8, 8), 5, 80).unwrap();
        assert_eq!(chunk_names(&webp)[..2], [*b"VP8X", *b"ANIM"]);
        let decoder = WebPDecoder::new(std::io::Cursor::new(&webp)).unwrap();
        assert!(decoder.has_animation());
        assert!(matches!(decoder.loop_count(), LoopCount::Finite(n) if n.get() == 5));
        let shown: Vec<_> = decoder
            .into_frames()
            .map(|f| std::time::Duration::from(f.unwrap().delay()).as_millis())
            .collect();
        assert_eq!(shown, [100, 250, 40]);
    }

    #[test]
    fn what_the_encoder_reports_follows_the_sentence() {
        let encoder = new_encoder(8, 8, 0).unwrap();
        // Before any call it has nothing to say.
        assert_eq!(
            with_its_message(&encoder, "a frame failed.".to_owned()),
            "a frame failed."
        );
        // A frame the size of no canvas makes libwebp refuse it and say why.
        let mut picture = Picture(ffi::WebPPicture::new().unwrap());
        picture.0.use_argb = 1;
        picture.0.width = 4;
        picture.0.height = 4;
        let pixels = [0u8; 4 * 4 * 4];
        // SAFETY: `pixels` holds 4 rows of 4 * 4 bytes, and the picture is 4 x 4.
        assert_ne!(
            unsafe { ffi::WebPPictureImportRGBA(&mut picture.0, pixels.as_ptr(), 16) },
            0
        );
        // SAFETY: the encoder is live and the picture holds pixels.
        let added = unsafe { ffi::WebPAnimEncoderAdd(encoder.0, &mut picture.0, 0, ptr::null()) };
        assert_eq!(added, 0);
        let said = with_its_message(&encoder, "a frame failed.".to_owned());
        assert!(said.starts_with("a frame failed. libwebp said: "), "{said}");
        assert!(said.contains("Invalid frame dimensions"), "{said}");
        assert!(said.ends_with('.') && !said.ends_with(".."), "{said}");
    }

    #[test]
    fn an_animation_with_no_frames_is_refused() {
        let err = encode_animation(std::iter::empty(), (8, 8), 0, 80).unwrap_err();
        assert!(matches!(err, Error::Encode { ref code, .. } if code == "WEBP"));
    }

    #[test]
    fn a_frame_of_another_size_is_refused() {
        let frames = [(flat(8, 0), 100), (flat(4, 0), 100)];
        let err = encode_animation(frames.into_iter().map(Ok), (8, 8), 0, 80).unwrap_err();
        assert!(err.to_string().contains("size"), "{err}");
    }

    #[test]
    fn a_frame_that_fails_to_read_stops_the_animation() {
        let frames = [Ok((flat(8, 0), 100)), Err(Error::Cancelled)];
        let err = encode_animation(frames.into_iter(), (8, 8), 0, 80).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }

    #[test]
    fn an_animation_past_the_size_limit_is_refused() {
        let err = encode_animation(std::iter::empty(), (MAX_SIDE + 1, 1), 0, 80).unwrap_err();
        assert!(err.to_string().contains("16383"), "{err}");
    }

    #[test]
    fn a_side_past_the_limit_is_refused() {
        let img = DynamicImage::new_rgb8(MAX_SIDE + 1, 1);
        let err = encode_lossy(&img, 75, None).unwrap_err();
        assert!(matches!(err, Error::Encode { ref code, .. } if code == "WEBP"));
        assert!(err.to_string().contains("16383"), "{err}");
        // The limit itself is fine.
        let widest = DynamicImage::new_rgb8(MAX_SIDE, 1);
        assert!(encode_lossy(&widest, 75, None).is_ok());
    }
}
