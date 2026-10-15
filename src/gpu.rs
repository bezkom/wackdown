//! Direct3D 11 + DXGI flip-model swap chain + Direct2D device context.
//!
//! Replaces ID2D1HwndRenderTarget so frames are presented in step with the display:
//! `present` with sync interval 1 blocks until the next vertical blank, which paces the
//! frame loop at the monitor's refresh rate (60, 120, 144 Hz...) with no timers involved.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

pub struct Gpu {
    hwnd: HWND,
    pub dc: ID2D1DeviceContext,
    swap_chain: IDXGISwapChain1,
    size: (u32, u32),
    dpi: f32,
}

impl Gpu {
    pub fn new(hwnd: HWND, factory: ID2D1Factory1, dpi: f32) -> Result<Self> {
        unsafe {
            let device = create_d3d_device()?;
            let dxgi_device: IDXGIDevice1 = device.cast()?;
            // Keep at most one frame queued: lowest input latency for scrolling.
            let _ = dxgi_device.SetMaximumFrameLatency(1);
            let adapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;

            let size = client_size(hwnd);
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                // Anchor old content top-left during live resize instead of stretching it.
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swap_chain = dxgi_factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;
            let _ = dxgi_factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);

            let d2d_device = factory.CreateDevice(&dxgi_device)?;
            let dc = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let gpu = Self { hwnd, dc, swap_chain, size, dpi };
            gpu.bind_back_buffer()?;
            Ok(gpu)
        }
    }

    fn bind_back_buffer(&self) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: self.dpi,
                dpiY: self.dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                ..Default::default()
            };
            let bitmap = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.dc.SetTarget(&bitmap);
            self.dc.SetDpi(self.dpi, self.dpi);
        }
        Ok(())
    }

    /// Matches the swap chain to the client area. Cheap when nothing changed.
    pub fn resize(&mut self) -> Result<()> {
        let size = client_size(self.hwnd);
        if size == self.size {
            return Ok(());
        }
        unsafe {
            // The back buffer must not be referenced while it is resized.
            self.dc.SetTarget(None);
            self.swap_chain.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        }
        self.size = size;
        self.bind_back_buffer()
    }

    pub fn set_dpi(&mut self, dpi: f32) -> Result<()> {
        self.dpi = dpi;
        unsafe { self.dc.SetDpi(dpi, dpi) };
        Ok(())
    }

    /// Presents the frame. With `vsync`, blocks until the next vertical blank.
    /// Returns false if the device was lost and everything must be recreated.
    pub fn present(&self, vsync: bool) -> bool {
        let hr = unsafe { self.swap_chain.Present(if vsync { 1 } else { 0 }, DXGI_PRESENT(0)) };
        !(hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET)
    }
}

/// Hardware device with BGRA support (needed by Direct2D); WARP software rendering when no GPU
/// is usable (remote desktop, some VMs).
unsafe fn create_d3d_device() -> Result<ID3D11Device> {
    let mut last = Error::from(E_FAIL);
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device = None;
        match D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        ) {
            Ok(()) => return device.ok_or_else(|| Error::from(E_FAIL)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

pub fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    ((rect.right - rect.left).max(1) as u32, (rect.bottom - rect.top).max(1) as u32)
}
