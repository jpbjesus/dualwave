use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use ds_proto::Connection;
use ds_proto::input::BatteryStatus;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use crate::platform::wide;
use parking_lot::Mutex;
use super::{Arc, Engine, OverlayCorner};

const WM_SHOW: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const REFRESH_MS: u32 = 500;

static HWND_: AtomicIsize = AtomicIsize::new(0);
static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();
static REQUEST: Mutex<Option<(OverlayCorner, Instant)>> = Mutex::new(None);
static ROWS: Mutex<Vec<Row>> = Mutex::new(Vec::new());
/// One controller as the overlay draws it.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    name: String,
    detail: String,
    label: String,
    /// 0..=1, or `None` while the battery is unknown.
    level: Option<f32>,
    low: bool,
    charging: bool,
}

pub(super) fn show(engine: &Arc<Engine>, corner: OverlayCorner, ms: u32) {
    *REQUEST.lock() = Some((corner, Instant::now() + Duration::from_millis(ms as u64)));
    let hwnd = HWND_.load(Ordering::Acquire);
    if hwnd != 0 {
        unsafe {
            PostMessageW(hwnd as HWND, WM_SHOW, 0, 0);
        }
        return;
    }
    let _ = ENGINE.set(engine.clone());
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("battery-overlay".into())
            .spawn(run);
    });
}

fn rows(engine: &Engine) -> Vec<Row> {
    let settings = engine.settings.read();
    let low_percent = settings.battery_overlay.low_percent;
    let devices: Vec<_> = engine.devices.read().clone();
    let mut out = Vec::new();
    for d in devices {
        let input = d.live.lock().input;
        let name = {
            let mac = d.mac.lock().clone();
            match settings.device_names.get(&mac) {
                Some(n) if !n.trim().is_empty() => n.clone(),
                _ => d.model.name().to_string(),
            }
        };
        let conn = match d.conn {
            Connection::Usb => "USB",
            Connection::Bluetooth => "Bluetooth",
        };
        let charging = matches!(input.battery_status, BatteryStatus::Charging | BatteryStatus::Full);
        let state = match input.battery_status {
            BatteryStatus::Charging => Some(crate::i18n::tr("Charging")),
            BatteryStatus::Full => Some(crate::i18n::tr("Full")),
            _ => None,
        };
        let detail = match state {
            Some(st) => format!("{conn} · {st}"),
            None => conn.to_string(),
        };
        let known = input.full && input.battery_status != BatteryStatus::Error;
        out.push(Row {
            name,
            detail,
            label: if !known {
                "—".into()
            } else if settings.battery_bars {
                let n = input.battery_bars() as usize;
                format!("{}{}", "■".repeat(n), "□".repeat(3 - n))
            } else {
                format!("{}%", input.battery_shown())
            },
            level: known.then(|| input.battery_shown().min(100) as f32 / 100.0),
            low: known && !charging && input.battery_percent < low_percent,
            charging,
        });
    }
    out
}

fn run() {
    unsafe {
        // Sizes are scaled by hand from the monitor's DPI, so Windows must
        // not scale the window again, even before the UI makes the whole
        // process DPI aware.
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(null());
        let class = wide("DSZBatteryOverlay");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let title = wide("DSZ battery");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            log::warn!("battery overlay: no window");
            return;
        }
        SetLayeredWindowAttributes(hwnd, 0, 235, LWA_ALPHA);
        HWND_.store(hwnd as isize, Ordering::Release);
        // Asked for while the window was being made.
        PostMessageW(hwnd, WM_SHOW, 0, 0);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Pixels at 96 DPI scaled to `dpi`.
fn px(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

const W: i32 = 300;
const PAD: i32 = 14;
const ROW_H: i32 = 46;

unsafe fn place(hwnd: HWND, rows: usize, corner: OverlayCorner) {
    let fg = GetForegroundWindow();
    let mon = MonitorFromWindow(if fg.is_null() { hwnd } else { fg }, MONITOR_DEFAULTTOPRIMARY);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut mi) == 0 {
        return;
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
        dx = 96;
    }
    let dpi = dx.max(96);
    let w = px(W, dpi);
    let h = px(PAD * 2 + ROW_H * rows.max(1) as i32, dpi);
    let m = px(24, dpi);
    // The work area, so the overlay stays clear of the taskbar on the
    // desktop; a fullscreen game covers the taskbar anyway.
    let r = mi.rcWork;
    let x = match corner {
        OverlayCorner::TopLeft | OverlayCorner::BottomLeft => r.left + m,
        _ => r.right - m - w,
    };
    let y = match corner {
        OverlayCorner::TopLeft | OverlayCorner::TopRight => r.top + m,
        _ => r.bottom - m - h,
    };
    DPI.store(dpi as isize, Ordering::Release);
    let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, px(16, dpi), px(16, dpi));
    // The window owns the region from here, unless the call fails.
    if SetWindowRgn(hwnd, rgn, 0) == 0 {
        DeleteObject(rgn as HGDIOBJ);
    }
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
}

static DPI: AtomicIsize = AtomicIsize::new(96);

/// Rebuild the rows; true when the window must grow or shrink.
fn refresh() -> bool {
    let Some(engine) = ENGINE.get() else {
        return false;
    };
    let new = rows(engine);
    let mut rows = ROWS.lock();
    let resized = rows.len() != new.len();
    *rows = new;
    resized
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SHOW => {
            let request = *REQUEST.lock();
            if let Some((corner, _)) = request.filter(|(_, until)| Instant::now() < *until) {
                refresh();
                place(hwnd, ROWS.lock().len(), corner);
                InvalidateRect(hwnd, null(), 0);
                SetTimer(hwnd, TIMER_ID, REFRESH_MS, None);
            }
            0
        }
        WM_TIMER => {
            let request = *REQUEST.lock();
            if let Some((corner, _)) = request.filter(|(_, until)| Instant::now() < *until) {
                if refresh() {
                    place(hwnd, ROWS.lock().len(), corner);
                }
                InvalidateRect(hwnd, null(), 0);
            } else {
                KillTimer(hwnd, TIMER_ID);
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

const BG: (u8, u8, u8) = (22, 23, 28);
const TEXT: (u8, u8, u8) = (236, 237, 242);
const MUTED: (u8, u8, u8) = (150, 153, 166);
const GOOD: (u8, u8, u8) = (88, 200, 128);
const MID: (u8, u8, u8) = (240, 184, 64);
const BAD: (u8, u8, u8) = (236, 76, 88);

unsafe fn font(dpi: u32, size: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    CreateFontW(
        -px(size, dpi),
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        face.as_ptr(),
    )
}

unsafe fn fill(dc: HDC, r: RECT, c: (u8, u8, u8)) {
    let b = CreateSolidBrush(rgb(c.0, c.1, c.2));
    FillRect(dc, &r, b);
    DeleteObject(b as HGDIOBJ);
}

unsafe fn text(dc: HDC, s: &str, mut r: RECT, c: (u8, u8, u8), f: HFONT, flags: u32) {
    let w: Vec<u16> = s.encode_utf16().collect();
    let old = SelectObject(dc, f as HGDIOBJ);
    SetTextColor(dc, rgb(c.0, c.1, c.2));
    DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(dc, old);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut cr: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut cr);
    let (w, h) = (cr.right - cr.left, cr.bottom - cr.top);
    // Drawn off screen first, so a refresh doesn't flicker.
    let dc = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(dc, bmp as HGDIOBJ);
    SetBkMode(dc, TRANSPARENT as i32);
    let dpi = DPI.load(Ordering::Acquire) as u32;
    fill(dc, cr, BG);

    let name_font = font(dpi, 15, FW_SEMIBOLD as i32);
    let small_font = font(dpi, 12, FW_NORMAL as i32);
    let pct_font = font(dpi, 15, FW_SEMIBOLD as i32);
    let rows = ROWS.lock().clone();
    if rows.is_empty() {
        let r = RECT { left: px(PAD, dpi), top: 0, right: w - px(PAD, dpi), bottom: h };
        text(dc, crate::i18n::tr("No controller connected"), r, MUTED, small_font, DT_VCENTER | DT_LEFT);
    }
    for (i, row) in rows.iter().enumerate() {
        let top = px(PAD + ROW_H * i as i32, dpi);
        let left = px(PAD, dpi);
        let right = w - px(PAD, dpi);
        let color = match row.level {
            _ if row.low => BAD,
            Some(l) if l < 0.5 && !row.charging => MID,
            Some(_) => GOOD,
            None => MUTED,
        };
        // Percentage, right-aligned, then the battery icon left of it.
        let pct_w = px(52, dpi);
        let pr = RECT { left: right - pct_w, top, right, bottom: top + px(ROW_H, dpi) };
        text(dc, &row.label, pr, color, pct_font, DT_VCENTER | DT_RIGHT);
        let bw = px(34, dpi);
        let bh = px(16, dpi);
        let bx = right - pct_w - px(8, dpi) - bw;
        let by = top + (px(ROW_H, dpi) - bh) / 2;
        let t = px(2, dpi).max(1);
        // Outline as four bars, then the nub and the level.
        let outline = if row.low { BAD } else { MUTED };
        fill(dc, RECT { left: bx, top: by, right: bx + bw, bottom: by + t }, outline);
        fill(dc, RECT { left: bx, top: by + bh - t, right: bx + bw, bottom: by + bh }, outline);
        fill(dc, RECT { left: bx, top: by, right: bx + t, bottom: by + bh }, outline);
        fill(dc, RECT { left: bx + bw - t, top: by, right: bx + bw, bottom: by + bh }, outline);
        let nub_h = bh / 2;
        fill(
            dc,
            RECT { left: bx + bw, top: by + (bh - nub_h) / 2, right: bx + bw + t + 1, bottom: by + (bh + nub_h) / 2 },
            outline,
        );
        if let Some(l) = row.level {
            let inner = bw - 4 * t;
            let fw = ((inner as f32) * l.clamp(0.0, 1.0)).round() as i32;
            if fw > 0 {
                fill(
                    dc,
                    RECT { left: bx + 2 * t, top: by + 2 * t, right: bx + 2 * t + fw, bottom: by + bh - 2 * t },
                    color,
                );
            }
        }
        if row.charging {
            let cr = RECT { left: bx, top: by, right: bx + bw, bottom: by + bh };
            text(dc, "⚡", cr, TEXT, small_font, DT_VCENTER | DT_CENTER);
        }
        // Name over connection and charge state.
        let text_right = bx - px(10, dpi);
        let nr = RECT { left, top: top + px(5, dpi), right: text_right, bottom: top + px(25, dpi) };
        text(dc, &row.name, nr, TEXT, name_font, DT_LEFT | DT_VCENTER);
        let dr = RECT { left, top: top + px(24, dpi), right: text_right, bottom: top + px(42, dpi) };
        text(dc, &row.detail, dr, MUTED, small_font, DT_LEFT | DT_VCENTER);
    }
    DeleteObject(name_font as HGDIOBJ);
    DeleteObject(small_font as HGDIOBJ);
    DeleteObject(pct_font as HGDIOBJ);

    BitBlt(hdc, 0, 0, w, h, dc, 0, 0, SRCCOPY);
    SelectObject(dc, old_bmp);
    DeleteObject(bmp as HGDIOBJ);
    DeleteDC(dc);
    EndPaint(hwnd, &ps);
}
