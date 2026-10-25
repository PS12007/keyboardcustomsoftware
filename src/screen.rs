//! Keeps a tiny, averaged copy of the main screen for the "Screen Grid" effect.
//! Only runs while that effect is selected.
//!
//! The result is a 65 x 24 picture: 4 cells per key width and 4 per key row.
//!
//! Normal path (DXGI desktop duplication): the graphics card hands over a new
//! frame only when something on screen changed, and shrinks it for us (mipmaps),
//! so we read back about 120 x 67 pixels instead of millions. When nothing
//! changes it costs next to nothing.
//!
//! Fallback (GDI): used only if duplication isn't available. Slower, so it runs
//! less often.

use crate::color::Rgb;
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC, SelectObject,
    SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, SRCCOPY,
};
use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SystemParametersInfoW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETWORKAREA};

pub const GW: usize = 65;
pub const GH: usize = 24;

/// How often to look for a new screen picture.
const INTERVAL: Duration = Duration::from_millis(40);
/// The GDI fallback blocks for ~20 ms per grab, so it runs less often.
const GDI_INTERVAL: Duration = Duration::from_millis(100);

/// A small screen picture plus where it sits on the desktop (physical pixels).
struct Small {
    w: usize,
    h: usize,
    px: Vec<Rgb>,
    desk: RECT,
}

pub struct ScreenCapture {
    dup: Option<Dup>,
    gdi: Option<Gdi>,
    retry_dup: Instant,
    last: Option<Instant>,
    small: Option<Small>,
    pub px: Vec<Rgb>,
}

// All handles are only ever used from the thread that owns the Renderer.
unsafe impl Send for ScreenCapture {}

impl ScreenCapture {
    pub fn new() -> Self {
        ScreenCapture {
            dup: None,
            gdi: None,
            retry_dup: Instant::now(),
            last: None,
            small: None,
            px: vec![[0.0; 3]; GW * GH],
        }
    }

    /// Refresh `px` if it's time. `skip_taskbar` leaves out the taskbar.
    pub fn update(&mut self, skip_taskbar: bool) {
        let every = if self.dup.is_none() && self.gdi.is_some() { GDI_INTERVAL } else { INTERVAL };
        if self.last.is_some_and(|t| t.elapsed() < every) {
            return;
        }
        self.last = Some(Instant::now());
        unsafe {
            // real pixels, not the scaled-down ones Windows gives DPI-unaware code
            let prev = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            self.grab();
            let area = if skip_taskbar { work_area() } else { None };
            if !prev.is_null() {
                SetThreadDpiAwarenessContext(prev);
            }
            if let Some(s) = &self.small {
                shrink(s, area.unwrap_or(s.desk), &mut self.px);
            }
        }
    }

    unsafe fn grab(&mut self) {
        if self.dup.is_none() && Instant::now() >= self.retry_dup {
            self.dup = Dup::new();
            // e.g. the lock screen or a UAC prompt is showing: try again soon
            self.retry_dup = Instant::now() + Duration::from_secs(2);
        }
        if let Some(d) = self.dup.as_mut() {
            match d.next(&mut self.small) {
                Ok(()) => {
                    self.gdi = None;
                    return;
                }
                Err(_) => self.dup = None, // screen mode changed etc.: rebuild later
            }
        }
        if self.gdi.is_none() {
            self.gdi = Gdi::new();
        }
        if let Some(g) = &self.gdi {
            g.grab(&mut self.small);
        }
    }
}

unsafe fn work_area() -> Option<RECT> {
    let mut wa: RECT = std::mem::zeroed();
    let ok = SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut RECT as *mut _, 0) != 0;
    (ok && wa.right > wa.left && wa.bottom > wa.top).then_some(wa)
}

/// Average the part of `s` inside `area` (desktop pixels) into the GW x GH grid.
fn shrink(s: &Small, area: RECT, out: &mut [Rgb]) {
    if s.w == 0 || s.h == 0 || s.px.len() < s.w * s.h {
        return;
    }
    let dw = (s.desk.right - s.desk.left).max(1) as f32;
    let dh = (s.desk.bottom - s.desk.top).max(1) as f32;
    let fx = |x: i32| ((x - s.desk.left) as f32 / dw * s.w as f32).clamp(0.0, s.w as f32);
    let fy = |y: i32| ((y - s.desk.top) as f32 / dh * s.h as f32).clamp(0.0, s.h as f32);
    let (ax0, ax1, ay0, ay1) = (fx(area.left), fx(area.right), fy(area.top), fy(area.bottom));
    for gy in 0..GH {
        let y0 = ((ay0 + (ay1 - ay0) * gy as f32 / GH as f32) as usize).min(s.h - 1);
        let y1 = ((ay0 + (ay1 - ay0) * (gy + 1) as f32 / GH as f32).ceil() as usize).clamp(y0 + 1, s.h);
        for gx in 0..GW {
            let x0 = ((ax0 + (ax1 - ax0) * gx as f32 / GW as f32) as usize).min(s.w - 1);
            let x1 = ((ax0 + (ax1 - ax0) * (gx + 1) as f32 / GW as f32).ceil() as usize).clamp(x0 + 1, s.w);
            let mut sum = [0.0f32; 3];
            for y in y0..y1 {
                for p in &s.px[y * s.w + x0..y * s.w + x1] {
                    sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as f32;
            out[gy * GW + gx] = sum.map(|v| v / n);
        }
    }
}

fn bgra(p: &[u8]) -> Rgb {
    [p[2] as f32 / 255.0, p[1] as f32 / 255.0, p[0] as f32 / 255.0]
}

// ---------------------------------------------------------------- DXGI duplication

struct Dup {
    dup: IDXGIOutputDuplication,
    ctx: ID3D11DeviceContext,
    mips: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
    staging: ID3D11Texture2D,
    level: u32,
    w: usize,
    h: usize,
    desk: RECT,
    /// a copy was started last time and can be read now
    pending: bool,
}

impl Dup {
    unsafe fn new() -> Option<Self> {
        // find the main screen (the one at 0,0) and the graphics card driving it;
        // on laptops with two GPUs it can be either one
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut found = None;
        'outer: for a in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(a) else { break };
            for o in 0.. {
                let Ok(output) = adapter.EnumOutputs(o) else { break };
                let Ok(desc) = output.GetDesc() else { continue };
                let r = desc.DesktopCoordinates;
                if desc.AttachedToDesktop.as_bool() && r.left == 0 && r.top == 0 {
                    found = Some((adapter.cast::<IDXGIAdapter>().ok()?, output, r));
                    break 'outer;
                }
            }
        }
        let (adapter, output, r) = found?;
        let desk = RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
        let mut device: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )
        .ok()?;
        let (device, ctx) = (device?, ctx?);
        let dup = output.cast::<IDXGIOutput1>().ok()?.DuplicateOutput(&device).ok()?;
        let mode = dup.GetDesc().ModeDesc;
        let (sw, sh) = (mode.Width, mode.Height);
        // halve the size while it's still at least as big as the grid
        let mut level = 0;
        while (sw >> (level + 1)) as usize >= GW && (sh >> (level + 1)) as usize >= GH {
            level += 1;
        }
        let (w, h) = ((sw >> level).max(1), (sh >> level).max(1));
        let tex = |width: u32, height: u32, mips: u32, usage: D3D11_USAGE, bind: i32, cpu: i32, misc: i32| {
            let d = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: mips,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: usage,
                BindFlags: bind as u32,
                CPUAccessFlags: cpu as u32,
                MiscFlags: misc as u32,
            };
            let mut t: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&d, None, Some(&mut t)).ok()?;
            t
        };
        let mips = tex(
            sw,
            sh,
            level + 1,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0,
            0,
            D3D11_RESOURCE_MISC_GENERATE_MIPS.0,
        )?;
        let staging = tex(w, h, 1, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0, 0)?;
        let mut srv = None;
        device.CreateShaderResourceView(&mips, None, Some(&mut srv)).ok()?;
        Some(Dup { dup, ctx, mips, srv: srv?, staging, level, w: w as usize, h: h as usize, desk, pending: false })
    }

    /// Picks up the picture copied on the previous call (so we never wait for the
    /// graphics card), then starts copying the newest frame if the screen changed.
    unsafe fn next(&mut self, small: &mut Option<Small>) -> windows::core::Result<()> {
        if self.pending {
            self.pending = false;
            self.read(small)?;
        }
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res = None;
        match self.dup.AcquireNextFrame(0, &mut info, &mut res) {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(()),
            Err(e) => return Err(e),
        }
        let result = (|| {
            if info.LastPresentTime == 0 && small.is_some() {
                return Ok(()); // only the mouse moved
            }
            let Some(frame) = res.as_ref() else { return Ok(()) };
            let frame: ID3D11Texture2D = frame.cast()?;
            self.ctx.CopySubresourceRegion(&self.mips, 0, 0, 0, 0, &frame, 0, None);
            self.ctx.GenerateMips(&self.srv);
            self.ctx.CopySubresourceRegion(&self.staging, 0, 0, 0, 0, &self.mips, self.level, None);
            self.ctx.Flush();
            self.pending = true;
            Ok(())
        })();
        let _ = self.dup.ReleaseFrame();
        result
    }

    unsafe fn read(&mut self, small: &mut Option<Small>) -> windows::core::Result<()> {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        self.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let s = small.get_or_insert_with(|| Small { w: 0, h: 0, px: vec![], desk: self.desk });
        s.w = self.w;
        s.h = self.h;
        s.desk = self.desk;
        s.px.clear();
        for y in 0..self.h {
            let row = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), self.w * 4);
            s.px.extend(row.chunks_exact(4).map(bgra));
        }
        self.ctx.Unmap(&self.staging, 0);
        Ok(())
    }
}

// ---------------------------------------------------------------- GDI fallback

const FW: usize = GW * 4;
const FH: usize = GH * 4;

struct Gdi {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *const u8,
}

impl Gdi {
    unsafe fn new() -> Option<Self> {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: FW as i32,
            biHeight: -(FH as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old = SelectObject(dc, bmp);
        // nearest-pixel sampling (16 samples per grid cell, averaged later);
        // HALFTONE looks about the same here but costs 4x the CPU
        SetStretchBltMode(dc, COLORONCOLOR);
        Some(Gdi { dc, bmp, old, bits: bits as *const u8 })
    }

    unsafe fn grab(&self, small: &mut Option<Small>) {
        let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return;
        }
        let ok = StretchBlt(self.dc, 0, 0, FW as i32, FH as i32, screen, 0, 0, w, h, SRCCOPY) != 0;
        ReleaseDC(std::ptr::null_mut(), screen);
        if !ok {
            return; // keep the last picture
        }
        GdiFlush();
        let src = std::slice::from_raw_parts(self.bits, FW * FH * 4);
        *small = Some(Small {
            w: FW,
            h: FH,
            px: src.chunks_exact(4).map(bgra).collect(),
            desk: RECT { left: 0, top: 0, right: w, bottom: h },
        });
    }
}

impl Drop for Gdi {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    fn run(s: &mut super::ScreenCapture, name: &str) {
        let (n, mut busy, mut fresh) = (90, Duration::ZERO, 0);
        for _ in 0..n {
            s.last = None;
            let before = s.px.clone();
            let t = Instant::now();
            s.update(true);
            busy += t.elapsed();
            fresh += (before != s.px) as u32;
            std::thread::sleep(Duration::from_millis(33));
        }
        let mean = s.px.iter().fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
        println!(
            "{name}: {:.2} ms per update, {fresh}/{n} changed, mean color {:?}",
            busy.as_secs_f64() * 1000.0 / n as f64,
            mean.map(|v| v / s.px.len() as f32)
        );
    }

    /// cargo test --release grab_speed -- --ignored --nocapture
    #[test]
    #[ignore]
    fn grab_speed() {
        let mut s = super::ScreenCapture::new();
        let t = Instant::now();
        s.update(true);
        println!("first update (setup): {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
        run(&mut s, "duplication");
        println!("duplication active: {}", s.dup.is_some());
        // force the fallback
        s.retry_dup = Instant::now() + Duration::from_secs(3600);
        s.dup = None;
        run(&mut s, "gdi");
    }
}
