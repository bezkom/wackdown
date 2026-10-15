use windows::core::*;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::DirectWrite::*;

// Embedded TTF font bytes compiled directly into the .exe binary: Iosevka v34.9.0, subset by
// assets/fonts/subset_fonts.py (SIL OFL 1.1, see assets/fonts/Iosevka-LICENSE.md).
const EMBEDDED_FONTS: [&[u8]; 12] = [
    include_bytes!("../assets/fonts/Iosevka-Regular.ttf"),
    include_bytes!("../assets/fonts/Iosevka-Bold.ttf"),
    include_bytes!("../assets/fonts/Iosevka-Italic.ttf"),
    include_bytes!("../assets/fonts/Iosevka-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/IosevkaAile-Regular.ttf"),
    include_bytes!("../assets/fonts/IosevkaAile-Bold.ttf"),
    include_bytes!("../assets/fonts/IosevkaAile-Italic.ttf"),
    include_bytes!("../assets/fonts/IosevkaAile-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/IosevkaEtoile-Regular.ttf"),
    include_bytes!("../assets/fonts/IosevkaEtoile-Bold.ttf"),
    include_bytes!("../assets/fonts/IosevkaEtoile-Italic.ttf"),
    include_bytes!("../assets/fonts/IosevkaEtoile-BoldItalic.ttf"),
];

/// Monospace; also the fallback for anything that can't be resolved.
pub const MONO_FAMILY: &str = "Iosevka";
/// Quasi-proportional sans.
pub const SANS_FAMILY: &str = "Iosevka Aile";
/// Quasi-proportional slab serif.
pub const SERIF_FAMILY: &str = "Iosevka Etoile";
const SYSTEM_FALLBACK_FAMILY: &str = "Consolas";

/// A font family name plus the collection it lives in.
#[derive(Clone)]
pub struct FontChoice {
    pub family: Vec<u16>,
    pub collection: IDWriteFontCollection,
    pub metrics: FaceMetrics,
}

/// Vertical metrics as fractions of the em, read from the font itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: f32,
    pub x_height: f32,
}

impl Default for FaceMetrics {
    fn default() -> Self {
        Self { ascent: 0.8, descent: 0.2, cap_height: 0.7, x_height: 0.5 }
    }
}

impl FaceMetrics {
    /// Baseline offset from the top of a line box of height `line` that centers the font's
    /// ascent+descent box (equal half-leading above and below).
    pub fn baseline(&self, size: f32, line: f32) -> f32 {
        let content = (self.ascent + self.descent) * size;
        (line - content) / 2.0 + self.ascent * size
    }
}

fn read_metrics(collection: &IDWriteFontCollection, family: &[u16]) -> Option<FaceMetrics> {
    unsafe {
        let mut index = 0u32;
        let mut exists = BOOL(0);
        collection.FindFamilyName(PCWSTR(family.as_ptr()), &mut index, &mut exists).ok()?;
        if !exists.as_bool() {
            return None;
        }
        let font = collection
            .GetFontFamily(index)
            .ok()?
            .GetFirstMatchingFont(DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL)
            .ok()?;
        let mut m = DWRITE_FONT_METRICS::default();
        font.GetMetrics(&mut m);
        let em = m.designUnitsPerEm.max(1) as f32;
        Some(FaceMetrics {
            ascent: m.ascent as f32 / em,
            descent: m.descent as f32 / em,
            cap_height: m.capHeight as f32 / em,
            x_height: m.xHeight as f32 / em,
        })
    }
}

fn choice(name: &str, collection: &IDWriteFontCollection) -> FontChoice {
    let family = wide(name);
    let metrics = read_metrics(collection, &family).unwrap_or_default();
    FontChoice { family, collection: collection.clone(), metrics }
}

impl FontChoice {
    pub fn same_as(&self, other: &FontChoice) -> bool {
        self.family == other.family && self.collection == other.collection
    }
}

pub struct Fonts {
    embedded: Option<IDWriteFontCollection>,
    system: IDWriteFontCollection,
}

impl Fonts {
    pub fn new(factory: &IDWriteFactory) -> Result<Self> {
        let embedded = unsafe { create_embedded_font_collection(factory) }.ok();
        let system = unsafe {
            let mut collection = None;
            factory.GetSystemFontCollection(&mut collection, false)?;
            collection.ok_or_else(|| Error::from(windows::Win32::Foundation::E_FAIL))?
        };
        Ok(Self { embedded, system })
    }

    /// Resolves a CSS-style family list (e.g. `"Iosevka Etoile", Georgia, serif`) to the first
    /// available family. Embedded families win over installed ones; the generic names
    /// `monospace`, `sans-serif` and `serif` map to Iosevka, Iosevka Aile and Iosevka Etoile.
    pub fn resolve(&self, css_family_list: &str) -> FontChoice {
        for name in css_family_list.split(',').map(|n| n.trim().trim_matches(|c| c == '"' || c == '\'').trim()) {
            let name = match name.to_ascii_lowercase().as_str() {
                "" => continue,
                "monospace" => MONO_FAMILY,
                "sans-serif" | "sans" => SANS_FAMILY,
                "serif" => SERIF_FAMILY,
                _ => name,
            };
            if let Some(collection) = self.embedded.as_ref().filter(|c| has_family(c, name)) {
                return choice(name, collection);
            }
            if has_family(&self.system, name) {
                return choice(name, &self.system);
            }
        }
        match &self.embedded {
            Some(collection) => choice(MONO_FAMILY, collection),
            None => choice(SYSTEM_FALLBACK_FAMILY, &self.system),
        }
    }
}

fn has_family(collection: &IDWriteFontCollection, name: &str) -> bool {
    let name = wide(name);
    let mut index = 0u32;
    let mut exists = BOOL(0);
    unsafe { collection.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists) }.is_ok() && exists.as_bool()
}

/// Builds an in-memory DirectWrite font collection from the embedded TTFs,
/// bypassing the Windows font directory for reproducible rendering.
/// Requires Windows 10 1703+ (IDWriteFactory5); callers fall back to system fonts otherwise.
unsafe fn create_embedded_font_collection(factory: &IDWriteFactory) -> Result<IDWriteFontCollection> {
    let factory5: IDWriteFactory5 = factory.cast()?;
    let loader = factory5.CreateInMemoryFontFileLoader()?;
    factory5.RegisterFontFileLoader(&loader)?;
    let builder = factory5.CreateFontSetBuilder()?;
    for bytes in EMBEDDED_FONTS {
        // The data is 'static, so no owner object is needed to keep it alive.
        let file = loader.CreateInMemoryFontFileReference(
            &factory5,
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            None::<&IUnknown>,
        )?;
        builder.AddFontFile(&file)?;
    }
    let set = builder.CreateFontSet()?;
    let collection = factory5.CreateFontCollectionFromFontSet(&set)?;
    collection.cast()
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
