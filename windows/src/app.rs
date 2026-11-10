//! The window: the libraries on the left (each shown or not), the bars and
//! the picture area on the right, the loader thread that reads the
//! libraries, and the watcher thread that wakes it the moment one changes.
//! The UI thread only draws: everything that can block runs on a thread
//! and posts a message back.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use libretifs::AttrValue;
use retifs_photos_core::{albums_of, Album, Library, Photo, ALBUM};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::{SS_CENTERIMAGE, SS_ENDELLIPSIS, SS_NOPREFIX};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL, VK_F5};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::canvas::{self, Key};

const CLASS: PCWSTR = w!("retifs-photos");
pub(crate) const WM_LOADED: u32 = WM_APP + 1;
pub(crate) const WM_THUMBS: u32 = WM_APP + 2;
pub(crate) const WM_IMAGE: u32 = WM_APP + 3;
pub(crate) const WM_NOTE: u32 = WM_APP + 4;
/// The bars again (what they show changed).
pub(crate) const WM_RELAYOUT: u32 = WM_APP + 5;

const ID_TABS: i32 = 100;
const ID_LIBS: i32 = 101;
const ID_ANNOUNCES: i32 = 102;
pub(crate) const ID_BACK: i32 = 110;
pub(crate) const ID_PREV: i32 = 111;
pub(crate) const ID_NEXT: i32 = 112;
pub(crate) const ID_FAV: i32 = 113;
pub(crate) const ID_ADD: i32 = 114;
pub(crate) const ID_CLEAR: i32 = 115;
pub(crate) const ID_REMOVE: i32 = 116;
pub(crate) const ID_LIBRARY: i32 = 117;
pub(crate) const ID_RENAME: i32 = 118;
pub(crate) const ID_COVER: i32 = 119;
pub(crate) const ID_OPEN: i32 = 120;
pub(crate) const ID_SELECT_ALL: i32 = 121;
const ID_SHOW_PHOTOS: i32 = 122;
const ID_SHOW_ALBUMS: i32 = 123;
/// The library menu's items: a library each, from here.
const ID_TO_LIBRARY: i32 = 1000;

/// A library this device has.
#[derive(Clone)]
pub struct Lib {
    pub name: String,
    pub writable: bool,
}

#[derive(Clone, PartialEq)]
pub enum View {
    Photos,
    Albums,
    Album(String),
}

/// Everything the UI thread knows. Borrowed only for short spells, never
/// across a call that dispatches messages (a menu, a dialog, moving a child).
pub struct State {
    pub libraries: Vec<Lib>,
    /// Libraries not shown (remembered between runs).
    pub hidden: BTreeSet<String>,
    /// Every photo in every library, newest first.
    pub all: Vec<Photo>,
    /// Where each photo is in `all`.
    pub index: HashMap<Key, usize>,
    /// Libraries read so far (counts for the others aren't known yet).
    pub read: HashSet<String>,
    pub error: Option<String>,
    /// "Reading libraries…" while the first read goes on.
    pub reading: Option<String>,
    pub view: View,
    /// The photos the grid shows now: the timeline, or an album's.
    pub list: Vec<Photo>,
    pub list_index: HashMap<Key, usize>,
    /// The albums in the libraries shown.
    pub albums: Vec<Album>,
    /// Where the timeline and the albums were scrolled to, for going back.
    pub scrolls: HashMap<&'static str, f32>,
    pub canvas: canvas::Canvas,
}

/// The window's controls (copied out: no borrow needed to reach them).
#[derive(Clone, Copy)]
struct Controls {
    hwnd: HWND,
    announces: HWND,
    libs_label: HWND,
    libs: HWND,
    status: HWND,
    tabs: HWND,
    back: HWND,
    prev: HWND,
    next: HWND,
    title: HWND,
    fav: HWND,
    add: HWND,
    remove: HWND,
    clear: HWND,
    library: HWND,
    rename: HWND,
    canvas: HWND,
    font: HFONT,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static CONTROLS: Cell<Option<Controls>> = const { Cell::new(None) };
    /// Filling the library list: its change notes are ours, not the user's.
    static FILLING: Cell<bool> = const { Cell::new(false) };
}

/// The main window, for worker threads to post to.
static MAIN: AtomicIsize = AtomicIsize::new(0);
static NOTE: Mutex<String> = Mutex::new(String::new());
/// What the loader read last, for the UI thread to take.
static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);
/// Wakes the loader to read everything again (after a change made here).
static POKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// Each library's change count as the loader read it, for the watcher.
static READ_COUNTS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Runs `f` on the state, unless it's borrowed already (a message sent
/// to us while we were busy): then nothing, and the caller tries later.
pub fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| {
        let mut b = s.try_borrow_mut().ok()?;
        b.as_mut().map(f)
    })
}

fn controls() -> Option<Controls> {
    CONTROLS.with(|c| c.get())
}

/// Tests and screenshots (`RETIFS_PHOTOS_OFFSCREEN`): nothing shows on the
/// screen or takes the focus from what the user is doing.
pub(crate) fn offscreen() -> bool {
    std::env::var_os("RETIFS_PHOTOS_OFFSCREEN").is_some()
}

pub(crate) fn wide(s: &str) -> HSTRING {
    HSTRING::from(s)
}

pub(crate) fn post(msg: u32) {
    let h = MAIN.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h as _), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// Says something under the libraries (from any thread).
pub(crate) fn note(text: impl Into<String>) {
    *NOTE.lock().unwrap() = text.into();
    post(WM_NOTE);
}

/// Reads everything again soon.
pub(crate) fn poke() {
    let (m, c) = &POKE;
    *m.lock().unwrap() = true;
    c.notify_all();
}

/// Runs `work` on a thread with its own connection; its error (if any)
/// goes to the note line, and the libraries are read again after.
fn spawn(what: &'static str, work: impl FnOnce(&Library) -> Result<(), String> + Send + 'static) {
    note(format!("{what}…"));
    std::thread::spawn(move || {
        let lib = Library::new();
        match work(&lib) {
            Ok(()) => note(""),
            Err(e) => note(format!("{what}: {e}")),
        }
        poke();
    });
}

/// Where a windowed program writes what went wrong (it has no console).
fn log_line(text: &str) {
    use std::io::Write;
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("retifs").join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("retifs-photos.log")) {
        let _ = writeln!(f, "{text}");
    }
}

/// The libraries not shown, one a line (in the user's roaming settings).
fn settings_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("retifs-photos").join("hidden-libraries.txt")
}

fn load_hidden() -> BTreeSet<String> {
    std::fs::read_to_string(settings_path()).map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default()
}

fn save_hidden(hidden: &BTreeSet<String>) {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, hidden.iter().map(|h| format!("{h}\n")).collect::<String>());
}

pub fn run() {
    std::panic::set_hook(Box::new(|info| log_line(&format!("panic: {info}"))));
    unsafe {
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES | ICC_STANDARD_CLASSES });
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as _),
            hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&class);
        canvas::register(&class);
        crate::dialogs::register(&class);
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, w!("Photos"), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, CW_USEDEFAULT, CW_USEDEFAULT, 1000, 700, None, None, instance, None) else {
            return;
        };
        MAIN.store(hwnd.0 as isize, Ordering::SeqCst);
        // Its size for the screen it's on (created at 96 dpi's numbers).
        let _ = SetWindowPos(hwnd, None, 0, 0, scale(hwnd, 1240), scale(hwnd, 820), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        build(hwnd);
        if offscreen() {
            // Tests and screenshots: drawn off screen, never activated
            // (PrintWindow still sees it).
            let _ = SetWindowPos(hwnd, None, -4000, 100, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        }
        crate::images::start();
        std::thread::spawn(loader);
        std::thread::spawn(watcher);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_KEYDOWN && shortcut(msg.wParam.0 as u16) {
                continue;
            }
            // Tab between the library list and the pictures.
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Keys for the whole window: Ctrl+1 the timeline, Ctrl+2 the albums, F5 read again.
unsafe fn shortcut(vk: u16) -> bool {
    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
    match vk {
        0x31 if ctrl => command(ID_SHOW_PHOTOS),
        0x32 if ctrl => command(ID_SHOW_ALBUMS),
        v if v == VK_F5.0 => {
            note("Reading the libraries again…");
            poke();
        }
        _ => return false,
    }
    true
}

pub(crate) unsafe fn scale(hwnd: HWND, v: i32) -> i32 {
    v * GetDpiForWindow(hwnd).max(96) as i32 / 96
}

pub(crate) unsafe fn child(parent: HWND, class: PCWSTR, text: &str, style: WINDOW_STYLE, id: i32, font: HFONT) -> HWND {
    let h = CreateWindowExW(WINDOW_EX_STYLE(0), class, &wide(text), WS_CHILD | style, 0, 0, 10, 10, parent, HMENU(id as _), GetModuleHandleW(None).unwrap_or_default(), None).unwrap_or_default();
    SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    h
}

/// The system's message font, at the window's DPI.
pub(crate) unsafe fn message_font(hwnd: HWND) -> HFONT {
    let mut m = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    let _ = SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, m.cbSize, Some(&mut m as *mut _ as *mut _), 0, GetDpiForWindow(hwnd).max(96));
    CreateFontIndirectW(&m.lfMessageFont)
}

unsafe fn build(hwnd: HWND) {
    let font = message_font(hwnd);
    let button = |text: &str, id: i32| child(hwnd, WC_BUTTONW, text, WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32), id, font);
    // The libraries: Announces above them (the house layout), then a
    // checkbox each.
    let announces = child(hwnd, WC_BUTTONW, "Announces", WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32), ID_ANNOUNCES, font);
    let libs_label = child(hwnd, WC_STATICW, "Libraries", WS_VISIBLE, 0, font);
    let libs = child(hwnd, WC_LISTVIEWW, "", WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE((LVS_REPORT | LVS_NOCOLUMNHEADER | LVS_SINGLESEL | LVS_SHOWSELALWAYS) as u32), ID_LIBS, font);
    SendMessageW(libs, LVM_SETEXTENDEDLISTVIEWSTYLE, WPARAM(0), LPARAM((LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP) as isize));
    for (i, fmt) in [LVCFMT_LEFT, LVCFMT_RIGHT].iter().enumerate() {
        let col = LVCOLUMNW { mask: LVCF_WIDTH | LVCF_FMT, fmt: *fmt, cx: 100, ..Default::default() };
        SendMessageW(libs, LVM_INSERTCOLUMNW, WPARAM(i), LPARAM(&col as *const _ as isize));
    }
    let status = child(hwnd, WC_STATICW, "Asking retifs…", WS_VISIBLE | WINDOW_STYLE(SS_NOPREFIX.0), 0, font);

    let tabs = child(hwnd, WC_TABCONTROLW, "", WS_VISIBLE | WS_CLIPSIBLINGS | WS_TABSTOP, ID_TABS, font);
    for (i, t) in ["Photos", "Albums"].iter().enumerate() {
        let text = wide(t);
        let item = TCITEMW { mask: TCIF_TEXT, pszText: PWSTR(text.as_ptr() as *mut u16), ..Default::default() };
        SendMessageW(tabs, TCM_INSERTITEMW, WPARAM(i), LPARAM(&item as *const _ as isize));
    }
    let back = button("‹ Albums", ID_BACK);
    let prev = button("◀", ID_PREV);
    let next = button("▶", ID_NEXT);
    let title = child(hwnd, WC_STATICW, "", WS_VISIBLE | WINDOW_STYLE(SS_NOPREFIX.0 | SS_ENDELLIPSIS.0 | SS_CENTERIMAGE.0), 0, font);
    // A push-like checkbox: pressed while what's selected is a favourite.
    let fav = child(hwnd, WC_BUTTONW, "★ Favourite", WS_TABSTOP | WINDOW_STYLE((BS_CHECKBOX | BS_PUSHLIKE) as u32), ID_FAV, font);
    let add = button("+ Add to albums…", ID_ADD);
    let remove = button("Remove from album", ID_REMOVE);
    let clear = button("Clear selection", ID_CLEAR);
    let library = button("Library…", ID_LIBRARY);
    let rename = button("Rename…", ID_RENAME);
    let canvas = canvas::create(hwnd);

    CONTROLS.with(|c| c.set(Some(Controls { hwnd, announces, libs_label, libs, status, tabs, back, prev, next, title, fav, add, remove, clear, library, rename, canvas, font })));
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            libraries: Vec::new(),
            hidden: load_hidden(),
            all: Vec::new(),
            index: HashMap::new(),
            read: HashSet::new(),
            error: None,
            reading: Some("Asking retifs…".into()),
            view: View::Photos,
            list: Vec::new(),
            list_index: HashMap::new(),
            albums: Vec::new(),
            scrolls: HashMap::new(),
            canvas: canvas::Canvas::new(canvas, GetDpiForWindow(canvas).max(96) as f32),
        })
    });
    layout(hwnd);
    let _ = SetFocus(canvas);
}

/// The sidebar's width.
unsafe fn side_width(hwnd: HWND) -> i32 {
    scale(hwnd, 230)
}

/// Places the controls, and shows the ones the view has now: the tabs and
/// a bar with what can be done to the selection, the album or the photo
/// open in the viewer.
unsafe fn layout(hwnd: HWND) {
    let Some(c) = controls() else { return };
    // What the bar shows, read while nothing else is going on.
    let Some(bar) = with(bar_state) else {
        post(WM_RELAYOUT);
        return;
    };
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let s = |v| scale(hwnd, v);
    let (w, h) = (rc.right, rc.bottom);
    let m = s(10);
    let side = side_width(hwnd);
    let place = |h: HWND, x: i32, y: i32, w: i32, hh: i32| {
        let _ = MoveWindow(h, x, y, w.max(0), hh.max(0), true);
    };
    let show = |h: HWND, on: bool| {
        let _ = ShowWindow(h, if on { SW_SHOWNA } else { SW_HIDE });
    };

    // The sidebar: Announces, the libraries, the status at the bottom.
    let bh = s(30);
    place(c.announces, m, m, side - 2 * m, bh);
    place(c.libs_label, m + s(2), m + bh + s(14), side - 2 * m, s(20));
    let status_h = s(64);
    let list_top = m + bh + s(38);
    place(c.libs, m, list_top, side - 2 * m, h - list_top - status_h - m);
    place(c.status, m + s(2), h - status_h - s(4), side - 2 * m, status_h);
    let lw = side - 2 * m - GetSystemMetricsForDpi(SM_CXVSCROLL, GetDpiForWindow(hwnd)) - s(4);
    SendMessageW(c.libs, LVM_SETCOLUMNWIDTH, WPARAM(0), LPARAM((lw - s(56)) as isize));
    SendMessageW(c.libs, LVM_SETCOLUMNWIDTH, WPARAM(1), LPARAM(s(56) as isize));

    // The right side: tabs, the bar, the pictures.
    let x0 = side + s(1);
    let rw = w - x0;
    let mut y = m;
    show(c.tabs, !bar.viewer);
    if !bar.viewer {
        place(c.tabs, x0 + m, y, rw - 2 * m, s(28));
        y += s(28) + s(6);
    }
    let row = s(30);
    let mut left = x0 + m;
    let mut right = x0 + rw - m;
    let mut at_left = |h: HWND, width: i32, on: bool| {
        show(h, on);
        if on {
            place(h, left, y, width, row);
            left += width + s(6);
        }
    };
    at_left(c.back, s(96), bar.back.is_some());
    at_left(c.prev, s(36), bar.viewer);
    at_left(c.next, s(36), bar.viewer);
    let mut at_right = |h: HWND, width: i32, on: bool| {
        show(h, on);
        if on {
            right -= width;
            place(h, right, y, width, row);
            right -= s(6);
        }
    };
    at_right(c.clear, s(118), bar.selection && !bar.viewer);
    at_right(c.remove, s(140), bar.selection && bar.album && !bar.viewer);
    at_right(c.rename, s(90), bar.album && !bar.selection && !bar.viewer);
    at_right(c.library, s(170), bar.album && !bar.selection && !bar.viewer);
    at_right(c.add, s(140), bar.selection || bar.viewer);
    at_right(c.fav, s(110), bar.selection || bar.viewer);
    place(c.title, left + s(4), y, right - left - s(4), row);
    y += row + s(8);
    place(c.canvas, x0, y, rw, h - y);

    if let Some(b) = &bar.back {
        set_text(c.back, b);
    }
    set_text(c.title, &bar.title);
    set_text(c.library, &bar.library);
    SendMessageW(c.fav, BM_SETCHECK, WPARAM(if bar.favourite { 1 } else { 0 }), LPARAM(0));
    SendMessageW(c.tabs, TCM_SETCURSEL, WPARAM(if bar.albums_tab { 1 } else { 0 }), LPARAM(0));
    let _ = InvalidateRect(hwnd, None, true);
}

/// What the bar above the pictures says and offers now.
struct Bar {
    viewer: bool,
    album: bool,
    albums_tab: bool,
    selection: bool,
    back: Option<String>,
    title: String,
    library: String,
    favourite: bool,
}

fn bar_state(s: &mut State) -> Bar {
    let selected = canvas::selected_photos(s);
    let viewer = s.canvas.viewer.is_some();
    let mut bar = Bar {
        viewer,
        album: matches!(s.view, View::Album(_)),
        albums_tab: !matches!(s.view, View::Photos),
        selection: !selected.is_empty() && !viewer,
        back: None,
        title: String::new(),
        library: String::new(),
        favourite: !selected.is_empty() && selected.iter().all(|p| p.favorite()),
    };
    if let Some(v) = &s.canvas.viewer {
        bar.back = Some("‹ Back".into());
        if let Some(p) = selected.first() {
            bar.title = format!("{}   ·   {} of {}", p.title(), v.index + 1, v.keys.len());
        }
        return bar;
    }
    if let View::Album(name) = &s.view {
        bar.back = Some("‹ Albums".into());
        bar.library = match album_library(s, name) {
            Some(l) => format!("Library: {l} ▾"),
            None => "Libraries: several ▾".into(),
        };
    }
    bar.title = if bar.selection {
        format!("{} selected", selected.len())
    } else {
        match &s.view {
            View::Photos => count(s.list.len(), "photo", "photos"),
            View::Albums => count(s.albums.len(), "album", "albums"),
            View::Album(name) => format!("{name}   ·   {}", count(s.list.len(), "photo", "photos")),
        }
    };
    bar
}

pub(crate) fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The one library an album's photos are all in, if they are (in any library).
fn album_library(s: &State, name: &str) -> Option<String> {
    let libs: BTreeSet<&str> = s.all.iter().filter(|p| in_album(p, name)).map(|p| p.ns.as_str()).collect();
    (libs.len() == 1).then(|| libs.into_iter().next().unwrap().to_string())
}

pub(crate) fn in_album(p: &Photo, name: &str) -> bool {
    p.albums().iter().any(|a| a.eq_ignore_ascii_case(name))
}

pub(crate) unsafe fn set_text(h: HWND, text: &str) {
    let _ = SetWindowTextW(h, &wide(text));
}

/// What's shown again from what's known: the libraries shown, their
/// albums, the photos of the view; the grid keeps its place.
pub(crate) fn refilter(s: &mut State) {
    let anchor = canvas::anchor(s);
    s.index = s.all.iter().enumerate().map(|(i, p)| (canvas::key(p), i)).collect();
    let shown: Vec<Photo> = s.all.iter().filter(|p| !s.hidden.contains(&p.ns)).cloned().collect();
    s.albums = albums_of(&shown);
    if let View::Album(name) = &s.view {
        if !s.albums.iter().any(|a| a.name.eq_ignore_ascii_case(name)) && s.reading.is_none() {
            // The album is gone (its last photo left it, or its library is hidden).
            s.view = View::Albums;
        }
    }
    s.list = match &s.view {
        View::Photos => shown,
        View::Albums => Vec::new(),
        View::Album(name) => shown.into_iter().filter(|p| in_album(p, name)).collect(),
    };
    s.list_index = s.list.iter().enumerate().map(|(i, p)| (canvas::key(p), i)).collect();
    canvas::relayout(s, anchor);
    post(WM_RELAYOUT);
}

/// Shows `view`, each kept where it was scrolled to.
pub(crate) fn set_view(s: &mut State, view: View) {
    let slot = |v: &View| match v {
        View::Photos => "photos",
        View::Albums => "albums",
        View::Album(_) => "album",
    };
    s.scrolls.insert(slot(&s.view), s.canvas.scroll);
    let back_to_albums = matches!(s.view, View::Album(_)) && view == View::Albums;
    s.view = view;
    s.canvas.clear_selection();
    s.canvas.focus = None;
    s.canvas.layout = None;
    refilter(s);
    let at = if matches!(s.view, View::Album(_)) && !back_to_albums { 0.0 } else { s.scrolls.get(slot(&s.view)).copied().unwrap_or(0.0) };
    s.canvas.scroll_to(at);
}

/// What the loader read.
struct Loaded {
    libraries: Vec<Lib>,
    photos: Vec<Photo>,
    read: HashSet<String>,
    error: Option<String>,
    reading: Option<String>,
    /// The whole read is done (not a library of it).
    done: bool,
}

fn publish(l: Loaded) {
    *LOADED.lock().unwrap() = Some(l);
    post(WM_LOADED);
}

fn sort(photos: &mut Vec<Photo>) {
    photos.sort_by(|a, b| b.when().cmp(&a.when()).then(a.id.cmp(&b.id)));
    photos.dedup_by(|a, b| a.id == b.id && a.ns == b.ns);
}

/// Reads every library (all of them, so showing one again is at once),
/// then again when poked: by the watcher when one changed (here or on
/// another device), or after a change made here. Without retifsd it asks
/// again every 15 s; otherwise every 5 min, in case a change went unsaid.
fn loader() {
    let lib = Library::new();
    let mut counts: HashMap<String, u64> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    // Read once without trouble: later reads replace it whole, not library by library.
    let mut ok = false;
    let mut force = true;
    loop {
        if !force {
            let now = lib.change_counts(&names);
            force = !ok || now.is_empty() || now != counts;
        }
        if force {
            match lib.libraries() {
                Err(e) => {
                    ok = false;
                    publish(Loaded { libraries: Vec::new(), photos: Vec::new(), read: HashSet::new(), error: Some(e), reading: None, done: true });
                }
                Ok(n) => {
                    names = n;
                    let libraries: Vec<Lib> = names.iter().map(|n| Lib { name: n.clone(), writable: lib.writable(n) }).collect();
                    let gradual = !ok;
                    let mut all = Vec::new();
                    let mut read = HashSet::new();
                    let mut error = None;
                    for (i, ns) in names.iter().enumerate() {
                        if gradual {
                            publish(Loaded { libraries: libraries.clone(), photos: all.clone(), read: read.clone(), error: None, reading: Some(format!("Reading {ns} ({} of {})…", i + 1, names.len())), done: false });
                        }
                        match lib.photos(std::slice::from_ref(ns)) {
                            Ok(p) => all.extend(p),
                            Err(e) => error = Some(format!("{ns}: {e}")),
                        }
                        read.insert(ns.clone());
                        sort(&mut all);
                    }
                    counts = lib.change_counts(&names);
                    *READ_COUNTS.lock().unwrap() = Some(counts.clone());
                    ok = error.is_none();
                    publish(Loaded { libraries, photos: all, read, error, reading: None, done: true });
                }
            }
        }
        let (m, c) = &POKE;
        let mut poked = m.lock().unwrap();
        if !*poked {
            poked = c.wait_timeout(poked, Duration::from_secs(if ok { 300 } else { 15 })).unwrap().0;
        }
        force = *poked;
        *poked = false;
    }
}

/// Pokes the loader the moment a library changes: retifsd says so
/// (`Library::wait_change`), waited for on a `Library` of its own so the
/// loader isn't held up meanwhile.
fn watcher() {
    let lib = Library::new();
    let mut known: HashMap<String, u64> = HashMap::new();
    loop {
        // The libraries read last, a newly read one from the count of its read.
        let read = READ_COUNTS.lock().unwrap().clone().unwrap_or_default();
        known.retain(|ns, _| read.contains_key(ns));
        for (ns, n) in read {
            known.entry(ns).or_insert(n);
        }
        if lib.wait_change(&mut known, Duration::from_secs(60)) {
            poke();
        }
    }
}

/// Shows what the loader read.
unsafe fn loaded() {
    let Some(l) = LOADED.lock().unwrap().take() else { return };
    let done = l.done;
    let applied = with(|s| {
        if l.error.is_some() && l.libraries.is_empty() {
            // No daemon: said, and what was shown stays until it's back.
            s.error = l.error;
            s.reading = None;
        } else {
            s.libraries = l.libraries;
            s.all = l.photos;
            s.read = l.read;
            s.error = l.error;
            s.reading = l.reading;
            if done {
                // Thumbnails that couldn't be read are tried again.
                s.canvas.failed.clear();
            }
        }
        refilter(s);
    });
    if applied.is_none() {
        // Busy: the next round shows it.
        return;
    }
    fill_libraries();
    show_status();
    if let Some(c) = controls() {
        let _ = InvalidateRect(c.canvas, None, false);
    }
}

/// The library list: a checkbox each (shown or not) and its photo count;
/// a library read and holding no photos isn't listed (an app lists the
/// libraries it has something to show from).
unsafe fn fill_libraries() {
    let Some(c) = controls() else { return };
    let Some(rows) = with(|s| {
        s.libraries
            .iter()
            .filter_map(|l| {
                let n = s.all.iter().filter(|p| p.ns == l.name).count();
                let read = s.read.contains(&l.name);
                if read && n == 0 {
                    return None;
                }
                let count = if read { n.to_string() } else { "…".into() };
                Some((l.name.clone(), count, !s.hidden.contains(&l.name)))
            })
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    FILLING.with(|f| f.set(true));
    let have = SendMessageW(c.libs, LVM_GETITEMCOUNT, WPARAM(0), LPARAM(0)).0 as usize;
    let same = have == rows.len() && rows.iter().enumerate().all(|(i, (name, _, _))| item_text(c.libs, i, 0) == *name);
    if !same {
        SendMessageW(c.libs, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    }
    for (i, (name, count, on)) in rows.iter().enumerate() {
        for (col, text) in [name, count].iter().enumerate() {
            let t = wide(text);
            let item = LVITEMW { mask: LVIF_TEXT, iItem: i as i32, iSubItem: col as i32, pszText: PWSTR(t.as_ptr() as *mut u16), ..Default::default() };
            let msg = if col == 0 && !same { LVM_INSERTITEMW } else { LVM_SETITEMTEXTW };
            SendMessageW(c.libs, msg, WPARAM(i), LPARAM(&item as *const _ as isize));
        }
        // The state image: 1 unchecked, 2 checked.
        let item = LVITEMW { stateMask: LVIS_STATEIMAGEMASK, state: LIST_VIEW_ITEM_STATE_FLAGS((if *on { 2 } else { 1 }) << 12), ..Default::default() };
        SendMessageW(c.libs, LVM_SETITEMSTATE, WPARAM(i), LPARAM(&item as *const _ as isize));
    }
    FILLING.with(|f| f.set(false));
}

unsafe fn item_text(list: HWND, i: usize, col: i32) -> String {
    let mut buf = [0u16; 260];
    let item = LVITEMW { iSubItem: col, pszText: PWSTR(buf.as_mut_ptr()), cchTextMax: buf.len() as i32, ..Default::default() };
    let n = SendMessageW(list, LVM_GETITEMTEXTW, WPARAM(i), LPARAM(&item as *const _ as isize)).0 as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

/// The line under the libraries: reading, a note, an error, or the totals.
unsafe fn show_status() {
    let Some(c) = controls() else { return };
    let note = NOTE.lock().unwrap().clone();
    let Some(text) = with(|s| {
        if let Some(e) = &s.error {
            format!("retifsd isn't answering: {e}")
        } else if let Some(r) = &s.reading {
            r.clone()
        } else if !note.is_empty() {
            note.clone()
        } else {
            let shown = s.all.iter().filter(|p| !s.hidden.contains(&p.ns)).count();
            // The libraries shown that hold photos (the list has only those).
            let with_photos = s.libraries.iter().filter(|l| !s.hidden.contains(&l.name) && s.all.iter().any(|p| p.ns == l.name)).count();
            format!("{} in {}\r\n{}", count(shown, "photo", "photos"), count(with_photos, "library", "libraries"), count(s.albums.len(), "album", "albums"))
        }
    }) else {
        return;
    };
    set_text(c.status, &text);
}

/// A library's checkbox changed: shown or hidden from now on.
unsafe fn library_toggled(i: usize, on: bool) {
    // By its name: the list leaves out libraries with no photos.
    let Some(c) = controls() else { return };
    let name = item_text(c.libs, i, 0);
    with(|s| {
        let Some(l) = s.libraries.iter().find(|l| l.name == name) else { return };
        let changed = if on { s.hidden.remove(&l.name) } else { s.hidden.insert(l.name.clone()) };
        if changed {
            save_hidden(&s.hidden);
            refilter(s);
        }
    });
    show_status();
    if let Some(c) = controls() {
        let _ = InvalidateRect(c.canvas, None, false);
    }
}

/// Does what a button, a menu item or a key asks.
pub(crate) unsafe fn command(id: i32) {
    let Some(c) = controls() else { return };
    let hwnd = c.hwnd;
    match id {
        ID_SHOW_PHOTOS => {
            with(|s| {
                s.canvas.viewer = None;
                set_view(s, View::Photos)
            });
        }
        ID_SHOW_ALBUMS => {
            with(|s| {
                s.canvas.viewer = None;
                set_view(s, View::Albums)
            });
        }
        ID_BACK => {
            with(|s| {
                if s.canvas.viewer.is_some() {
                    canvas::close_viewer(s);
                } else {
                    set_view(s, View::Albums);
                }
            });
        }
        ID_PREV | ID_NEXT => {
            with(|s| canvas::step(s, if id == ID_PREV { -1 } else { 1 }));
        }
        ID_OPEN => {
            with(|s| canvas::open_focused(s));
        }
        ID_CLEAR => {
            with(|s| s.canvas.clear_selection());
            post(WM_RELAYOUT);
        }
        ID_SELECT_ALL => {
            with(|s| {
                let keys: Vec<Key> = s.list.iter().map(canvas::key).collect();
                s.canvas.selected = keys.into_iter().collect();
            });
            post(WM_RELAYOUT);
        }
        ID_FAV => favourite(),
        ID_ADD => add_to_albums(hwnd),
        ID_REMOVE => remove_from_album(),
        ID_COVER => make_cover(),
        ID_LIBRARY => album_library_menu(hwnd),
        ID_RENAME => rename_album(hwnd),
        ID_ANNOUNCES => announces(hwnd),
        _ => return,
    }
    let _ = InvalidateRect(c.canvas, None, false);
    // Keys go on to the pictures, not to the button pressed.
    let _ = SetFocus(c.canvas);
}

/// The photos an action is for: the one in the viewer, else those selected.
fn chosen() -> Vec<Photo> {
    with(|s| canvas::selected_photos(s)).unwrap_or_default()
}

/// Those of the chosen photos this device may change (in a library it
/// may write to); the others are said, and left as they are.
unsafe fn changeable() -> Vec<Photo> {
    let photos = chosen();
    let (ok, not): (Vec<Photo>, Vec<Photo>) = with(|s| {
        let writable: HashSet<&str> = s.libraries.iter().filter(|l| l.writable).map(|l| l.name.as_str()).collect();
        photos.into_iter().partition(|p| writable.contains(p.ns.as_str()))
    })
    .unwrap_or_default();
    if !not.is_empty() {
        let libs: BTreeSet<&str> = not.iter().map(|p| p.ns.as_str()).collect();
        let libs = libs.into_iter().collect::<Vec<_>>().join(", ");
        note(if ok.is_empty() { format!("This device may not change {libs} (read only here).") } else { format!("{} in {libs} left as they are: read only here.", count(not.len(), "photo", "photos")) });
        show_status();
    }
    ok
}

/// Changes photos here at once (as retifsd is asked to), until read again.
fn update_photos(keys: &[Key], change: impl Fn(&mut Photo)) {
    with(|s| {
        for k in keys {
            if let Some(&i) = s.index.get(k) {
                change(&mut s.all[i]);
            }
        }
        refilter(s);
    });
}

fn set_names(p: &mut Photo, key: &str, names: Vec<String>) {
    match names.len() {
        0 => {
            p.attrs.remove(key);
        }
        1 => {
            p.attrs.insert(key.into(), AttrValue::Str(names[0].clone()));
        }
        _ => {
            p.attrs.insert(key.into(), AttrValue::List(names.into_iter().map(AttrValue::Str).collect()));
        }
    }
}

/// A star for each chosen photo, or none if they all had one.
unsafe fn favourite() {
    let photos = changeable();
    if photos.is_empty() {
        return;
    }
    let on = !photos.iter().all(|p| p.favorite());
    let keys: Vec<Key> = photos.iter().map(canvas::key).collect();
    update_photos(&keys, |p| {
        if on {
            p.attrs.insert("favorite".into(), AttrValue::Bool(true));
        } else {
            p.attrs.remove("favorite");
        }
    });
    spawn(if on { "Adding to favourites" } else { "Removing from favourites" }, move |lib| {
        for p in &photos {
            lib.set_favorite(p, on)?;
        }
        Ok(())
    });
}

/// Puts the chosen photos into albums, or takes them out: the albums
/// there are (checked if all of them are in it already), and new ones.
unsafe fn add_to_albums(owner: HWND) {
    let photos = changeable();
    if photos.is_empty() {
        return;
    }
    let albums: Vec<(String, usize, usize)> = with(|s| {
        let all_albums = albums_of(&s.all.iter().filter(|p| !s.hidden.contains(&p.ns)).cloned().collect::<Vec<_>>());
        all_albums.iter().map(|a| (a.name.clone(), a.count, photos.iter().filter(|p| in_album(p, &a.name)).count())).collect()
    })
    .unwrap_or_default();
    let Some((add, remove)) = crate::dialogs::pick_albums(owner, &albums, photos.len()) else { return };
    if add.is_empty() && remove.is_empty() {
        return;
    }
    let keys: Vec<Key> = photos.iter().map(canvas::key).collect();
    let (a2, r2) = (add.clone(), remove.clone());
    update_photos(&keys, move |p| {
        let mut names: Vec<String> = p.albums().into_iter().filter(|n| !r2.iter().chain(&a2).any(|x| x.eq_ignore_ascii_case(n))).collect();
        names.extend(a2.iter().cloned());
        set_names(p, ALBUM, names);
    });
    spawn("Filing in albums", move |lib| {
        for p in &photos {
            for name in &add {
                lib.file_in_album(p, name, true)?;
            }
            for name in &remove {
                lib.file_in_album(p, name, false)?;
            }
        }
        Ok(())
    });
}

/// Takes the selected photos out of the album shown.
unsafe fn remove_from_album() {
    let Some(View::Album(name)) = with(|s| s.view.clone()) else { return };
    let photos = changeable();
    if photos.is_empty() {
        return;
    }
    let keys: Vec<Key> = photos.iter().map(canvas::key).collect();
    let n2 = name.clone();
    update_photos(&keys, move |p| {
        let names = p.albums().into_iter().filter(|a| !a.eq_ignore_ascii_case(&n2)).collect();
        set_names(p, ALBUM, names);
    });
    with(|s| s.canvas.clear_selection());
    spawn("Taking photos out of the album", move |lib| {
        for p in &photos {
            lib.file_in_album(p, &name, false)?;
        }
        Ok(())
    });
}

/// The selected photo is the album's cover from now on.
unsafe fn make_cover() {
    let Some((View::Album(name), libs)) = with(|s| (s.view.clone(), s.libraries.iter().map(|l| l.name.clone()).collect::<Vec<_>>())) else { return };
    let photos = chosen();
    let [p] = photos.as_slice() else { return };
    let p = p.clone();
    spawn("Making the album's cover", move |lib| lib.set_cover(&libs, &name, &p));
}

/// Which library the album shown is in: choosing another moves all its
/// photos there (their content by reference), after asking.
unsafe fn album_library_menu(owner: HWND) {
    let Some(Some((name, now, libs, n))) = with(|s| {
        let View::Album(name) = s.view.clone() else { return None };
        let n = s.all.iter().filter(|p| in_album(p, &name)).count();
        Some((name.clone(), album_library(s, &name), s.libraries.clone(), n))
    }) else {
        return;
    };
    let Ok(menu) = CreatePopupMenu() else { return };
    let writable: Vec<&Lib> = libs.iter().filter(|l| l.writable).collect();
    if writable.is_empty() {
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, w!("Photos may add to none of the libraries"));
    }
    for (i, l) in writable.iter().enumerate() {
        let flags = if now.as_deref() == Some(l.name.as_str()) { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, flags, (ID_TO_LIBRARY + i as i32) as usize, &wide(&l.name));
    }
    let Some(c) = controls() else { return };
    let mut rc = RECT::default();
    let _ = GetWindowRect(c.library, &mut rc);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTBUTTON, rc.left, rc.bottom, 0, owner, None);
    let _ = DestroyMenu(menu);
    let Some(to) = (cmd.0 as i32).checked_sub(ID_TO_LIBRARY).and_then(|i| writable.get(i as usize)).map(|l| l.name.clone()) else { return };
    if now.as_deref() == Some(to.as_str()) {
        return;
    }
    let from = match &now {
        Some(l) => format!("out of {l}"),
        None => "out of the libraries they're in now".into(),
    };
    let q = wide(&format!("Move the album “{name}” into {to}?\n\nIts {} move there, {from}. Their content isn't copied: the new records refer to it.", count(n, "photo", "photos")));
    if MessageBoxW(owner, &q, w!("Photos"), MB_YESNO | MB_ICONQUESTION) != IDYES {
        return;
    }
    // Moved into a library not shown: it's shown from now on, or the album would vanish.
    with(|s| {
        if s.hidden.remove(&to) {
            save_hidden(&s.hidden);
        }
    });
    fill_libraries();
    let all: Vec<String> = libs.iter().map(|l| l.name.clone()).collect();
    spawn("Moving the album", move |lib| lib.move_album(&all, &name, &to).map(|_| ()));
}

/// A new name for the album shown, on every photo in it.
unsafe fn rename_album(owner: HWND) {
    let Some((View::Album(name), libs)) = with(|s| (s.view.clone(), s.libraries.iter().map(|l| l.name.clone()).collect::<Vec<_>>())) else { return };
    let Some(to) = crate::dialogs::ask_text(owner, "Rename the album", "A new name for the album (on each of its photos):", &name) else { return };
    let to = to.trim().to_string();
    if to.is_empty() || to == name {
        return;
    }
    let keys: Vec<Key> = with(|s| s.all.iter().filter(|p| in_album(p, &name)).map(canvas::key).collect()).unwrap_or_default();
    let (n2, t2) = (name.clone(), to.clone());
    update_photos(&keys, move |p| {
        let mut names: Vec<String> = p.albums().into_iter().filter(|a| !a.eq_ignore_ascii_case(&n2) && !a.eq_ignore_ascii_case(&t2)).collect();
        names.push(t2.clone());
        set_names(p, ALBUM, names);
    });
    let t3 = to.clone();
    with(|s| {
        s.view = View::Album(t3);
        refilter(s);
    });
    spawn("Renaming the album", move |lib| lib.rename_album(&libs, &name, &to));
}

/// Announces live in the retifs app: it's opened (the libraries heard of,
/// to join one, are there).
unsafe fn announces(owner: HWND) {
    let installed = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Programs").join("retifs").join("retifs-desktop.exe");
    let exe = if installed.exists() { installed.display().to_string() } else { "retifs-desktop.exe".into() };
    let r = ShellExecuteW(owner, w!("open"), &wide(&exe), w!("--show"), None, SW_SHOWNORMAL);
    if r.0 as isize <= 32 {
        note("The retifs app isn't installed here: its Announces page lists the libraries heard of.");
        show_status();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Labels on the window's own background.
        WM_CTLCOLORSTATIC => {
            let dc = HDC(wparam.0 as _);
            SetBkColor(dc, COLORREF(GetSysColor(COLOR_WINDOW)));
            let gray = controls().map(|c| c.status.0 == lparam.0 as _).unwrap_or(false);
            SetTextColor(dc, COLORREF(GetSysColor(if gray { COLOR_GRAYTEXT } else { COLOR_WINDOWTEXT })));
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
        WM_PAINT => {
            // A line between the libraries and the pictures.
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let x = side_width(hwnd);
            let line = RECT { left: x, top: 0, right: x + scale(hwnd, 1), bottom: rc.bottom };
            FillRect(dc, &line, GetSysColorBrush(COLOR_3DLIGHT));
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_SIZE => {
            layout(hwnd);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // A new font for the new DPI, then the size Windows suggests.
            if let Some(mut c) = controls() {
                let font = message_font(hwnd);
                for h in [c.announces, c.libs_label, c.libs, c.status, c.tabs, c.back, c.prev, c.next, c.title, c.fav, c.add, c.remove, c.clear, c.library, c.rename] {
                    SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
                }
                let _ = DeleteObject(c.font);
                c.font = font;
                CONTROLS.with(|x| x.set(Some(c)));
            }
            let r = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            layout(hwnd);
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let m = &mut *(lparam.0 as *mut MINMAXINFO);
            m.ptMinTrackSize = POINT { x: scale(hwnd, 760), y: scale(hwnd, 480) };
            LRESULT(0)
        }
        WM_NOTIFY => {
            let hdr = &*(lparam.0 as *const NMHDR);
            if hdr.idFrom == ID_TABS as usize && hdr.code == TCN_SELCHANGE {
                let tabs = controls().map(|c| c.tabs).unwrap_or_default();
                let albums = SendMessageW(tabs, TCM_GETCURSEL, WPARAM(0), LPARAM(0)).0 == 1;
                command(if albums { ID_SHOW_ALBUMS } else { ID_SHOW_PHOTOS });
            }
            if hdr.idFrom == ID_LIBS as usize && hdr.code == LVN_ITEMCHANGED && !FILLING.with(|f| f.get()) {
                let n = &*(lparam.0 as *const NMLISTVIEW);
                let image = |s: u32| (s & LVIS_STATEIMAGEMASK.0) >> 12;
                if (n.uChanged.0 & LVIF_STATE.0) != 0 && image(n.uNewState) != image(n.uOldState) && image(n.uOldState) != 0 && n.iItem >= 0 {
                    library_toggled(n.iItem as usize, image(n.uNewState) == 2);
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            command((wparam.0 & 0xffff) as i32);
            LRESULT(0)
        }
        WM_LOADED => {
            loaded();
            LRESULT(0)
        }
        WM_THUMBS => {
            canvas::thumbs_arrived();
            LRESULT(0)
        }
        WM_IMAGE => {
            canvas::images_arrived();
            LRESULT(0)
        }
        WM_NOTE => {
            show_status();
            LRESULT(0)
        }
        WM_RELAYOUT => {
            layout(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            if let Some(c) = controls() {
                let _ = DeleteObject(c.font);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
