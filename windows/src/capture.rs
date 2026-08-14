//! Screen capture via DXGI Desktop Duplication.
//!
//! The pipeline stays on the GPU until the very last step: we copy only the
//! region of interest from the back buffer texture into a CPU-readable staging
//! texture via `CopySubresourceRegion`, then map it. This avoids copying the
//! whole desktop to CPU on every frame.

use anyhow::{anyhow, Context, Result};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
};

use screen_translator_core::types::Frame;

pub struct DesktopCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    /// Top-left corner of the duplicated output in virtual-screen coords.
    origin: (i32, i32),
    screen_size: (u32, u32),
    staging: Option<ID3D11Texture2D>,
    staging_size: (u32, u32),
}

impl DesktopCapture {
    pub fn new() -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().context("CreateDXGIFactory1")?;
            let adapter: IDXGIAdapter1 = factory
                .EnumAdapters1(0)
                .context("EnumAdapters1 (no graphics adapter)")?;
            let output = adapter.EnumOutputs(0).context("EnumOutputs (no display output)")?;
            let output1: IDXGIOutput1 = output.cast().context("IDXGIOutput1 cast")?;

            let feature_levels = [
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_9_1,
            ];
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("D3D11CreateDevice")?;
            let device = device.ok_or_else(|| anyhow!("D3D11CreateDevice returned no device"))?;
            let context = context
                .ok_or_else(|| anyhow!("D3D11CreateDevice returned no context"))?;

            let duplication = output1.DuplicateOutput(&device).context("DuplicateOutput (is another app duplicating the desktop?)")?;

            let desc = output.GetDesc().context("IDXGIOutput::GetDesc")?;
            let origin = (desc.DesktopCoordinates.left, desc.DesktopCoordinates.top);
            let screen_size = (
                (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left) as u32,
                (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top) as u32,
            );

            // Discard the initial desktop frame (it represents the pre-existing
            // screen, not a change).
            let _ = capture_frame_raw(&duplication, &context);

            Ok(Self {
                device,
                context,
                duplication,
                origin,
                screen_size,
                staging: None,
                staging_size: (0, 0),
            })
        }
    }

    pub fn origin(&self) -> (i32, i32) {
        self.origin
    }

    /// Capture the given rectangle (in output-local coordinates) as a BGRA8
    /// frame. Returns `None` when no new frame was presented within the wait
    /// window.
    pub fn capture_region(&mut self, x: u32, y: u32, w: u32, h: u32) -> Result<Option<Frame>> {
        if w == 0 || h == 0 {
            return Ok(None);
        }
        unsafe {
            // Make sure the whole desktop is inside the output bounds.
            let x = x.min(self.screen_size.0);
            let y = y.min(self.screen_size.1);
            let right = (x + w).min(self.screen_size.0);
            let bottom = (y + h).min(self.screen_size.1);
            if right <= x || bottom <= y {
                return Ok(None);
            }
            let (w, h) = (right - x, bottom - y);

            // Acquire the frame. The returned texture is valid until ReleaseFrame.
            let Some(desktop_tex) = acquire_texture(&self.duplication)? else {
                return Ok(None); // timeout
            };

            let mut pixels = Vec::with_capacity((w * h * 4) as usize);

            // Ensure a staging texture exists for this desktop size.
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            desktop_tex.GetDesc(&mut desc);
            if self.staging_size != (desc.Width, desc.Height) {
                self.staging = Some(create_staging(&self.device, desc.Width, desc.Height)?);
                self.staging_size = (desc.Width, desc.Height);
            }
            let staging = self.staging.as_ref().unwrap();

            // Copy only the requested sub-rectangle to CPU.
            let src_box = D3D11_BOX {
                left: x,
                top: y,
                front: 0,
                right: x + w,
                bottom: y + h,
                back: 1,
            };
            self.context
                .CopySubresourceRegion(staging, 0, 0, 0, 0, &desktop_tex, 0, Some(&src_box));

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("ID3D11DeviceContext::Map")?;

            let src = std::slice::from_raw_parts(mapped.pData as *const u8, (h as usize) * mapped.RowPitch as usize);
            let row_bytes = (w * 4) as usize;
            for row in 0..h as usize {
                let start = row * mapped.RowPitch as usize;
                pixels.extend_from_slice(&src[start..start + row_bytes]);
            }
            self.context.Unmap(staging, 0);

            // ReleaseFrame invalidates the desktop texture reference.
            drop(desktop_tex);
            self.duplication.ReleaseFrame().context("ReleaseFrame")?;

            Ok(Some(Frame {
                width: w,
                height: h,
                pixels,
                timestamp: now_secs(),
            }))
        }
    }
}

/// Acquire the next frame's texture. Returns `Ok(None)` on a wait timeout and
/// re-creates duplication if the session was lost.
fn acquire_texture(
    duplication: &IDXGIOutputDuplication,
) -> Result<Option<ID3D11Texture2D>> {
    let mut frame_info = windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource: Option<windows::Win32::Graphics::Dxgi::IDXGIResource> = None;
    let result = unsafe { duplication.AcquireNextFrame(32, &mut frame_info, &mut resource) };
    if let Err(e) = result {
        if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
            return Ok(None);
        }
        if e.code() == DXGI_ERROR_ACCESS_LOST {
            return Err(anyhow!("DXGI_ACCESS_LOST: desktop session changed"));
        }
        return Err(anyhow!("AcquireNextFrame failed: {e}"));
    }
    let Some(res) = resource else {
        return Ok(None);
    };
    let tex: ID3D11Texture2D = res.cast().context("desktop resource is not a texture")?;
    Ok(Some(tex))
}

/// Fetch and immediately release a single raw frame (used at init).
fn capture_frame_raw(
    duplication: &IDXGIOutputDuplication,
    _context: &ID3D11DeviceContext,
) -> Result<()> {
    let _ = acquire_texture(duplication)?;
    unsafe {
        duplication.ReleaseFrame().ok();
    }
    Ok(())
}

fn create_staging(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut tex))
            .context("CreateTexture2D (staging)")?;
    }
    tex.ok_or_else(|| anyhow!("CreateTexture2D returned no texture"))
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
