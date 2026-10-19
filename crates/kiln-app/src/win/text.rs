//! DirectWrite: the bundled display serif, system UI and mono faces, and
//! the text formats the app draws with.

use windows::core::{Interface, Result, BOOL, HSTRING};
use windows::Win32::Graphics::DirectWrite::*;

use crate::theme::{fonts, type_scale};

pub struct Text {
    pub factory: IDWriteFactory,
    pub display: IDWriteTextFormat,
    pub display_center: IDWriteTextFormat,
    /// Wrapping monospace for engine output in the details dialog.
    pub mono_block: IDWriteTextFormat,
    pub lead_center: IDWriteTextFormat,
    pub body: IDWriteTextFormat,
    pub body_center: IDWriteTextFormat,
    pub caption: IDWriteTextFormat,
    pub caption_center: IDWriteTextFormat,
    pub code: IDWriteTextFormat,
    /// True if the bundled serif loaded; false means the fallback is used.
    pub display_bundled: bool,
}

impl Text {
    pub fn new() -> Result<Self> {
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };

        let bundled = bundled_collection(&factory)
            .ok()
            .and_then(|c| family_name(&c).map(|name| (c, name)));
        let (display_collection, display_family) = match bundled {
            Some((c, name)) => (Some(c), name),
            None => (None, fonts::DISPLAY_FALLBACK.to_owned()),
        };
        let display_bundled = display_collection.is_some();
        let ui = first_installed(&factory, &fonts::UI);
        let mono = first_installed(&factory, &fonts::MONO);

        let make = |family: &str,
                    collection: Option<&IDWriteFontCollection>,
                    weight: DWRITE_FONT_WEIGHT,
                    size: f32,
                    center: bool|
         -> Result<IDWriteTextFormat> {
            let f = unsafe {
                factory.CreateTextFormat(
                    &HSTRING::from(family),
                    collection,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    &HSTRING::from("en-us"),
                )?
            };
            unsafe {
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                if center {
                    f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                }
                let sign = factory.CreateEllipsisTrimmingSign(&f)?;
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                f.SetTrimming(&trimming, &sign)?;
            }
            Ok(f)
        };

        let display_collection: Option<IDWriteFontCollection> =
            display_collection.map(|c| c.cast()).transpose()?;
        Ok(Self {
            display: make(
                &display_family,
                display_collection.as_ref(),
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                type_scale::TITLE,
                false,
            )?,
            display_center: make(
                &display_family,
                display_collection.as_ref(),
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                type_scale::TITLE,
                true,
            )?,
            mono_block: {
                let f = make(
                    mono,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    type_scale::CAPTION,
                    false,
                )?;
                unsafe {
                    f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
                }
                f
            },
            lead_center: make(
                ui,
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                type_scale::LEAD,
                true,
            )?,
            body: make(ui, None, DWRITE_FONT_WEIGHT_NORMAL, type_scale::BODY, false)?,
            body_center: make(ui, None, DWRITE_FONT_WEIGHT_NORMAL, type_scale::BODY, true)?,
            caption: make(
                ui,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                type_scale::CAPTION,
                false,
            )?,
            caption_center: make(
                ui,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                type_scale::CAPTION,
                true,
            )?,
            code: make(
                mono,
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                type_scale::CAPTION,
                true,
            )?,
            factory,
            display_bundled,
        })
    }

    /// A text layout for a format code: uppercase with 0.06em tracking.
    /// Returns the layout and its width.
    pub fn code_layout(&self, code: &str, max_w: f32, h: f32) -> Result<(IDWriteTextLayout, f32)> {
        let upper: Vec<u16> = code.to_uppercase().encode_utf16().collect();
        let layout = unsafe {
            self.factory
                .CreateTextLayout(&upper, &self.code, max_w, h)?
        };
        let tracking = type_scale::CODE_TRACKING_EM * type_scale::CAPTION;
        if let Ok(l1) = layout.cast::<IDWriteTextLayout1>() {
            let range = DWRITE_TEXT_RANGE {
                startPosition: 0,
                length: upper.len() as u32,
            };
            unsafe { l1.SetCharacterSpacing(0.0, tracking, 0.0, range)? };
        }
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut m)? };
        Ok((layout, m.widthIncludingTrailingWhitespace))
    }
}

impl Text {
    /// Width of `s` in `format`, for placing text after other text.
    pub fn measure(&self, s: &str, format: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self
                .factory
                .CreateTextLayout(&wide, format, 10_000.0, 100.0)
            else {
                return 0.0;
            };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return 0.0;
            }
            m.widthIncludingTrailingWhitespace
        }
    }
}

/// The bundled serif as a private font collection. Needs Windows 10 1703
/// or later; earlier systems get the fallback serif.
fn bundled_collection(factory: &IDWriteFactory) -> Result<IDWriteFontCollection1> {
    let f5: IDWriteFactory5 = factory.cast()?;
    unsafe {
        let loader = f5.CreateInMemoryFontFileLoader()?;
        f5.RegisterFontFileLoader(&loader)?;
        let data = fonts::DISPLAY_TTF;
        let file = loader.CreateInMemoryFontFileReference(
            &f5,
            data.as_ptr().cast(),
            data.len() as u32,
            None,
        )?;
        let builder = f5.CreateFontSetBuilder()?;
        builder.AddFontFile(&file)?;
        let set = builder.CreateFontSet()?;
        f5.CreateFontCollectionFromFontSet(&set)
    }
}

/// The family name DirectWrite gives the bundled font. Asked for rather
/// than assumed, since naming tables can be read in more than one way.
fn family_name(collection: &IDWriteFontCollection1) -> Option<String> {
    unsafe {
        if collection.GetFontFamilyCount() == 0 {
            return None;
        }
        let family = collection.GetFontFamily(0).ok()?;
        let names = family.GetFamilyNames().ok()?;
        let len = names.GetStringLength(0).ok()? as usize;
        let mut buf = vec![0u16; len + 1];
        names.GetString(0, &mut buf).ok()?;
        let name = String::from_utf16_lossy(&buf[..len]);
        (!name.is_empty()).then_some(name)
    }
}

/// The first family in `families` that Windows has, else the last one.
fn first_installed(factory: &IDWriteFactory, families: &[&'static str]) -> &'static str {
    let mut collection = None;
    if unsafe { factory.GetSystemFontCollection(&mut collection, false) }.is_ok() {
        if let Some(c) = collection {
            for name in families {
                let mut index = 0;
                let mut exists = BOOL(0);
                let found =
                    unsafe { c.FindFamilyName(&HSTRING::from(*name), &mut index, &mut exists) };
                if found.is_ok() && exists.as_bool() {
                    return name;
                }
            }
        }
    }
    families[families.len() - 1]
}
