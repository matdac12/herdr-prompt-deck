//! A native, always-on-top scratchpad window, launched by the deck on Windows.
//!
//! Runs as its own process (`prompt-deck window --target <pane>`) so its message
//! loop never blocks the TUI. `Ctrl+Enter` or the button sends the text straight
//! to the target pane with `herdr pane send-text` — no focus round-trip through a
//! third-party editor. The buffer is persisted to `scratch.md` on send and close.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateFontW, DIB_RGB_COLORS,
    DeleteDC, GetDC, GetDIBits, GetObjectW, HBITMAP, ReleaseDC, UpdateWindow,
};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateAcceleratorTableW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GetClientRect, GetMessageW, GetWindowTextLengthW, GetWindowTextW, LoadCursorW,
    MoveWindow, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowTextW,
    ShowWindow, TranslateAcceleratorW, TranslateMessage, ACCEL, CW_USEDEFAULT, IDC_ARROW, MSG,
    WNDCLASSW,
};

const DEFAULT_TITLE: &str = "Prompt Deck - scratchpad   (Ctrl+Enter sends)";

const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;

const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_NOHIDESEL: u32 = 0x0100;
const ES_WANTRETURN: u32 = 0x1000;
const EDIT_STYLE: u32 =
    WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP | ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL;

const BUTTON_STYLE: u32 = WS_CHILD | WS_VISIBLE | WS_TABSTOP;

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_CLOSE: u32 = 0x0010;
const WM_SETFONT: u32 = 0x0030;
const WM_COMMAND: u32 = 0x0111;
const EN_CHANGE: u16 = 0x0300;

const SW_SHOW: i32 = 5;
const VK_RETURN: u16 = 0x0D;
const VK_O: u16 = 0x4F;
const VK_V: u16 = 0x56;
const FCONTROL: u8 = 0x08;
const FVIRTKEY: u8 = 0x01;

const EM_SETSEL: u32 = 0x00B1;
const WM_PASTE: u32 = 0x0302;

const ID_SEND: usize = 1;
const ID_CLEAR: usize = 2;
const ID_EDIT: usize = 3;
const ID_FILE: usize = 4;
const ID_PASTE: usize = 5;

struct EditorState {
    target: String,
    edit: HWND,
    send: HWND,
    file: HWND,
    paste: HWND,
    clear: HWND,
    scratch: std::path::PathBuf,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<EditorState>> = const { std::cell::RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn run(target: &str) -> io::Result<()> {
    unsafe { run_window(target) }
}

unsafe fn run_window(target: &str) -> io::Result<()> {
    let hinst = unsafe { GetModuleHandleW(null()) };
    if hinst.is_null() {
        return Err(io::Error::last_os_error());
    }

    let class_name = wide("PromptDeckEditorWindow");

    // If a scratchpad is already open, just raise it rather than stacking windows.
    let existing = unsafe { FindWindowW(class_name.as_ptr(), null()) };
    if !existing.is_null() {
        unsafe { SetForegroundWindow(existing) };
        return Ok(());
    }

    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: null_mut(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: 6usize as _,
        lpszMenuName: null(),
        lpszClassName: class_name.as_ptr(),
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(io::Error::last_os_error());
    }

    STATE.with(|s| {
        *s.borrow_mut() = Some(EditorState {
            target: target.to_string(),
            edit: null_mut(),
            send: null_mut(),
            file: null_mut(),
            paste: null_mut(),
            clear: null_mut(),
            scratch: crate::scratch_path(),
        });
    });

    let title = wide(DEFAULT_TITLE);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            760,
            560,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        )
    };
    if hwnd.is_null() {
        return Err(io::Error::last_os_error());
    }

    let accels = [
        ACCEL {
            fVirt: FCONTROL | FVIRTKEY,
            key: VK_RETURN,
            cmd: ID_SEND as u16,
        },
        ACCEL {
            fVirt: FCONTROL | FVIRTKEY,
            key: VK_O,
            cmd: ID_FILE as u16,
        },
        ACCEL {
            fVirt: FCONTROL | FVIRTKEY,
            key: VK_V,
            cmd: ID_PASTE as u16,
        },
    ];
    let haccel = unsafe { CreateAcceleratorTableW(accels.as_ptr(), accels.len() as i32) };

    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetForegroundWindow(hwnd);
    }

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    loop {
        let ret = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if ret <= 0 {
            break;
        }
        if haccel.is_null() || unsafe { TranslateAcceleratorW(hwnd, haccel, &msg) } == 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => on_create(hwnd),
        WM_SIZE => {
            on_size(hwnd);
            0
        }
        WM_COMMAND => {
            on_command(hwnd, wparam);
            0
        }
        WM_CLOSE => {
            persist();
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn child(parent: HWND, hinst: HMODULE, class: &str, text: &str, style: u32, id: usize) -> HWND {
    let class = wide(class);
    let text = wide(text);
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            text.as_ptr(),
            style,
            0,
            0,
            10,
            10,
            parent,
            id as _,
            hinst,
            null(),
        )
    }
}

fn on_create(hwnd: HWND) -> LRESULT {
    let hinst = unsafe { GetModuleHandleW(null()) };
    let edit = child(hwnd, hinst, "EDIT", "", EDIT_STYLE, ID_EDIT);
    let send = child(hwnd, hinst, "BUTTON", "Send to agent  (Ctrl+Enter)", BUTTON_STYLE, ID_SEND);
    let paste = child(hwnd, hinst, "BUTTON", "Paste screenshot  (Ctrl+V)", BUTTON_STYLE, ID_PASTE);
    let file = child(hwnd, hinst, "BUTTON", "Insert file...  (Ctrl+O)", BUTTON_STYLE, ID_FILE);
    let clear = child(hwnd, hinst, "BUTTON", "Clear", BUTTON_STYLE, ID_CLEAR);

    let face = wide("Consolas");
    let font = unsafe { CreateFontW(-16, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 1 | 48, face.as_ptr()) };
    if !font.is_null() {
        unsafe { SendMessageW(edit, WM_SETFONT, font as usize, 1) };
    }

    let seed = std::fs::read_to_string(crate::scratch_path()).unwrap_or_default();
    let seed = wide(&seed);
    unsafe {
        SetWindowTextW(edit, seed.as_ptr());
        SetFocus(edit);
    }

    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.edit = edit;
            st.send = send;
            st.file = file;
            st.paste = paste;
            st.clear = clear;
        }
    });
    0
}

fn on_size(hwnd: HWND) {
    let mut rc: windows_sys::Win32::Foundation::RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };

    let (edit, send, file, paste, clear) = STATE.with(|s| {
        let b = s.borrow();
        match b.as_ref() {
            Some(st) => (st.edit, st.send, st.file, st.paste, st.clear),
            None => (null_mut(), null_mut(), null_mut(), null_mut(), null_mut()),
        }
    });

    let margin = 8;
    let btn_h = 32;
    let send_w = 190;
    let paste_w = 190;
    let file_w = 140;
    let clear_w = 80;
    let height = rc.bottom - rc.top;
    let width = rc.right - rc.left;
    let edit_h = (height - margin * 2 - btn_h - 6).max(20);

    unsafe {
        MoveWindow(edit, margin, margin, width - margin * 2, edit_h, 1);
        let y = height - margin - btn_h;
        MoveWindow(send, margin, y, send_w, btn_h, 1);
        MoveWindow(paste, margin + send_w + 6, y, paste_w, btn_h, 1);
        MoveWindow(file, margin + send_w + paste_w + 12, y, file_w, btn_h, 1);
        MoveWindow(clear, margin + send_w + paste_w + file_w + 18, y, clear_w, btn_h, 1);
    }
}

fn on_command(hwnd: HWND, wparam: WPARAM) {
    let id = wparam & 0xFFFF;
    let code = ((wparam >> 16) & 0xFFFF) as u16;
    match id {
        ID_SEND => send_to_agent(hwnd),
        ID_PASTE => paste_clipboard(hwnd),
        ID_FILE => insert_file_path(),
        ID_CLEAR => clear_edit(),
        ID_EDIT if code == EN_CHANGE => reset_title(hwnd),
        _ => {}
    }
}

fn edit_text(edit: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(edit) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = unsafe { GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

fn with_state<R>(f: impl FnOnce(&EditorState) -> R) -> Option<R> {
    STATE.with(|s| s.borrow().as_ref().map(f))
}

fn send_to_agent(hwnd: HWND) {
    let Some((text, target, scratch)) = with_state(|st| {
        (edit_text(st.edit), st.target.clone(), st.scratch.clone())
    }) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let _ = std::fs::write(&scratch, &text);
    crate::send_text(&target, &text);
    let title = wide("Prompt Deck - sent to agent");
    unsafe { SetWindowTextW(hwnd, title.as_ptr()) };
}

/// Append text to the buffer, then place the caret at the end. Uses
/// get + `SetWindowText`, which works even when the window isn't focused.
fn insert_text_at_caret(text: &str) {
    if let Some(edit) = with_state(|st| st.edit) {
        let updated = format!("{}{text}", edit_text(edit));
        let buf = wide(&updated);
        unsafe {
            SetWindowTextW(edit, buf.as_ptr());
            let len = GetWindowTextLengthW(edit) as usize;
            SendMessageW(edit, EM_SETSEL, len, len as isize);
            SetFocus(edit);
        }
    }
}

/// Open the native file dialog and insert the chosen path at the caret.
fn insert_file_path() {
    let Some(target) = with_state(|st| st.target.clone()) else {
        return;
    };
    let mut dialog = rfd::FileDialog::new().set_title("Select a file to insert");
    if let Some(dir) = crate::pane_cwd(&target) {
        dialog = dialog.set_directory(dir);
    }
    let Some(path) = dialog.pick_file() else {
        return;
    };
    insert_text_at_caret(&format!("{} ", path.to_string_lossy()));
}

/// `Ctrl+V`: if the clipboard holds an image (e.g. a Win+Shift+S snip), save it as
/// a PNG and insert the path; otherwise fall back to a normal text paste.
fn paste_clipboard(hwnd: HWND) {
    let Some((width, height, rgba)) = clipboard_image() else {
        if let Some(edit) = with_state(|st| st.edit) {
            unsafe { SendMessageW(edit, WM_PASTE, 0, 0) };
        }
        return;
    };

    let path = crate::screenshot_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match save_png(&path, width, height, &rgba) {
        Ok(()) => {
            insert_text_at_caret(&format!("{} ", path.display()));
            reset_title(hwnd);
        }
        Err(_) => {
            let title = wide("Prompt Deck - could not save screenshot");
            unsafe { SetWindowTextW(hwnd, title.as_ptr()) };
        }
    }
}

const CF_DIB: u32 = 8;
const CF_BITMAP: u32 = 2;

/// Copy the clipboard's bitmap into RGBA8. Tries `CF_DIB` (what screenshots use),
/// then `CF_BITMAP`. Returns `(width, height, rgba)`.
fn clipboard_image() -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return None;
        }
        let result = (|| {
            let handle = GetClipboardData(CF_DIB);
            if !handle.is_null() {
                let ptr = GlobalLock(handle);
                if !ptr.is_null() {
                    let size = GlobalSize(handle);
                    let bytes = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
                    GlobalUnlock(handle);
                    if let Some(image) = dib_to_rgba(&bytes) {
                        return Some(image);
                    }
                }
            }
            let hbitmap = GetClipboardData(CF_BITMAP);
            if hbitmap.is_null() {
                return None;
            }
            hbitmap_to_rgba(hbitmap as HBITMAP)
        })();
        CloseClipboard();
        result
    }
}

/// Extract a top-down 32-bit RGBA buffer from an `HBITMAP` via `GetDIBits`.
unsafe fn hbitmap_to_rgba(hbitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    let mut bitmap: BITMAP = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        GetObjectW(
            hbitmap,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bitmap as *mut BITMAP as *mut std::ffi::c_void,
        )
    };
    if ok == 0 {
        return None;
    }
    let width = bitmap.bmWidth;
    let height = bitmap.bmHeight;
    if width <= 0 || height <= 0 {
        return None;
    }

    let screen = unsafe { GetDC(null_mut()) };
    let mem = unsafe { CreateCompatibleDC(screen) };
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;

    let mut bytes = vec![0u8; (width as usize) * (height as usize) * 4];
    let lines = unsafe {
        GetDIBits(
            mem,
            hbitmap,
            0,
            height as u32,
            bytes.as_mut_ptr() as *mut std::ffi::c_void,
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    unsafe { DeleteDC(mem) };
    unsafe { ReleaseDC(null_mut(), screen) };
    if lines == 0 {
        return None;
    }

    for px in bytes.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    Some((width as u32, height as u32, bytes))
}

/// Convert a packed `BITMAPINFOHEADER` + pixel data blob (BI_RGB, 24/32bpp) to RGBA.
fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    fn u16_at(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
    fn i32_at(b: &[u8], o: usize) -> i32 {
        i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }

    if dib.len() < 40 {
        return None;
    }
    let header_size = u32_at(dib, 0) as usize;
    let width = i32_at(dib, 4);
    let height = i32_at(dib, 8);
    let planes = u16_at(dib, 12);
    let bpp = u16_at(dib, 14) as usize;
    let compression = u32_at(dib, 16);
    if width <= 0 || height == 0 || planes != 1 || compression != 0 || (bpp != 24 && bpp != 32) {
        return None;
    }

    let w = width as usize;
    let h = height.unsigned_abs() as usize;
    let bottom_up = height > 0;
    let stride = (w * bpp).div_ceil(32) * 4;
    let pixel_offset = header_size;
    if pixel_offset + stride * h > dib.len() {
        return None;
    }

    let bytes_per_pixel = bpp / 8;
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_row = if bottom_up { h - 1 - y } else { y };
        let src = pixel_offset + src_row * stride;
        let dst_row = y * w * 4;
        for x in 0..w {
            let p = src + x * bytes_per_pixel;
            let d = dst_row + x * 4;
            rgba[d] = dib[p + 2];
            rgba[d + 1] = dib[p + 1];
            rgba[d + 2] = dib[p];
            rgba[d + 3] = 255;
        }
    }
    Some((w as u32, h as u32, rgba))
}

fn save_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(rgba).map_err(io::Error::other)?;
    Ok(())
}

fn clear_edit() {
    if let Some(edit) = with_state(|st| st.edit) {
        let empty = wide("");
        unsafe { SetWindowTextW(edit, empty.as_ptr()) };
        unsafe { SetFocus(edit) };
    }
}

fn persist() {
    if let Some((text, scratch)) = with_state(|st| (edit_text(st.edit), st.scratch.clone())) {
        let _ = std::fs::write(&scratch, &text);
    }
}

fn reset_title(hwnd: HWND) {
    let title = wide(DEFAULT_TITLE);
    unsafe { SetWindowTextW(hwnd, title.as_ptr()) };
}
