//! Native Windows window: draws the calculator skin with the live LCD, and
//! turns mouse clicks and the PC keyboard into key events for the emulator.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use minifb::{InputCallback, Key, KeyRepeat, MouseButton, MouseMode, Scale, ScaleMode, Window, WindowOptions};

use crate::Shared;

#[allow(dead_code)]
mod layout {
    include!("../res/skin/layout.rs");
}
use layout::*;

pub const TITLE: &str = "fx-9860GII Emulator";

static SKIN: &[u8] = include_bytes!("../res/skin/skin.rgb");
static TURBO_ON: &[u8] = include_bytes!("../res/skin/turbo_on.rgb");
static MSG_OFF: &[u8] = include_bytes!("../res/skin/msg_off.rgb");
static MSG_SHOT: &[u8] = include_bytes!("../res/skin/msg_shot.rgb");
static MSG_TURBO: &[u8] = include_bytes!("../res/skin/msg_turbo.rgb");
static MSG_HELP: &[u8] = include_bytes!("../res/skin/msg_help.rgb");

fn rgb_to_u32(rgb: &[u8]) -> Vec<u32> {
    rgb.chunks_exact(3).map(|p| (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect()
}

fn blit(buf: &mut [u32], src: &[u32], x0: usize, y0: usize, w: usize) {
    for (row, line) in src.chunks_exact(w).enumerate() {
        let o = (y0 + row) * W + x0;
        buf[o..o + w].copy_from_slice(line);
    }
}

/// Lighten a rectangle (pressed key).
fn highlight(buf: &mut [u32], x0: usize, y0: usize, x1: usize, y1: usize) {
    for y in y0..y1.min(H) {
        for x in x0..x1.min(W) {
            let p = buf[y * W + x];
            let f = |c: u32| c + (255 - c) * 35 / 100;
            buf[y * W + x] = f(p >> 16 & 255) << 16 | f(p >> 8 & 255) << 8 | f(p & 255);
        }
    }
}

/// Characters typed on the PC keyboard (layout-independent).
struct Chars(Arc<Mutex<Vec<char>>>);
impl InputCallback for Chars {
    fn add_char(&mut self, c: u32) {
        if let Some(ch) = char::from_u32(c) {
            self.0.lock().unwrap().push(ch);
        }
    }
}

fn char_key(c: char) -> Option<u8> {
    Some(match c {
        '0' => 0x11, '1' => 0x21, '2' => 0x22, '3' => 0x23, '4' => 0x31,
        '5' => 0x32, '6' => 0x33, '7' => 0x41, '8' => 0x42, '9' => 0x43,
        '.' => 0x12, ',' => 0x55, '+' => 0x24, '-' => 0x25, '*' => 0x34, '/' => 0x35,
        '(' => 0x53, ')' => 0x54, '^' => 0x73, 'e' | 'E' => 0x13, 'x' | 'X' => 0x61,
        's' | 'S' => 0x64, 'c' | 'C' => 0x65, 't' | 'T' => 0x66, 'l' | 'L' => 0x63,
        'm' | 'M' => 0x84, 'o' | 'O' => 0x82, 'v' | 'V' => 0x83,
        _ => return None,
    })
}

/// Keys that are held (not characters): sent as press and release.
fn held_key(k: Key) -> Option<u8> {
    Some(match k {
        Key::Enter | Key::NumPadEnter => 0x15,
        Key::Backspace => 0x44,
        Key::Escape => 0x74,
        Key::Up => 0x86,
        Key::Down => 0x75,
        Key::Left => 0x85,
        Key::Right => 0x76,
        Key::F1 => 0x91,
        Key::F2 => 0x92,
        Key::F3 => 0x93,
        Key::F4 => 0x94,
        Key::F5 => 0x95,
        Key::F6 => 0x96,
        Key::Tab => 0x71,
        Key::Home | Key::Delete => 0x07,
        _ => return None,
    })
}

// --- a little Win32 for dialogs, the window icon and finding the window ---

#[allow(non_snake_case)]
mod win {
    use std::ffi::c_void;
    pub type HWND = *mut c_void;
    #[link(name = "user32")]
    extern "system" {
        pub fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, kind: u32) -> i32;
        pub fn FindWindowW(class: *const u16, title: *const u16) -> HWND;
        pub fn SetForegroundWindow(hwnd: HWND) -> i32;
        pub fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
        pub fn LoadImageW(inst: *mut c_void, name: *const u16, kind: u32, cx: i32, cy: i32, flags: u32) -> *mut c_void;
        pub fn SendMessageW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize;
        pub fn SystemParametersInfoW(action: u32, param: u32, pv: *mut c_void, winini: u32) -> i32;
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

/// Ask a yes/no question in a normal Windows dialog.
fn ask(hwnd: win::HWND, text: &str) -> bool {
    let (t, c) = (win::wide(text), win::wide(TITLE));
    unsafe { win::MessageBoxW(hwnd, t.as_ptr(), c.as_ptr(), 0x24 /* MB_YESNO | MB_ICONQUESTION */) == 6 }
}

/// Bring an already running emulator window to the front. Returns false if none.
pub fn focus_existing() -> bool {
    let t = win::wide(TITLE);
    unsafe {
        let h = win::FindWindowW(std::ptr::null(), t.as_ptr());
        if h.is_null() {
            return false;
        }
        win::ShowWindow(h, 9 /* SW_RESTORE */);
        win::SetForegroundWindow(h);
    }
    true
}

fn set_icon(hwnd: win::HWND) {
    unsafe {
        let inst = win::GetModuleHandleW(std::ptr::null());
        // Icon resource 1 (see res/fxemu.rc); MAKEINTRESOURCE(1).
        let id = 1usize as *const u16;
        for (which, size) in [(1usize, 32), (0usize, 16)] {
            let icon = win::LoadImageW(inst, id, 1 /* IMAGE_ICON */, size, size, 0);
            if !icon.is_null() {
                win::SendMessageW(hwnd, 0x80 /* WM_SETICON */, which, icon as isize);
            }
        }
    }
}

// --- screenshots (PNG, no dependencies) ---

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn png_gray(w: usize, h: usize, pixels: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w + 1) * h);
    for row in pixels.chunks_exact(w) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib stream with stored (uncompressed) blocks
    let mut z = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        let n = chunk.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&(b << 16 | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit grayscale
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

fn save_screenshot(lcd: &[u8]) -> std::io::Result<std::path::PathBuf> {
    const S: usize = 4;
    let (w, h) = (128 * S, 64 * S);
    let mut px = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let on = lcd[(y / S) * 16 + (x / S) / 8] >> (7 - (x / S) % 8) & 1 != 0;
            px[y * w + x] = if on { 30 } else { 205 };
        }
    }
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    let mut dir = std::path::Path::new(&home).join("Pictures");
    if !dir.is_dir() {
        dir = std::path::PathBuf::from(&home);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut path = dir.join(format!("fx9860-screenshot-{}.png", secs));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("fx9860-screenshot-{}-{}.png", secs, n));
        n += 1;
    }
    std::fs::write(&path, png_gray(w, h, &px))?;
    Ok(path)
}

/// Initial window size: the full skin, shrunk if needed to fit the usable
/// screen height (keeping the aspect ratio). The window can be resized later.
fn initial_size() -> (usize, usize) {
    let mut r = win::Rect::default();
    let ok = unsafe { win::SystemParametersInfoW(0x30 /* SPI_GETWORKAREA */, 0, &mut r as *mut _ as *mut _, 0) };
    let avail = if ok != 0 { (r.bottom - r.top - 50).max(400) as usize } else { 900 };
    if avail >= H {
        (W, H)
    } else {
        (W * avail / H, avail)
    }
}

/// Run the window until it is closed. The emulator runs on another thread.
pub fn run(shared: Arc<Shared>) {
    let (win_w, win_h) = initial_size();
    let mut window = match Window::new(
        TITLE,
        win_w,
        win_h,
        WindowOptions { resize: true, scale: Scale::X1, scale_mode: ScaleMode::AspectRatioStretch, ..WindowOptions::default() },
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("cannot open window: {}", e);
            return;
        }
    };
    window.set_target_fps(60);
    window.set_background_color(17, 22, 28);
    let hwnd = window.get_window_handle() as win::HWND;
    set_icon(hwnd);

    let chars = Arc::new(Mutex::new(Vec::new()));
    window.set_input_callback(Box::new(Chars(chars.clone())));

    let skin = rgb_to_u32(SKIN);
    let turbo_img = rgb_to_u32(TURBO_ON);
    let msg_off = rgb_to_u32(MSG_OFF);
    let msg_shot = rgb_to_u32(MSG_SHOT);
    let msg_turbo = rgb_to_u32(MSG_TURBO);
    let msg_help = rgb_to_u32(MSG_HELP);
    let turbo_rect = TOOLS.iter().find(|t| t.0 == "turbo").copied().unwrap();

    let mut buf = vec![0u32; W * H];
    let mut mouse_key: Option<u8> = None;
    let mut mouse_was_down = false;
    let mut held: HashSet<u8> = HashSet::new(); // keys held via keyboard/mouse, for highlighting
    let mut flash_until: Vec<(u8, Instant)> = Vec::new(); // typed characters, briefly highlighted
    let mut shot_until = Instant::now();
    let mut turbo = false;

    let push = |code: u8, down: bool| shared.key_events.lock().unwrap().push_back((code, down));

    while window.is_open() && !shared.quit.load(Ordering::SeqCst) {
        // --- mouse ---
        let down = window.get_mouse_down(MouseButton::Left);
        let pos = window.get_unscaled_mouse_pos(MouseMode::Discard).and_then(|(mx, my)| {
            // Map window pixels to skin pixels (the skin is scaled to fit, centred).
            let (ww, wh) = window.get_size();
            let s = (ww as f32 / W as f32).min(wh as f32 / H as f32);
            let ox = (ww as f32 - W as f32 * s) / 2.0;
            let oy = (wh as f32 - H as f32 * s) / 2.0;
            let x = (mx - ox) / s;
            let y = (my - oy) / s;
            (x >= 0.0 && y >= 0.0 && (x as usize) < W && (y as usize) < H).then(|| (x as usize, y as usize))
        });
        if down && !mouse_was_down {
            if let Some((x, y)) = pos {
                if let Some(k) = KEYS.iter().find(|k| x >= k.2 && x < k.4 && y >= k.3 && y < k.5) {
                    push(k.0, true);
                    mouse_key = Some(k.0);
                    held.insert(k.0);
                } else if let Some(t) = TOOLS.iter().find(|t| x >= t.1 && x < t.3 && y >= t.2 && y < t.4) {
                    match t.0 {
                        "turbo" => {
                            turbo = !turbo;
                            shared.turbo.store(turbo, Ordering::SeqCst);
                        }
                        "shot" => {
                            let lcd = shared.lcd.lock().unwrap().clone();
                            if save_screenshot(&lcd).is_ok() {
                                shot_until = Instant::now() + Duration::from_secs(4);
                            }
                        }
                        "restart" => {
                            if ask(hwnd, "Restart the calculator?\n\nYour saved files are kept; unsaved work in the current app is lost.") {
                                shared.reset.store(true, Ordering::SeqCst);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if !down && mouse_was_down {
            if let Some(k) = mouse_key.take() {
                push(k, false);
                held.remove(&k);
            }
        }
        mouse_was_down = down;

        // --- keyboard: held keys ---
        for k in window.get_keys_pressed(KeyRepeat::No) {
            if let Some(code) = held_key(k) {
                push(code, true);
                held.insert(code);
            }
        }
        for k in window.get_keys_released() {
            if let Some(code) = held_key(k) {
                push(code, false);
                held.remove(&code);
            }
        }
        // --- keyboard: typed characters (press + release) ---
        for c in chars.lock().unwrap().drain(..) {
            if let Some(code) = char_key(c) {
                push(code, true);
                push(code, false);
                flash_until.push((code, Instant::now() + Duration::from_millis(150)));
            }
        }

        // --- draw ---
        buf.copy_from_slice(&skin);
        let lcd = shared.lcd.lock().unwrap().clone();
        for y in 0..64 {
            for x in 0..128 {
                let on = lcd[y * 16 + x / 8] >> (7 - x % 8) & 1 != 0;
                let c = if on { LCD_ON } else { LCD_OFF };
                for dy in 0..LCD_SCALE {
                    let o = (LCD_Y + y * LCD_SCALE + dy) * W + LCD_X + x * LCD_SCALE;
                    buf[o..o + LCD_SCALE].fill(c);
                }
            }
        }
        let now = Instant::now();
        flash_until.retain(|&(_, t)| t > now);
        for k in KEYS {
            if held.contains(&k.0) || flash_until.iter().any(|f| f.0 == k.0) {
                highlight(&mut buf, k.2, k.3, k.4, k.5);
            }
        }
        if turbo {
            blit(&mut buf, &turbo_img, turbo_rect.1, turbo_rect.2, turbo_rect.3 - turbo_rect.1);
        }
        let msg = if now < shot_until {
            &msg_shot
        } else if shared.off.load(Ordering::Relaxed) {
            &msg_off
        } else if turbo {
            &msg_turbo
        } else {
            &msg_help
        };
        blit(&mut buf, msg, STATUS_X, STATUS_Y, STATUS_W);

        if window.update_with_buffer(&buf, W, H).is_err() {
            break;
        }
    }
    // Release anything still held, then let the emulator save and stop.
    for k in held {
        push(k, false);
    }
    shared.quit.store(true, Ordering::SeqCst);
}
