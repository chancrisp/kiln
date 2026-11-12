//! Pictures made in the tests: stored sideways with an EXIF orientation,
//! and carrying a colour profile. Included with `mod pictures;`.
#![allow(dead_code)]

use std::fs;
use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::tiff::TiffDecoder;
use image::{
    ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Rgb, RgbImage,
};

pub const RED: [u8; 3] = [255, 0, 0];
pub const BLUE: [u8; 3] = [0, 0, 255];
pub const GREEN: [u8; 3] = [0, 255, 0];

/// A minimal EXIF block: a little-endian TIFF header and one entry, tag
/// 0x0112 (orientation) of type SHORT.
pub fn exif_orientation(orientation: u16) -> Vec<u8> {
    let mut blob = b"II*\0".to_vec();
    blob.extend(8u32.to_le_bytes()); // the first directory follows the header
    blob.extend(1u16.to_le_bytes()); // one entry
    blob.extend(0x0112u16.to_le_bytes());
    blob.extend(3u16.to_le_bytes()); // SHORT
    blob.extend(1u32.to_le_bytes()); // one value
    blob.extend(orientation.to_le_bytes());
    blob.extend(0u16.to_le_bytes()); // padding to four bytes
    blob.extend(0u32.to_le_bytes()); // no next directory
    blob
}

/// `w` x `h`, red in the left half and blue in the right.
pub fn left_red(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |x, _| Rgb(if x < w / 2 { RED } else { BLUE }))
}

/// [`left_red`] with the top-left pixel green, so that orientations that
/// differ by a flip, such as 1 and 4, put it in different corners.
pub fn corner_marked(w: u32, h: u32) -> RgbImage {
    let mut img = left_red(w, h);
    img.put_pixel(0, 0, Rgb(GREEN));
    img
}

/// A JPEG of `img` as it is stored, with EXIF orientation `orientation`
/// and the colour profile `icc` when given.
pub fn jpeg_with(path: &Path, img: &RgbImage, orientation: u16, icc: Option<&[u8]>, quality: u8) {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
    encoder
        .set_exif_metadata(exif_orientation(orientation))
        .unwrap();
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc.to_vec()).unwrap();
    }
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    fs::write(path, bytes).unwrap();
}

/// A PNG of `img` as it is stored, left uncompressed so a later re-encode
/// is smaller, with an `eXIf` chunk holding `orientation` and the colour
/// profile `icc` when given.
pub fn png_with(path: &Path, img: &RgbImage, orientation: u16, icc: Option<&[u8]>) {
    let mut bytes = Vec::new();
    let mut encoder = PngEncoder::new_with_quality(
        &mut bytes,
        CompressionType::Uncompressed,
        FilterType::NoFilter,
    );
    encoder
        .set_exif_metadata(exif_orientation(orientation))
        .unwrap();
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc.to_vec()).unwrap();
    }
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    fs::write(path, bytes).unwrap();
}

/// A real RGB profile: Display P3.
pub fn display_p3() -> Vec<u8> {
    moxcms::ColorProfile::new_display_p3().encode().unwrap()
}

/// The decoder for the picture at `path`, as the image crate makes it. A TIFF
/// decoder is made on its own: once the reader has set limits on it, it can't
/// read a field larger than the pixels, and a test picture's profile is.
fn decoder_of(path: &Path) -> Box<dyn ImageDecoder> {
    let bytes = fs::read(path).unwrap();
    let format = image::guess_format(&bytes).unwrap();
    let reader = Cursor::new(bytes);
    if format == ImageFormat::Tiff {
        Box::new(TiffDecoder::new(reader).unwrap())
    } else {
        Box::new(
            ImageReader::with_format(reader, format)
                .into_decoder()
                .unwrap(),
        )
    }
}

/// The colour profile stored in the picture at `path`, as its own decoder
/// reports it.
pub fn icc_of(path: &Path) -> Option<Vec<u8>> {
    decoder_of(path).icc_profile().unwrap()
}

/// The EXIF orientation stored in the picture at `path`.
pub fn orientation_of(path: &Path) -> image::metadata::Orientation {
    decoder_of(path).orientation().unwrap()
}

/// Whether `pixel` is within `tolerance` of `want` in every channel.
pub fn near(pixel: [u8; 3], want: [u8; 3], tolerance: u8) -> bool {
    pixel
        .iter()
        .zip(want)
        .all(|(a, b)| a.abs_diff(b) <= tolerance)
}

/// `tests/fixtures/sample.heic` (the 64 x 48 gradient) wrapped again with
/// HEIF properties of its own: `irot`, the turn a viewer applies
/// (anticlockwise, in quarters), and a `colr` box holding the colour
/// profile `icc`. The HEVC picture is the fixture's, byte for byte; only
/// the boxes around it are new, laid out as `gen_fixtures` lays them.
pub fn heic_with(sample: &[u8], quarters: Option<u8>, icc: Option<&[u8]>) -> Vec<u8> {
    heif(&Heif {
        quarters,
        icc,
        ..Heif::new(sample)
    })
}

/// A HEIC or AVIF made from a fixture's single picture.
pub struct Heif<'a> {
    /// `sample.heic` or `sample.avif`: its picture and decoder setup are
    /// used byte for byte.
    pub sample: &'a [u8],
    /// The `irot` turn on the main picture, anticlockwise in quarters.
    pub quarters: Option<u8>,
    /// A `colr` profile on the main picture.
    pub icc: Option<&'a [u8]>,
    /// An Exif item for the main picture holding this orientation, as
    /// phones write beside `irot`.
    pub exif_orientation: Option<u16>,
    /// The main picture as a grid of two copies of the sample side by
    /// side (128 x 48), as phones store their photos.
    pub grid: bool,
}

impl<'a> Heif<'a> {
    pub fn new(sample: &'a [u8]) -> Self {
        Heif {
            sample,
            quarters: None,
            icc: None,
            exif_orientation: None,
            grid: false,
        }
    }
}

fn boxed(tag: &[u8], body: &[u8]) -> Vec<u8> {
    let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(tag);
    b.extend_from_slice(body);
    b
}

fn full(tag: &[u8], version: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version, 0, 0, 0];
    b.extend_from_slice(body);
    boxed(tag, &b)
}

/// The HEIF file `spec` describes.
pub fn heif(spec: &Heif) -> Vec<u8> {
    let sample = spec.sample;
    let find = |tag: &[u8]| {
        let at = sample.windows(4).position(|w| w == tag)? - 4;
        let size = u32::from_be_bytes(sample[at..at + 4].try_into().unwrap()) as usize;
        Some(sample[at..at + size].to_vec())
    };
    let avif = find(b"av1C").is_some();
    let picture = find(b"mdat").unwrap()[8..].to_vec();
    // What a tile needs to be decoded, the essential decoder setup first.
    let mut setup = vec![];
    let (ftyp, kind) = if avif {
        setup.push(find(b"av1C").unwrap());
        setup.extend(find(b"pixi"));
        setup.extend(find(b"colr"));
        (boxed(b"ftyp", b"avif\0\0\0\0mif1avifmiaf"), b"av01")
    } else {
        setup.push(find(b"hvcC").unwrap());
        (boxed(b"ftyp", b"heic\0\0\0\0mif1heic"), b"hvc1")
    };
    let ispe = |w: u32, h: u32| {
        let mut b = w.to_be_bytes().to_vec();
        b.extend_from_slice(&h.to_be_bytes());
        full(b"ispe", 0, &b)
    };

    // Items as (id, type, data); the main picture is item 1. Properties
    // are numbered from 1, and each item links to some of them.
    let mut items: Vec<(u16, &[u8; 4], Vec<u8>)> = vec![];
    let mut properties: Vec<Vec<u8>> = setup;
    properties.push(ispe(64, 48));
    let mut tile = vec![0x81u8];
    tile.extend(2..=properties.len() as u8);
    let mut links: Vec<(u16, Vec<u8>)> = vec![];
    let mut refs: Vec<u8> = vec![];
    let mut main = if spec.grid {
        // ImageGrid: version, flags (16-bit sizes), one row, two columns,
        // and the whole picture's size.
        let mut grid = vec![0u8, 0, 0, 1];
        grid.extend_from_slice(&128u16.to_be_bytes());
        grid.extend_from_slice(&48u16.to_be_bytes());
        items.push((1, b"grid", grid));
        for id in [2u16, 3] {
            items.push((id, kind, picture.clone()));
            links.push((id, tile.clone()));
        }
        let mut dimg = 1u16.to_be_bytes().to_vec();
        for to in [2u16, 2, 3] {
            dimg.extend_from_slice(&to.to_be_bytes());
        }
        refs.extend(boxed(b"dimg", &dimg));
        properties.push(ispe(128, 48));
        vec![properties.len() as u8]
    } else {
        items.push((1, kind, picture.clone()));
        tile
    };
    if let Some(icc) = spec.icc {
        properties.push(boxed(b"colr", &[b"prof".as_slice(), icc].concat()));
        main.push(properties.len() as u8);
    }
    if let Some(q) = spec.quarters {
        properties.push(boxed(b"irot", &[q & 3]));
        main.push(0x80 | properties.len() as u8);
    }
    links.insert(0, (1, main));
    if let Some(o) = spec.exif_orientation {
        // The offset to the TIFF header, then "Exif\0\0", as phones write.
        let mut exif = 6u32.to_be_bytes().to_vec();
        exif.extend_from_slice(b"Exif\0\0");
        exif.extend(exif_orientation(o));
        let id = items.len() as u16 + 1;
        items.push((id, b"Exif", exif));
        let mut cdsc = id.to_be_bytes().to_vec();
        cdsc.extend_from_slice(&1u16.to_be_bytes());
        cdsc.extend_from_slice(&1u16.to_be_bytes());
        refs.extend(boxed(b"cdsc", &cdsc));
    }

    let hdlr = full(b"hdlr", 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
    let pitm = full(b"pitm", 0, &1u16.to_be_bytes());
    let mut iinf_body = (items.len() as u16).to_be_bytes().to_vec();
    for (id, kind, _) in &items {
        let mut infe = id.to_be_bytes().to_vec();
        infe.extend_from_slice(&[0, 0]);
        infe.extend_from_slice(*kind);
        infe.push(0);
        iinf_body.extend(full(b"infe", 2, &infe));
    }
    let iinf = full(b"iinf", 0, &iinf_body);
    let ipco = boxed(b"ipco", &properties.concat());
    let mut ipma_body = (links.len() as u32).to_be_bytes().to_vec();
    for (id, l) in &links {
        ipma_body.extend_from_slice(&id.to_be_bytes());
        ipma_body.push(l.len() as u8);
        ipma_body.extend_from_slice(l);
    }
    let ipma = full(b"ipma", 0, &ipma_body);
    let iprp = boxed(b"iprp", &[ipco, ipma].concat());
    let iref = if refs.is_empty() {
        vec![]
    } else {
        full(b"iref", 0, &refs)
    };
    let meta_for = |data_at: u32| {
        let mut iloc = vec![0x44, 0x00];
        iloc.extend_from_slice(&(items.len() as u16).to_be_bytes());
        let mut at = data_at;
        for (id, _, data) in &items {
            iloc.extend_from_slice(&id.to_be_bytes());
            iloc.extend_from_slice(&0u16.to_be_bytes());
            iloc.extend_from_slice(&1u16.to_be_bytes());
            iloc.extend_from_slice(&at.to_be_bytes());
            iloc.extend_from_slice(&(data.len() as u32).to_be_bytes());
            at += data.len() as u32;
        }
        let iloc = full(b"iloc", 0, &iloc);
        full(
            b"meta",
            0,
            &[&hdlr, &pitm, &iinf, &iref, &iprp, &iloc]
                .map(|b| b.as_slice())
                .concat(),
        )
    };
    let head = ftyp.len() + meta_for(0).len();
    let meta = meta_for((head + 8) as u32);
    let data: Vec<u8> = items.iter().flat_map(|(_, _, d)| d.clone()).collect();
    [ftyp, meta, boxed(b"mdat", &data)].concat()
}

/// A camera's Exif block, little-endian: make "Canon", model "EOS R5",
/// orientation `orientation`, a serial number "SN-0042" in the Exif
/// directory, and a GPS directory placing the photo at 51° 30' N.
pub fn camera_exif(orientation: u16) -> Vec<u8> {
    const ASCII: u16 = 2;
    const SHORT: u16 = 3;
    const LONG: u16 = 4;
    const RATIONAL: u16 = 5;
    let ifd = |n: u32| 2 + 12 * n + 4;
    let ifd0 = 8;
    let exif_ifd = ifd0 + ifd(5);
    let gps_ifd = exif_ifd + ifd(1);
    let values = gps_ifd + ifd(2);
    let make = values;
    let model = make + 6;
    let serial = model + 7;
    let latitude = serial + 8;

    let mut t = b"II*\0".to_vec();
    t.extend(ifd0.to_le_bytes());
    let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32| {
        t.extend(tag.to_le_bytes());
        t.extend(kind.to_le_bytes());
        t.extend(count.to_le_bytes());
        t.extend(value.to_le_bytes());
    };
    t.extend(5u16.to_le_bytes());
    entry(&mut t, 0x010F, ASCII, 6, make);
    entry(&mut t, 0x0110, ASCII, 7, model);
    entry(&mut t, 0x0112, SHORT, 1, u32::from(orientation));
    entry(&mut t, 0x8769, LONG, 1, exif_ifd);
    entry(&mut t, 0x8825, LONG, 1, gps_ifd);
    t.extend(0u32.to_le_bytes());
    t.extend(1u16.to_le_bytes());
    entry(&mut t, 0xA431, ASCII, 8, serial);
    t.extend(0u32.to_le_bytes());
    t.extend(2u16.to_le_bytes());
    entry(&mut t, 0x0001, ASCII, 2, u32::from(b'N'));
    entry(&mut t, 0x0002, RATIONAL, 3, latitude);
    t.extend(0u32.to_le_bytes());
    assert_eq!(t.len() as u32, values);
    t.extend_from_slice(b"Canon\0EOS R5\0SN-0042\0");
    for (n, d) in [(51u32, 1u32), (30, 1), (0, 1)] {
        t.extend(n.to_le_bytes());
        t.extend(d.to_le_bytes());
    }
    t
}

/// An XMP packet naming the camera and the place, and the orientation
/// `orientation`.
pub fn camera_xmp(orientation: u16) -> Vec<u8> {
    format!(
        "<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description xmlns:exif=\"http://ns.adobe.com/exif/1.0/\" xmlns:tiff=\"http://ns.adobe.com/tiff/1.0/\" \
         exif:GPSLatitude=\"51,30.0N\" tiff:Make=\"Canon\" tiff:Orientation=\"{orientation}\"/>\
         </rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
    )
    .into_bytes()
}

/// Words a photo's camera details and place leave in its bytes.
pub const CAMERA_WORDS: [&str; 6] = [
    "Canon",
    "EOS R5",
    "SN-0042",
    "xmpmeta",
    "GPSLatitude",
    "Hometown",
];

fn jpeg_segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut s = vec![0xFF, marker];
    s.extend(((payload.len() + 2) as u16).to_be_bytes());
    s.extend_from_slice(payload);
    s
}

/// A JPEG of `img`, sideways (orientation 6), with the colour profile
/// `icc`, a camera's Exif, XMP, IPTC naming the city "Hometown", and a
/// comment.
pub fn camera_jpeg(path: &Path, img: &RgbImage, icc: &[u8]) {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
    encoder.set_exif_metadata(camera_exif(6)).unwrap();
    encoder.set_icc_profile(icc.to_vec()).unwrap();
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    let extra = [
        jpeg_segment(
            0xE1,
            &[b"http://ns.adobe.com/xap/1.0/\0".as_slice(), &camera_xmp(6)].concat(),
        ),
        jpeg_segment(
            0xED,
            b"Photoshop 3.0\08BIM\x04\x04\0\0\0\0\0\x0E\x1C\x02\x5A\0\x08Hometown",
        ),
        jpeg_segment(0xFE, b"Hometown"),
    ]
    .concat();
    let with = [&bytes[..2], &extra, &bytes[2..]].concat();
    fs::write(path, with).unwrap();
}

fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    c.extend(crc.sum().to_be_bytes());
    c
}

/// A PNG of `img`, sideways (orientation 6), with the colour profile
/// `icc`, a camera's Exif in `eXIf`, XMP in `iTXt` and a `tEXt` comment.
pub fn camera_png(path: &Path, img: &RgbImage, icc: &[u8]) {
    let mut bytes = Vec::new();
    let mut encoder = PngEncoder::new(&mut bytes);
    encoder.set_exif_metadata(camera_exif(6)).unwrap();
    encoder.set_icc_profile(icc.to_vec()).unwrap();
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    let idat = bytes.windows(4).position(|w| w == b"IDAT").unwrap() - 4;
    let extra = [
        png_chunk(
            b"iTXt",
            &[b"XML:com.adobe.xmp\0\0\0\0\0".as_slice(), &camera_xmp(6)].concat(),
        ),
        png_chunk(b"tEXt", b"Comment\0Hometown"),
    ]
    .concat();
    fs::write(path, [&bytes[..idat], &extra, &bytes[idat..]].concat()).unwrap();
}

/// A lossless WEBP of `img`, sideways (orientation 6), with the colour
/// profile `icc`, a camera's Exif and XMP.
pub fn camera_webp(path: &Path, img: &RgbImage, icc: &[u8]) {
    use image::codecs::webp::WebPEncoder;
    let mut bytes = Vec::new();
    let mut encoder = WebPEncoder::new_lossless(&mut bytes);
    encoder.set_exif_metadata(camera_exif(6)).unwrap();
    encoder.set_icc_profile(icc.to_vec()).unwrap();
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    assert_eq!(
        &bytes[12..16],
        b"VP8X",
        "the encoder writes the extended kind"
    );
    let xmp = camera_xmp(6);
    bytes.extend_from_slice(b"XMP ");
    bytes.extend((xmp.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&xmp);
    if xmp.len() % 2 == 1 {
        bytes.push(0);
    }
    bytes[20] |= 0x04; // the XMP flag
    let riff = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

/// The Exif block of the picture at `path`, as its own decoder reports it.
pub fn exif_of(path: &Path) -> Option<Vec<u8>> {
    decoder_of(path).exif_metadata().unwrap()
}

/// The XMP packet of the picture at `path`, as its own decoder reports it.
pub fn xmp_of(path: &Path) -> Option<Vec<u8>> {
    decoder_of(path).xmp_metadata().unwrap()
}

/// The tags in the first directory of the Exif block `exif`.
pub fn ifd0_tags(exif: &[u8]) -> Vec<u16> {
    let tiff = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let le = tiff.starts_with(b"II");
    let u16_at = |i: usize| {
        let b = [tiff[i], tiff[i + 1]];
        if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    };
    let u32_at = |i: usize| {
        let b = [tiff[i], tiff[i + 1], tiff[i + 2], tiff[i + 3]];
        if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    let at = u32_at(4) as usize;
    (0..u16_at(at) as usize)
        .map(|n| u16_at(at + 2 + 12 * n))
        .collect()
}

/// A JPEG's coded picture: from its first scan to its end.
pub fn jpeg_scan(bytes: &[u8]) -> &[u8] {
    let at = bytes.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let end = bytes.windows(2).rposition(|w| w == [0xFF, 0xD9]).unwrap();
    &bytes[at..end + 2]
}
