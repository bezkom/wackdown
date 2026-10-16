//! Printing through the Windows print dialog and the XPS/Direct2D print pipeline. "Microsoft
//! Print to PDF" is just another printer here, so this is also how a PDF is made: no browser,
//! no PDF library, the same Direct2D drawing that goes on screen goes on paper.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Printing::PrintTicket::*;
use windows::Win32::Storage::Xps::Printing::*;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Controls::Dialogs::*;

use crate::fonts::wide;

/// What the user picked in the print dialog, in device-independent pixels (1/96 in).
pub struct PrinterChoice {
    pub name: String,
    /// Full sheet size.
    pub page_w: f32,
    pub page_h: f32,
    /// Printable area (the hardware margins are outside it).
    pub printable: (f32, f32, f32, f32),
    /// The dialog's settings (orientation, paper) as a print ticket for the job; None if the
    /// conversion failed, in which case the printer's defaults apply.
    ticket: Option<IStream>,
}

/// Shows the print dialog. `Ok(None)` when the user cancelled.
pub fn choose_printer(hwnd: HWND) -> Result<Option<PrinterChoice>> {
    unsafe {
        let mut pd = PRINTDLGEXW {
            lStructSize: std::mem::size_of::<PRINTDLGEXW>() as u32,
            hwndOwner: hwnd,
            Flags: PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION | PD_NOCURRENTPAGE | PD_HIDEPRINTTOFILE | PD_USEDEVMODECOPIESANDCOLLATE,
            nCopies: 1,
            nStartPage: START_PAGE_GENERAL,
            ..Default::default()
        };
        PrintDlgExW(&mut pd)?;
        let result = (|| -> Result<Option<PrinterChoice>> {
            if pd.dwResultAction != PD_RESULT_PRINT || pd.hDC.is_invalid() {
                return Ok(None);
            }
            let name = device_name(pd.hDevNames).ok_or_else(|| Error::from(E_FAIL))?;
            let caps = |i: GET_DEVICE_CAPS_INDEX| GetDeviceCaps(pd.hDC, i) as f32;
            let (dpi_x, dpi_y) = (caps(LOGPIXELSX).max(1.0), caps(LOGPIXELSY).max(1.0));
            let (kx, ky) = (96.0 / dpi_x, 96.0 / dpi_y);
            let page_w = caps(PHYSICALWIDTH) * kx;
            let page_h = caps(PHYSICALHEIGHT) * ky;
            let (ox, oy) = (caps(PHYSICALOFFSETX) * kx, caps(PHYSICALOFFSETY) * ky);
            let printable = (ox, oy, ox + caps(HORZRES) * kx, oy + caps(VERTRES) * ky);
            if !(page_w > 10.0 && page_h > 10.0) {
                return Err(Error::from(E_FAIL));
            }
            let ticket = print_ticket(&name, pd.hDevMode);
            Ok(Some(PrinterChoice { name, page_w, page_h, printable, ticket }))
        })();
        if !pd.hDC.is_invalid() {
            let _ = DeleteDC(pd.hDC);
        }
        for h in [pd.hDevMode, pd.hDevNames] {
            if !h.is_invalid() {
                let _ = GlobalFree(h);
            }
        }
        result
    }
}

impl PrinterChoice {
    /// US Letter on a named printer, without a dialog: 8.5 x 11 in with a quarter-inch hardware
    /// margin. The printer's own defaults apply (no ticket).
    #[cfg(test)]
    pub fn letter(printer: &str) -> Self {
        Self { name: printer.to_string(), page_w: 816.0, page_h: 1056.0, printable: (24.0, 24.0, 792.0, 1032.0), ticket: None }
    }

    /// Opens a print job on the chosen printer; pages are added through Direct2D's print
    /// control. With `output`, the job is written to that stream instead of the printer.
    pub fn open_job(&self, job_name: &str, output: Option<&IStream>) -> Result<IPrintDocumentPackageTarget> {
        unsafe {
            let factory: IPrintDocumentPackageTargetFactory = CoCreateInstance(&PrintDocumentPackageTargetFactory, None, CLSCTX_INPROC_SERVER)?;
            let name = wide(&self.name);
            let job = wide(job_name);
            factory.CreateDocumentPackageTargetForPrintJob(PCWSTR(name.as_ptr()), PCWSTR(job.as_ptr()), output, self.ticket.as_ref())
        }
    }
}

/// The printer name out of a DEVNAMES block.
unsafe fn device_name(hdevnames: HGLOBAL) -> Option<String> {
    if hdevnames.is_invalid() {
        return None;
    }
    let base = windows::Win32::System::Memory::GlobalLock(hdevnames) as *const u16;
    if base.is_null() {
        return None;
    }
    let names = &*(base as *const DEVNAMES);
    let at = base.add(names.wDeviceOffset as usize);
    let mut len = 0;
    while *at.add(len) != 0 && len < 512 {
        len += 1;
    }
    let name = String::from_utf16_lossy(std::slice::from_raw_parts(at, len));
    let _ = windows::Win32::System::Memory::GlobalUnlock(hdevnames);
    Some(name)
}

/// Converts the dialog's DEVMODE (paper, orientation, copies) to a job print ticket stream.
unsafe fn print_ticket(printer: &str, hdevmode: HGLOBAL) -> Option<IStream> {
    if hdevmode.is_invalid() {
        return None;
    }
    let size = windows::Win32::System::Memory::GlobalSize(hdevmode) as u32;
    let devmode = windows::Win32::System::Memory::GlobalLock(hdevmode) as *const DEVMODEA;
    if devmode.is_null() {
        return None;
    }
    let result = (|| -> Result<IStream> {
        let name = wide(printer);
        let provider = PTOpenProvider(PCWSTR(name.as_ptr()), 1)?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true)?;
        let converted = PTConvertDevModeToPrintTicket(provider, size, devmode, kPTJobScope, &stream);
        let _ = PTCloseProvider(provider);
        converted?;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        Ok(stream)
    })();
    let _ = windows::Win32::System::Memory::GlobalUnlock(hdevmode);
    result.ok()
}
