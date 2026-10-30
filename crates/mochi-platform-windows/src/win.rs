//! Win32 implementation: UI Automation for perception, GDI for screenshots, SendInput for
//! actions. Every function here runs on a blocking thread with its own COM init.

use crate::common::*;
use crate::common::{VK_MENU, VK_SHIFT};
use base64::Engine;
use mochi_core::types::*;
use std::thread::sleep;
use std::time::Duration;
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const MAX_VISITED: usize = 2500;
const MAX_DEPTH: usize = 30;
const MAX_ELEMENTS: usize = 300;
const SCREENSHOT_MAX_W: usize = 1280;

fn perr(e: impl std::fmt::Display) -> MochiError {
    MochiError::Perception(e.to_string())
}
fn aerr(e: impl std::fmt::Display) -> MochiError {
    MochiError::Action(e.to_string())
}

fn com() {
    // Ref-counted per thread; S_FALSE on repeat is fine.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn automation() -> Result<IUIAutomation> {
    com();
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).map_err(perr) }
}

// ---------- windows & monitors ----------

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn window_app_id(hwnd: HWND) -> String {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
    if pid == 0 {
        return String::new();
    }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(h);
        if !ok {
            return String::new();
        }
        app_id_from_path(&String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn window_rect(hwnd: HWND) -> Bounds {
    let mut r = RECT::default();
    unsafe { let _ = GetWindowRect(hwnd, &mut r); };
    rect_to_bounds(r.left, r.top, r.right, r.bottom)
}

pub fn monitors() -> Vec<MonitorInfo> {
    unsafe extern "system" fn cb(hmon: HMONITOR, _hdc: HDC, _rc: *mut RECT, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<MonitorInfo>) };
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if unsafe { GetMonitorInfoW(hmon, &mut mi) }.as_bool() {
            let r = mi.rcMonitor;
            out.push(MonitorInfo {
                id: out.len() as u32,
                bounds: rect_to_bounds(r.left, r.top, r.right, r.bottom),
                scale: 1.0,
                is_primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
                has_cursor: false,
            });
        }
        BOOL(1)
    }
    let mut v: Vec<MonitorInfo> = vec![];
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize)); };
    let mut p = POINT::default();
    unsafe { let _ = GetCursorPos(&mut p); };
    for m in v.iter_mut() {
        m.has_cursor = m.bounds.contains(p.x as f64, p.y as f64);
    }
    v
}

fn monitor_id_for(hwnd: HWND, mons: &[MonitorInfo]) -> u32 {
    let b = window_rect(hwnd);
    let (cx, cy) = b.center();
    mons.iter().find(|m| m.bounds.contains(cx, cy)).or_else(|| mons.iter().find(|m| m.is_primary)).map(|m| m.id).unwrap_or(0)
}

struct TopWindow {
    hwnd: HWND,
    app_id: String,
    title: String,
}

fn top_windows() -> Vec<TopWindow> {
    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<TopWindow>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            let title = window_title(hwnd);
            if !title.is_empty() {
                out.push(TopWindow { hwnd, app_id: window_app_id(hwnd), title });
            }
        }
        BOOL(1)
    }
    let mut v: Vec<TopWindow> = vec![];
    unsafe { let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize)); };
    v
}

// ---------- perception ----------

struct Walk<'a> {
    walker: &'a IUIAutomationTreeWalker,
    out: Vec<UiElement>,
    visited: usize,
    next_id: u32,
    clip: Bounds,
}

impl Walk<'_> {
    fn visit(&mut self, el: &IUIAutomationElement, depth: usize) {
        if self.visited >= MAX_VISITED || depth > MAX_DEPTH {
            return;
        }
        self.visited += 1;
        if let Some(e) = self.describe(el) {
            self.out.push(e);
        }
        let mut child = unsafe { self.walker.GetFirstChildElement(el) }.ok();
        while let Some(c) = child {
            self.visit(&c, depth + 1);
            if self.visited >= MAX_VISITED {
                break;
            }
            child = unsafe { self.walker.GetNextSiblingElement(&c) }.ok();
        }
    }

    fn describe(&mut self, el: &IUIAutomationElement) -> Option<UiElement> {
        unsafe {
            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(true) {
                return None;
            }
            let r = el.CurrentBoundingRectangle().ok()?;
            let bounds = rect_to_bounds(r.left, r.top, r.right, r.bottom);
            if bounds.w < 1.0 || bounds.h < 1.0 || bounds.overlap_ratio(&self.clip) == 0.0 && !self.clip.contains(bounds.x, bounds.y) {
                return None;
            }
            let ct = el.CurrentControlType().map(|c| c.0).unwrap_or(0);
            let role = role_name(ct);
            if matches!(role, "scrollbar" | "thumb" | "separator" | "titlebar") {
                return None;
            }
            let label = el.CurrentName().map(|b| b.to_string()).unwrap_or_default();
            let focusable = el.CurrentIsKeyboardFocusable().map(|b| b.as_bool()).unwrap_or(false);
            let enabled = el.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(true);
            let interactive = enabled && (role_is_interactive(role) || (focusable && role != "pane" && role != "window" && role != "text"));
            let mut value = String::new();
            if role_has_value(role) {
                if let Ok(vp) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                    value = vp.CurrentValue().map(|b| b.to_string()).unwrap_or_default();
                }
            }
            let id = self.next_id;
            self.next_id += 1;
            Some(UiElement {
                id,
                role: role.to_string(),
                label,
                value,
                bounds,
                interactive,
                focused: el.CurrentHasKeyboardFocus().map(|b| b.as_bool()).unwrap_or(false),
                secure: el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false),
            })
        }
    }
}

/// The window Mochi should look at: the foreground window, unless that is Mochi itself
/// (the user just clicked its panel or bubble), in which case the topmost other app
/// window is brought forward so keyboard input lands there too.
fn target_window() -> Result<HWND> {
    let own_pid = std::process::id();
    let pid_of = |hwnd: HWND| { let mut pid = 0u32; unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) }; pid };
    let fg = unsafe { GetForegroundWindow() };
    if !fg.0.is_null() && pid_of(fg) != own_pid {
        return Ok(fg);
    }
    // Z-order walk: EnumWindows enumerates top-level windows front to back.
    let candidates = top_windows();
    let next = candidates.into_iter().find(|w| {
        pid_of(w.hwnd) != own_pid
            && !unsafe { IsIconic(w.hwnd) }.as_bool()
            && !w.app_id.is_empty()
            && !matches!(w.app_id.as_str(), "explorer.exe" | "textinputhost.exe" | "shellexperiencehost.exe" | "searchhost.exe" | "startmenuexperiencehost.exe")
    });
    let Some(w) = next else { return Err(perr("no app window to work in (open the app you want Mochi to use)")) };
    unsafe {
        let _ = send(&[key(VK_MENU, 0, KEYBD_EVENT_FLAGS(0)), key(VK_MENU, 0, KEYEVENTF_KEYUP)]);
        let _ = SetForegroundWindow(w.hwnd);
    }
    sleep(Duration::from_millis(200));
    Ok(w.hwnd)
}

pub fn snapshot(want_screenshot: bool) -> Result<ScreenSnapshot> {
    let ui = automation()?;
    let hwnd = target_window()?;
    let mons = monitors();
    let title = window_title(hwnd);
    let window = WindowInfo {
        app_id: window_app_id(hwnd),
        title: title.clone(),
        bounds: window_rect(hwnd),
        monitor_id: monitor_id_for(hwnd, &mons),
        unsaved: unsaved_from_title(&title),
    };
    let root = unsafe { ui.ElementFromHandle(hwnd) }.map_err(perr)?;
    let walker = unsafe { ui.ControlViewWalker() }.map_err(perr)?;
    let mut walk = Walk { walker: &walker, out: vec![], visited: 0, next_id: 1, clip: window.bounds };
    walk.visit(&root, 0);
    let elements = trim_elements(walk.out, MAX_ELEMENTS);
    let thin = elements.len() < 5;
    let screenshot_png_b64 = if want_screenshot || thin { capture_png_b64(&window.bounds).ok() } else { None };
    Ok(ScreenSnapshot { window, monitors: mons, elements, remembered: vec![], screenshot_png_b64 })
}

/// The element under the cursor, merged into a fresh snapshot of the foreground window.
pub fn element_under_cursor() -> Result<Option<(ScreenSnapshot, u32)>> {
    let ui = automation()?;
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p) }.map_err(perr)?;
    let el = match unsafe { ui.ElementFromPoint(p) } {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    let r = unsafe { el.CurrentBoundingRectangle() }.map_err(perr)?;
    let b = rect_to_bounds(r.left, r.top, r.right, r.bottom);
    let name = unsafe { el.CurrentName() }.map(|s| s.to_string()).unwrap_or_default();
    let role = role_name(unsafe { el.CurrentControlType() }.map(|c| c.0).unwrap_or(0));
    let mut snap = snapshot(false)?;
    let found = snap.elements.iter().find(|e| e.bounds.overlap_ratio(&b) > 0.9 && (e.label == name || name.is_empty())).map(|e| e.id);
    let id = match found {
        Some(id) => id,
        None => {
            let id = snap.elements.iter().map(|e| e.id).max().unwrap_or(0) + 1;
            snap.elements.push(UiElement { id, role: role.into(), label: name, value: String::new(), bounds: b, interactive: true, focused: false, secure: false });
            id
        }
    };
    Ok(Some((snap, id)))
}

// ---------- screenshot ----------

fn capture_png_b64(b: &Bounds) -> Result<String> {
    let (x, y, w, h) = (b.x as i32, b.y as i32, b.w as i32, b.h as i32);
    if w <= 0 || h <= 0 {
        return Err(perr("empty window"));
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let blit = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY);
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);
        blit.map_err(perr)?;
        if lines == 0 {
            return Err(perr("GetDIBits failed"));
        }
        let (nw, nh, rgba) = downscale_bgra_to_rgba(&pixels, w as usize, h as usize, SCREENSHOT_MAX_W);
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, nw as u32, nh as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().map_err(perr)?;
            wr.write_image_data(&rgba).map_err(perr)?;
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(png_bytes))
    }
}

// ---------- actions ----------

fn send(inputs: &[INPUT]) -> Result<()> {
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != inputs.len() {
        return Err(aerr(format!("SendInput sent {n} of {}", inputs.len())));
    }
    Ok(())
}

fn mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS, data: u32) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn move_mouse(x: f64, y: f64) -> Result<()> {
    let (vx, vy, vw, vh) = unsafe {
        (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
    };
    send(&[mouse(to_absolute(x, vx, vw), to_absolute(y, vy, vh), MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, 0)])
}

fn click_at(x: f64, y: f64) -> Result<()> {
    move_mouse(x, y)?;
    sleep(Duration::from_millis(40));
    send(&[mouse(0, 0, MOUSEEVENTF_LEFTDOWN, 0)])?;
    sleep(Duration::from_millis(30));
    send(&[mouse(0, 0, MOUSEEVENTF_LEFTUP, 0)])
}

fn type_text(text: &str) -> Result<()> {
    for ch in text.chars() {
        if ch == '\n' {
            send(&[key(0x0D, 0, KEYBD_EVENT_FLAGS(0)), key(0x0D, 0, KEYEVENTF_KEYUP)])?;
            continue;
        }
        let mut units = [0u16; 2];
        for u in ch.encode_utf16(&mut units) {
            send(&[key(0, *u, KEYEVENTF_UNICODE), key(0, *u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)])?;
        }
        sleep(Duration::from_millis(6));
    }
    Ok(())
}

fn press_combo(combo: &str) -> Result<()> {
    let (mods, main) = parse_combo(combo).ok_or_else(|| aerr(format!("unknown key combo {combo:?}")))?;
    let (vk, extra_shift) = match main {
        KeySpec::Vk(v) => (v, false),
        KeySpec::Char(c) => {
            let r = unsafe { VkKeyScanW(c as u16) };
            if r == -1 {
                return Err(aerr(format!("no key for {c:?}")));
            }
            ((r & 0xFF) as u16, (r >> 8) & 1 == 1 && !mods.contains(&VK_SHIFT))
        }
    };
    let mut seq = vec![];
    for m in &mods {
        seq.push(key(*m, 0, KEYBD_EVENT_FLAGS(0)));
    }
    if extra_shift {
        seq.push(key(VK_SHIFT, 0, KEYBD_EVENT_FLAGS(0)));
    }
    seq.push(key(vk, 0, KEYBD_EVENT_FLAGS(0)));
    seq.push(key(vk, 0, KEYEVENTF_KEYUP));
    if extra_shift {
        seq.push(key(VK_SHIFT, 0, KEYEVENTF_KEYUP));
    }
    for m in mods.iter().rev() {
        seq.push(key(*m, 0, KEYEVENTF_KEYUP));
    }
    send(&seq)
}

fn focus_window(app_id: &str, title_contains: Option<&str>) -> Result<()> {
    let wins = top_windows();
    let want_title = title_contains.unwrap_or("").to_lowercase();
    let hit = wins
        .iter()
        .filter(|w| app_matches(app_id, &w.app_id, &w.title))
        .find(|w| want_title.is_empty() || w.title.to_lowercase().contains(&want_title))
        .or_else(|| wins.iter().find(|w| w.title.to_lowercase().contains(&want_title) && !want_title.is_empty()))
        .ok_or_else(|| aerr(format!("no window for {app_id} {title_contains:?}")))?;
    unsafe {
        if IsIconic(hit.hwnd).as_bool() {
            let _ = ShowWindow(hit.hwnd, SW_RESTORE);
        }
        // Windows refuses foreground changes from background processes unless a key was
        // recently pressed; tapping ALT is the long-standing workaround.
        let _ = send(&[key(VK_MENU, 0, KEYBD_EVENT_FLAGS(0)), key(VK_MENU, 0, KEYEVENTF_KEYUP)]);
        if !SetForegroundWindow(hit.hwnd).as_bool() {
            return Err(aerr("SetForegroundWindow refused"));
        }
    }
    sleep(Duration::from_millis(250));
    Ok(())
}

pub fn perform(action: &Action, screen: &ScreenSnapshot) -> Result<()> {
    match action {
        Action::FocusWindow { app_id, title_contains } => focus_window(app_id, title_contains.as_deref()),
        Action::ClickElement { element_id } => {
            let el = screen.element(*element_id).ok_or_else(|| aerr(format!("no element #{element_id}")))?;
            let (x, y) = el.bounds.center();
            click_at(x, y)
        }
        Action::ClickPoint { x, y } => click_at(*x, *y),
        Action::Type { text } => type_text(text),
        Action::Key { combo } => press_combo(combo),
        Action::Scroll { element_id, direction, amount } => {
            if let Some(el) = screen.element(*element_id) {
                let (x, y) = el.bounds.center();
                move_mouse(x, y)?;
            }
            let (flags, delta) = match direction {
                ScrollDirection::Up => (MOUSEEVENTF_WHEEL, 120i32),
                ScrollDirection::Down => (MOUSEEVENTF_WHEEL, -120i32),
                ScrollDirection::Left => (MOUSEEVENTF_HWHEEL, -120i32),
                ScrollDirection::Right => (MOUSEEVENTF_HWHEEL, 120i32),
            };
            for _ in 0..(*amount).max(1) {
                send(&[mouse(0, 0, flags, delta as u32)])?;
                sleep(Duration::from_millis(30));
            }
            Ok(())
        }
    }
}
