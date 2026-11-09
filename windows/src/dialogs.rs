//! Small modal windows: the albums to file photos in, and a line of text
//! (a new album's name, a new name for one). Each runs its own message
//! loop until closed, with the owner disabled, as a dialog box does.

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::{child, count, message_font, offscreen, scale, wide};

const CLASS: PCWSTR = w!("retifs-photos-dialog");
const ID_NEW: i32 = 300;
const ID_LIST: i32 = 301;
const ID_EDIT: i32 = 302;

/// What the window's procedure needs (kept in its GWLP_USERDATA).
#[derive(Default)]
struct Dialog {
    done: bool,
    ok: bool,
}

pub unsafe fn register(base: &WNDCLASSW) {
    let class = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: CLASS, ..*base };
    RegisterClassW(&class);
}

/// A window `w` × `h` (at 96 dpi) in the middle of `owner`.
unsafe fn window(owner: HWND, title: &str, w: i32, h: i32) -> Option<HWND> {
    let s = |v| scale(owner, v);
    let mut rc = RECT::default();
    let _ = GetWindowRect(owner, &mut rc);
    let (x, y) = (rc.left + (rc.right - rc.left - s(w)) / 2, rc.top + (rc.bottom - rc.top - s(h)) / 2);
    CreateWindowExW(WS_EX_DLGMODALFRAME, CLASS, &wide(title), WS_POPUP | WS_CAPTION | WS_SYSMENU, x, y, s(w), s(h), owner, None, GetModuleHandleW(None).unwrap_or_default(), None).ok()
}

/// Runs `hwnd` until OK or Cancel; true for OK. The window is still there
/// after (to read its fields), for the caller to destroy.
unsafe fn run(owner: HWND, hwnd: HWND) -> bool {
    let data = Box::into_raw(Box::new(Dialog::default()));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, data as isize);
    let _ = EnableWindow(owner, false);
    let _ = ShowWindow(hwnd, if offscreen() { SW_SHOWNOACTIVATE } else { SW_SHOW });
    let mut msg = MSG::default();
    while !(*data).done {
        if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Quitting: the main loop gets it too.
            PostQuitMessage(msg.wParam.0 as i32);
            break;
        }
        if IsDialogMessageW(hwnd, &msg).as_bool() {
            continue;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    let ok = Box::from_raw(data).ok;
    let _ = EnableWindow(owner, true);
    if !offscreen() {
        let _ = SetForegroundWindow(owner);
    }
    ok
}

unsafe fn buttons(hwnd: HWND, font: HFONT, w: i32, y: i32, ok: &str) {
    let s = |v| scale(hwnd, v);
    let okb = child(hwnd, WC_BUTTONW, ok, WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32), IDOK.0, font);
    let cancel = child(hwnd, WC_BUTTONW, "Cancel", WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32), IDCANCEL.0, font);
    let _ = MoveWindow(okb, s(w - 206), s(y), s(90), s(28), true);
    let _ = MoveWindow(cancel, s(w - 110), s(y), s(90), s(28), true);
}

unsafe fn text_of(h: HWND) -> String {
    let len = GetWindowTextLengthW(h);
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(h, &mut buf);
    String::from_utf16_lossy(&buf[..n as usize])
}

/// A line of text, `initial` to start with; none if cancelled.
pub unsafe fn ask_text(owner: HWND, title: &str, prompt: &str, initial: &str) -> Option<String> {
    let hwnd = window(owner, title, 400, 170)?;
    let font = message_font(hwnd);
    let s = |v| scale(hwnd, v);
    let label = child(hwnd, WC_STATICW, prompt, WS_VISIBLE, 0, font);
    let _ = MoveWindow(label, s(14), s(12), s(360), s(22), true);
    let edit = child(hwnd, WC_EDITW, initial, WS_VISIBLE | WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), ID_EDIT, font);
    let _ = MoveWindow(edit, s(14), s(40), s(356), s(26), true);
    buttons(hwnd, font, 384, 82, "OK");
    SendMessageW(edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
    let _ = SetFocus(edit);
    let ok = run(owner, hwnd);
    let text = text_of(edit);
    let _ = DestroyWindow(hwnd);
    let _ = DeleteObject(font);
    ok.then_some(text)
}

/// The albums to put `n` photos in (or take them out of): `albums` are
/// (name, photos in it, how many of these are). Checked: all of them are
/// in it. What changed: (albums to add them to, albums to take them out of).
pub unsafe fn pick_albums(owner: HWND, albums: &[(String, usize, usize)], n: usize) -> Option<(Vec<String>, Vec<String>)> {
    let hwnd = window(owner, "Add to albums", 420, 470)?;
    let font = message_font(hwnd);
    let s = |v| scale(hwnd, v);
    let intro = child(hwnd, WC_STATICW, &format!("Albums for the {} chosen: a photo can be in any number of albums, whichever library it's in.", count(n, "photo", "photos")), WS_VISIBLE, 0, font);
    let _ = MoveWindow(intro, s(14), s(10), s(380), s(40), true);
    let list = child(hwnd, WC_LISTVIEWW, "", WS_VISIBLE | WS_BORDER | WS_TABSTOP | WINDOW_STYLE((LVS_REPORT | LVS_NOCOLUMNHEADER | LVS_SINGLESEL | LVS_SHOWSELALWAYS) as u32), ID_LIST, font);
    SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, WPARAM(0), LPARAM((LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize));
    let _ = MoveWindow(list, s(14), s(56), s(376), s(300), true);
    for (i, (fmt, width)) in [(LVCFMT_LEFT, 250), (LVCFMT_RIGHT, 100)].iter().enumerate() {
        let col = LVCOLUMNW { mask: LVCF_WIDTH | LVCF_FMT, fmt: *fmt, cx: s(*width), ..Default::default() };
        SendMessageW(list, LVM_INSERTCOLUMNW, WPARAM(i), LPARAM(&col as *const _ as isize));
    }
    // (name, checked at first)
    let mut names: Vec<(String, bool)> = Vec::new();
    for (name, total, these) in albums {
        let note = if *these > 0 && *these < n { format!("{total} · {these} of these") } else { total.to_string() };
        add_row(list, names.len(), name, &note, *these == n);
        names.push((name.clone(), *these == n));
    }
    let new = child(hwnd, WC_BUTTONW, "New album…", WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32), ID_NEW, font);
    let _ = MoveWindow(new, s(14), s(368), s(120), s(28), true);
    buttons(hwnd, font, 404, 368, "OK");
    let _ = SetFocus(list);
    PICKER.with(|p| *p.borrow_mut() = Some((list, names)));
    let ok = run(owner, hwnd);
    let (list, names) = PICKER.with(|p| p.borrow_mut().take())?;
    let checked: Vec<bool> = (0..names.len()).map(|i| (SendMessageW(list, LVM_GETITEMSTATE, WPARAM(i), LPARAM(LVIS_STATEIMAGEMASK.0 as isize)).0 >> 12) == 2).collect();
    let _ = DestroyWindow(hwnd);
    let _ = DeleteObject(font);
    if !ok {
        return None;
    }
    let mut add = Vec::new();
    let mut remove = Vec::new();
    for ((name, was), now) in names.into_iter().zip(checked) {
        if now && !was {
            add.push(name);
        } else if was && !now {
            remove.push(name);
        }
    }
    Some((add, remove))
}

thread_local! {
    /// The album picker open now: its list and the names in it.
    static PICKER: std::cell::RefCell<Option<(HWND, Vec<(String, bool)>)>> = const { std::cell::RefCell::new(None) };
}

unsafe fn add_row(list: HWND, i: usize, name: &str, note: &str, checked: bool) {
    for (col, text) in [name, note].iter().enumerate() {
        let t = wide(text);
        let item = LVITEMW { mask: LVIF_TEXT, iItem: i as i32, iSubItem: col as i32, pszText: PWSTR(t.as_ptr() as *mut u16), ..Default::default() };
        SendMessageW(list, if col == 0 { LVM_INSERTITEMW } else { LVM_SETITEMTEXTW }, WPARAM(i), LPARAM(&item as *const _ as isize));
    }
    let item = LVITEMW { stateMask: LVIS_STATEIMAGEMASK, state: LIST_VIEW_ITEM_STATE_FLAGS((if checked { 2 } else { 1 }) << 12), ..Default::default() };
    SendMessageW(list, LVM_SETITEMSTATE, WPARAM(i), LPARAM(&item as *const _ as isize));
}

/// New album…: a name; one there already (in any case) is ticked instead.
unsafe fn new_album(hwnd: HWND) {
    let Some(name) = ask_text(hwnd, "New album", "The new album's name:", "") else { return };
    let name = name.trim().to_string();
    if name.is_empty() {
        return;
    }
    let Some((list, names)) = PICKER.with(|p| p.borrow().clone()) else { return };
    let i = match names.iter().position(|(n, _)| n.eq_ignore_ascii_case(&name)) {
        Some(i) => {
            let item = LVITEMW { stateMask: LVIS_STATEIMAGEMASK, state: LIST_VIEW_ITEM_STATE_FLAGS(2 << 12), ..Default::default() };
            SendMessageW(list, LVM_SETITEMSTATE, WPARAM(i), LPARAM(&item as *const _ as isize));
            i
        }
        None => {
            add_row(list, names.len(), &name, "new", true);
            PICKER.with(|p| {
                if let Some((_, n)) = p.borrow_mut().as_mut() {
                    n.push((name.clone(), false));
                }
            });
            names.len()
        }
    };
    SendMessageW(list, LVM_ENSUREVISIBLE, WPARAM(i), LPARAM(0));
    let _ = SetFocus(list);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Dialog;
    let finish = |ok: bool| {
        if !data.is_null() {
            (*data).done = true;
            (*data).ok = ok;
        }
    };
    match msg {
        WM_CTLCOLORSTATIC => {
            let dc = HDC(wparam.0 as _);
            SetBkColor(dc, COLORREF(GetSysColor(COLOR_WINDOW)));
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        WM_COMMAND => {
            match (wparam.0 & 0xffff) as i32 {
                id if id == IDOK.0 => finish(true),
                id if id == IDCANCEL.0 => finish(false),
                ID_NEW => new_album(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            finish(false);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
