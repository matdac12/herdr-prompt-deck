//! A native, always-on-top scratchpad window, launched by the deck on Windows.
//!
//! Runs as its own process (`prompt-deck window --target <pane>`) so its message
//! loop never blocks the TUI. `Ctrl+Enter` or the button sends the text straight
//! to the target pane with `herdr pane send-text` — no focus round-trip through a
//! third-party editor. The buffer is persisted to `scratch.md` on send and close.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateFontW, UpdateWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
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
const FCONTROL: u8 = 0x08;
const FVIRTKEY: u8 = 0x01;

const EM_REPLACESEL: u32 = 0x00C2;

const ID_SEND: usize = 1;
const ID_CLEAR: usize = 2;
const ID_EDIT: usize = 3;
const ID_FILE: usize = 4;

struct EditorState {
    target: String,
    edit: HWND,
    send: HWND,
    file: HWND,
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
            st.clear = clear;
        }
    });
    0
}

fn on_size(hwnd: HWND) {
    let mut rc: windows_sys::Win32::Foundation::RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };

    let (edit, send, file, clear) = STATE.with(|s| {
        let b = s.borrow();
        match b.as_ref() {
            Some(st) => (st.edit, st.send, st.file, st.clear),
            None => (null_mut(), null_mut(), null_mut(), null_mut()),
        }
    });

    let margin = 8;
    let btn_h = 32;
    let send_w = 200;
    let file_w = 150;
    let clear_w = 90;
    let height = rc.bottom - rc.top;
    let width = rc.right - rc.left;
    let edit_h = (height - margin * 2 - btn_h - 6).max(20);

    unsafe {
        MoveWindow(edit, margin, margin, width - margin * 2, edit_h, 1);
        let y = height - margin - btn_h;
        MoveWindow(send, margin, y, send_w, btn_h, 1);
        MoveWindow(file, margin + send_w + 8, y, file_w, btn_h, 1);
        MoveWindow(clear, margin + send_w + file_w + 16, y, clear_w, btn_h, 1);
    }
}

fn on_command(hwnd: HWND, wparam: WPARAM) {
    let id = wparam & 0xFFFF;
    let code = ((wparam >> 16) & 0xFFFF) as u16;
    match id {
        ID_SEND => send_to_agent(hwnd),
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

/// Open the native file dialog and insert the chosen path at the caret.
fn insert_file_path() {
    let Some((edit, target)) = with_state(|st| (st.edit, st.target.clone())) else {
        return;
    };
    let mut dialog = rfd::FileDialog::new().set_title("Select a file to insert");
    if let Some(dir) = crate::pane_cwd(&target) {
        dialog = dialog.set_directory(dir);
    }
    let Some(path) = dialog.pick_file() else {
        return;
    };
    let insertion = wide(&format!("{} ", path.to_string_lossy()));
    unsafe { SendMessageW(edit, EM_REPLACESEL, 1, insertion.as_ptr() as LPARAM) };
    unsafe { SetFocus(edit) };
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
