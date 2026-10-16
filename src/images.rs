//! Local images: decoded with Windows Imaging Component (PNG, JPEG, GIF, BMP, TIFF, WebP, and
//! whatever codecs are installed), converted to premultiplied BGRA and uploaded as Direct2D
//! bitmaps. Decoded images are cached per path so a relayout never decodes twice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows::core::*;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

use crate::fonts::wide;

/// Largest bitmap edge uploaded to the GPU; bigger images are downscaled while decoding.
const MAX_EDGE: u32 = 4096;

/// A decoded image: the GPU bitmap and its pixel size.
#[derive(Clone)]
pub struct Picture {
    pub bitmap: ID2D1Bitmap,
    pub width: f32,
    pub height: f32,
}

pub struct ImageCache {
    wic: Option<IWICImagingFactory>,
    /// `None` remembers a failed load, so a broken path is only tried once per document.
    pictures: HashMap<PathBuf, Option<Picture>>,
}

impl ImageCache {
    pub fn new() -> Self {
        let wic = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }.ok();
        Self { wic, pictures: HashMap::new() }
    }

    pub fn wic(&self) -> Option<&IWICImagingFactory> {
        self.wic.as_ref()
    }

    /// Forgets every decoded image (device loss, document reload).
    pub fn clear(&mut self) {
        self.pictures.clear();
    }

    /// The picture at `path`, decoding it on first use. `target` owns the resulting bitmap.
    pub fn get(&mut self, target: &ID2D1RenderTarget, path: &Path) -> Option<Picture> {
        if let Some(cached) = self.pictures.get(path) {
            return cached.clone();
        }
        let picture = self.wic.as_ref().and_then(|wic| unsafe { decode(wic, target, path) }.ok());
        self.pictures.insert(path.to_path_buf(), picture.clone());
        picture
    }
}

unsafe fn decode(wic: &IWICImagingFactory, target: &ID2D1RenderTarget, path: &Path) -> Result<Picture> {
    let name = wide(&path.to_string_lossy());
    let decoder = wic.CreateDecoderFromFilename(PCWSTR(name.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
    let frame = decoder.GetFrame(0)?;
    let (mut w, mut h) = (0u32, 0u32);
    frame.GetSize(&mut w, &mut h)?;
    if w == 0 || h == 0 {
        return Err(Error::from(windows::Win32::Foundation::E_FAIL));
    }
    // Huge images are scaled down on the CPU first: Direct2D bitmaps have a size limit, and
    // nothing on screen needs more than a few thousand pixels across anyway.
    let source: IWICBitmapSource = if w > MAX_EDGE || h > MAX_EDGE {
        let k = MAX_EDGE as f32 / w.max(h) as f32;
        let (sw, sh) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&frame, sw, sh, WICBitmapInterpolationModeFant)?;
        scaler.cast()?
    } else {
        frame.cast()?
    };
    let converter = wic.CreateFormatConverter()?;
    converter.Initialize(&source, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
    let bitmap = target.CreateBitmapFromWicBitmap(&converter, None)?;
    let size = bitmap.GetSize();
    Ok(Picture { bitmap, width: size.width, height: size.height })
}
