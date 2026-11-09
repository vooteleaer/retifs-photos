//! The pictures, drawn with Direct2D: the timeline (or an album's photos)
//! as square thumbnails under a header a month, the albums as cover tiles,
//! or one photo large with what's known of it (the viewer). Scrolling,
//! selection, keys and the thumbnails' cache are here; what an action does
//! to the libraries is in app.rs. Everything is in DIPs: the render target
//! has the window's DPI, so it's sharp at any zoom.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use retifs_photos_core::Photo;
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT};
use windows::Win32::UI::Controls::{SetScrollInfo, ShowScrollBar};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::{self, State, View};
use crate::images::{self, Fit, Pixels};

const CLASS: PCWSTR = w!("retifs-photos-canvas");
/// Draw again (asked while the state was busy).
const WM_REDRAW: u32 = WM_APP + 20;
/// Size again, the same way.
const WM_RESIZE: u32 = WM_APP + 21;
const SCROLL_TIMER: usize = 1;

const MARGIN: f32 = 16.0;
/// A thumbnail's side at least (the row's are widened to fill it).
const CELL: f32 = 150.0;
const GAP: f32 = 4.0;
const HEADER: f32 = 48.0;
const TILE: f32 = 180.0;
const TILE_GAP: f32 = 20.0;
const TILE_TEXT: f32 = 48.0;
/// The viewer's details, on the right.
const PANEL: f32 = 320.0;
/// Thumbnails kept drawn (~256 KB each): those not drawn longest go first.
const MAX_THUMBS: usize = 600;
/// Larger pictures kept for the viewer: the one shown and those next to it.
const MAX_IMAGES: usize = 8;

/// A photo, by library and record ID.
pub type Key = (String, String);

pub fn key(p: &Photo) -> Key {
    (p.ns.clone(), p.id.clone())
}

/// A row of the grid: a month's header, or thumbnails (tiles).
pub struct Row {
    pub y: f32,
    pub h: f32,
    pub header: Option<String>,
    pub first: usize,
    pub count: usize,
}

pub struct Layout {
    tiles: bool,
    cell: f32,
    gap: f32,
    rows: Vec<Row>,
    /// The row each item is in.
    row_of: Vec<usize>,
    height: f32,
}

/// One photo large, the others a key away.
pub struct Viewer {
    /// The photos it steps through (the grid's, when opened).
    pub keys: Vec<Key>,
    pub index: usize,
    /// 1: the whole photo fits.
    zoom: f32,
    pan: (f32, f32),
    /// Dragging: where the mouse and the pan were when it began.
    drag: Option<(f32, f32, f32, f32)>,
}

/// What's tied to the render target (made again if the device is lost).
struct Gfx {
    target: ID2D1HwndRenderTarget,
    /// For better scaling than the plain target has.
    dc: Option<ID2D1DeviceContext>,
    brush: ID2D1SolidColorBrush,
    /// Thumbnails by block, with the frame they were last drawn in.
    thumbs: HashMap<String, (ID2D1Bitmap, u64)>,
    /// The viewer's pictures by block: bitmap, size, last drawn.
    images: HashMap<String, (ID2D1Bitmap, f32, f32, u64)>,
}

pub struct Canvas {
    pub hwnd: HWND,
    dpi: f32,
    /// The client area, in DIPs.
    size: (f32, f32),
    gfx: Option<Gfx>,
    pub layout: Option<Layout>,
    pub scroll: f32,
    /// Where smooth scrolling is heading.
    target: f32,
    pub selected: HashSet<Key>,
    anchor: Option<usize>,
    pub focus: Option<usize>,
    /// Blocks that couldn't be read or decoded (until the next read).
    pub failed: HashSet<String>,
    frame: u64,
    /// The thumbnails asked for last (not asked again while the same).
    last_jobs: Vec<String>,
    /// The viewer's pictures being read.
    requested: HashSet<String>,
    pub viewer: Option<Viewer>,
    /// Whether the scroll bar is there now.
    bar_shown: bool,
}

impl Canvas {
    pub fn new(hwnd: HWND, dpi: f32) -> Canvas {
        Canvas {
            hwnd,
            dpi,
            size: (0.0, 0.0),
            gfx: None,
            layout: None,
            scroll: 0.0,
            target: 0.0,
            selected: HashSet::new(),
            anchor: None,
            focus: None,
            failed: HashSet::new(),
            frame: 0,
            last_jobs: Vec::new(),
            requested: HashSet::new(),
            viewer: None,
            bar_shown: true,
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    fn max_scroll(&self) -> f32 {
        self.layout.as_ref().map(|l| (l.height - self.size.1).max(0.0)).unwrap_or(0.0)
    }

    /// Straight there, no animation.
    pub fn scroll_to(&mut self, at: f32) {
        self.scroll = at.clamp(0.0, self.max_scroll());
        self.target = self.scroll;
        self.update_scrollbar();
        self.invalidate();
    }

    /// There smoothly (a timer moves it a little each frame).
    fn glide_to(&mut self, at: f32) {
        self.target = at.clamp(0.0, self.max_scroll());
        unsafe {
            SetTimer(self.hwnd, SCROLL_TIMER, 10, None);
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    fn update_scrollbar(&mut self) {
        // The viewer has none (it fills the area); the grid always has one
        // (greyed when all of it fits: no reflow as it comes and goes).
        let want = self.viewer.is_none();
        if want != self.bar_shown {
            self.bar_shown = want;
            unsafe {
                let _ = ShowScrollBar(self.hwnd, SB_VERT, want);
            }
        }
        if !want {
            return;
        }
        let (page, height) = match &self.layout {
            Some(l) => (self.size.1 as u32, l.height as i32),
            None => (0, 0),
        };
        let si = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL | SIF_DISABLENOSCROLL,
            nMin: 0,
            nMax: (height - 1).max(0),
            nPage: page,
            nPos: self.scroll as i32,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(self.hwnd, SB_VERT, &si, true);
        }
    }

    /// Px (from a message) to DIPs.
    fn dips(&self, px: i32) -> f32 {
        px as f32 * 96.0 / self.dpi
    }
}

/// DirectWrite's text formats, made once (they're in DIPs: any DPI).
struct Fonts {
    dw: IDWriteFactory,
    header: IDWriteTextFormat,
    body: IDWriteTextFormat,
    small: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    line: IDWriteTextFormat,
    line_small: IDWriteTextFormat,
    centered: IDWriteTextFormat,
    centered_big: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    glyph_small: IDWriteTextFormat,
}

thread_local! {
    static D2D: RefCell<Option<ID2D1Factory>> = const { RefCell::new(None) };
    static FONTS: RefCell<Option<std::rc::Rc<Fonts>>> = const { RefCell::new(None) };
}

fn d2d() -> Option<ID2D1Factory> {
    D2D.with(|f| {
        if f.borrow().is_none() {
            *f.borrow_mut() = unsafe { D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok() };
        }
        f.borrow().clone()
    })
}

unsafe fn format(dw: &IDWriteFactory, family: PCWSTR, size: f32, bold: bool, one_line: bool, center: bool) -> windows::core::Result<IDWriteTextFormat> {
    let weight = if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
    let f = dw.CreateTextFormat(family, None::<&IDWriteFontCollection>, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))?;
    if one_line {
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        let sign = dw.CreateEllipsisTrimmingSign(&f)?;
        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        f.SetTrimming(&trimming, &sign)?;
    }
    if center {
        f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    }
    Ok(f)
}

fn fonts() -> Option<std::rc::Rc<Fonts>> {
    FONTS.with(|f| {
        if f.borrow().is_none() {
            let made = unsafe {
                (|| -> windows::core::Result<Fonts> {
                    let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
                    let ui = w!("Segoe UI");
                    // Windows' own icon font: the star and the check mark.
                    let icons = w!("Segoe MDL2 Assets");
                    Ok(Fonts {
                        header: format(&dw, ui, 20.0, true, true, false)?,
                        body: format(&dw, ui, 14.0, false, false, false)?,
                        small: format(&dw, ui, 12.0, false, false, false)?,
                        bold: format(&dw, ui, 17.0, true, false, false)?,
                        line: format(&dw, ui, 14.0, true, true, false)?,
                        line_small: format(&dw, ui, 12.0, false, true, false)?,
                        centered: format(&dw, ui, 14.0, false, false, true)?,
                        centered_big: format(&dw, ui, 20.0, true, false, true)?,
                        glyph: format(&dw, icons, 16.0, false, true, true)?,
                        glyph_small: format(&dw, icons, 12.0, false, true, true)?,
                        dw,
                    })
                })()
                .ok()
            };
            *f.borrow_mut() = made.map(std::rc::Rc::new);
        }
        f.borrow().clone()
    })
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

fn inset(r: D2D_RECT_F, by: f32) -> D2D_RECT_F {
    rect(r.left + by, r.top + by, r.right - by, r.bottom - by)
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// A system colour (the user's theme), with alpha.
fn sys(i: SYS_COLOR_INDEX, a: f32) -> D2D1_COLOR_F {
    let c = unsafe { GetSysColor(i) };
    rgba((c & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, ((c >> 16) & 0xff) as f32 / 255.0, a)
}

fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, t: f32) -> D2D1_COLOR_F {
    rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1.0)
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub unsafe fn register(base: &WNDCLASSW) {
    let class = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: CLASS, style: CS_DBLCLKS, hbrBackground: HBRUSH::default(), ..*base };
    RegisterClassW(&class);
}

pub unsafe fn create(parent: HWND) -> HWND {
    CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, w!(""), WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_TABSTOP, 0, 0, 10, 10, parent, HMENU(200 as _), GetModuleHandleW(None).unwrap_or_default(), None).unwrap_or_default()
}

fn month_name(ym: &str) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    match (ym.get(..4), ym.get(5..7).and_then(|m| m.parse::<usize>().ok())) {
        (Some(y), Some(m)) if (1..=12).contains(&m) => format!("{} {y}", MONTHS[m - 1]),
        _ => "No date".into(),
    }
}

/// Where the grid was, to keep it there when it's laid out again: the
/// first item in view and how far down from the top it was.
pub enum Anchor {
    Photo(Key),
    Tile(usize),
}

pub fn anchor(s: &State) -> Option<(Anchor, f32)> {
    let l = s.canvas.layout.as_ref()?;
    let top = s.canvas.scroll;
    let r = l.rows.iter().find(|r| r.count > 0 && r.y + r.h > top)?;
    let a = if l.tiles { Anchor::Tile(r.first) } else { Anchor::Photo(key(s.list.get(r.first)?)) };
    Some((a, r.y - top))
}

/// The grid for the window's width: as many columns as fit, widened to
/// fill it; a header row wherever the month changes.
pub fn relayout(s: &mut State, anchor: Option<(Anchor, f32)>) {
    let width = s.canvas.size.0;
    let tiles = s.view == View::Albums;
    let n = if tiles { s.albums.len() } else { s.list.len() };
    let (min_cell, gap) = if tiles { (TILE, TILE_GAP) } else { (CELL, GAP) };
    let cols = (((width - 2.0 * MARGIN + gap) / (min_cell + gap)).floor() as usize).max(1);
    let cell = ((width - 2.0 * MARGIN - (cols - 1) as f32 * gap) / cols as f32).max(40.0);
    let mut rows = Vec::new();
    let mut row_of = vec![0; n];
    let mut y = if tiles { MARGIN } else { 0.0 };
    let mut i = 0;
    while i < n {
        let month = (!tiles).then(|| s.list[i].month());
        if let Some(m) = &month {
            rows.push(Row { y, h: HEADER, header: Some(month_name(m)), first: i, count: 0 });
            y += HEADER;
        }
        // The month's photos (or all the tiles), a row at a time.
        let mut end = i;
        while end < n && (tiles || Some(s.list[end].month()) == month) {
            end += 1;
        }
        while i < end {
            let count = cols.min(end - i);
            let h = if tiles { cell + TILE_TEXT } else { cell };
            for k in i..i + count {
                row_of[k] = rows.len();
            }
            rows.push(Row { y, h, header: None, first: i, count });
            y += h + gap;
            i += count;
        }
    }
    let height = y + MARGIN;
    let c = &mut s.canvas;
    c.layout = Some(Layout { tiles, cell, gap, rows, row_of, height });
    if c.focus.is_some_and(|f| f >= n) {
        c.focus = None;
    }
    if let Some((a, off)) = anchor {
        let found = match a {
            Anchor::Photo(k) if !tiles => s.list_index.get(&k).copied(),
            Anchor::Tile(t) if tiles && t < n => Some(t),
            _ => None,
        };
        if let (Some(i), Some(l)) = (found, s.canvas.layout.as_ref()) {
            let y = l.rows[l.row_of[i]].y;
            s.canvas.scroll = y - off;
        }
    }
    let c = &mut s.canvas;
    c.scroll = c.scroll.clamp(0.0, c.max_scroll());
    c.target = c.scroll;
    c.update_scrollbar();
    c.invalidate();
}

impl Layout {
    fn item_rect(&self, i: usize) -> D2D_RECT_F {
        let r = &self.rows[self.row_of[i]];
        let x = MARGIN + (i - r.first) as f32 * (self.cell + self.gap);
        rect(x, r.y, x + self.cell, r.y + r.h)
    }

    /// The item at `(x, y)` (y from the top of the whole grid).
    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let ri = self.rows.partition_point(|r| r.y + r.h < y);
        let r = self.rows.get(ri)?;
        if r.count == 0 || y < r.y {
            return None;
        }
        let col = ((x - MARGIN) / (self.cell + self.gap)).floor();
        if col < 0.0 || col as usize >= r.count || x - MARGIN - col * (self.cell + self.gap) > self.cell {
            return None;
        }
        Some(r.first + col as usize)
    }

    /// The item above (`-1`) or below (`1`) `i`: the same column of the
    /// next row with items, or its last.
    fn vertical(&self, i: usize, d: i32) -> usize {
        let ri = self.row_of[i];
        let col = i - self.rows[ri].first;
        let mut rj = ri as i64;
        loop {
            rj += d as i64;
            let Some(r) = (rj >= 0).then(|| self.rows.get(rj as usize)).flatten() else { return i };
            if r.count > 0 {
                return r.first + col.min(r.count - 1);
            }
        }
    }
}

/// The photos selected, in the grid's order; in the viewer, the one shown.
pub fn selected_photos(s: &State) -> Vec<Photo> {
    if let Some(v) = &s.canvas.viewer {
        return v.keys.get(v.index).and_then(|k| s.index.get(k)).map(|&i| vec![s.all[i].clone()]).unwrap_or_default();
    }
    if s.canvas.selected.is_empty() {
        return Vec::new();
    }
    s.list.iter().filter(|p| s.canvas.selected.contains(&key(p))).cloned().collect()
}

fn viewer_photo(s: &State) -> Option<&Photo> {
    let v = s.canvas.viewer.as_ref()?;
    s.index.get(v.keys.get(v.index)?).map(|&i| &s.all[i])
}

pub fn open_viewer(s: &mut State, i: usize) {
    if i >= s.list.len() {
        return;
    }
    s.canvas.viewer = Some(Viewer { keys: s.list.iter().map(key).collect(), index: i, zoom: 1.0, pan: (0.0, 0.0), drag: None });
    s.canvas.focus = Some(i);
    s.canvas.update_scrollbar();
    s.canvas.invalidate();
    app::post(app::WM_RELAYOUT);
}

/// Back to the grid, at the photo last shown.
pub fn close_viewer(s: &mut State) {
    let Some(v) = s.canvas.viewer.take() else { return };
    if let Some(i) = v.keys.get(v.index).and_then(|k| s.list_index.get(k)).copied() {
        s.canvas.focus = Some(i);
        s.canvas.anchor = Some(i);
        ensure_visible(s, i, false);
    }
    s.canvas.update_scrollbar();
    s.canvas.invalidate();
    app::post(app::WM_RELAYOUT);
}

/// The next (`1`) or previous (`-1`) photo in the viewer.
pub fn step(s: &mut State, d: i64) {
    let Some(v) = s.canvas.viewer.as_mut() else { return };
    let to = (v.index as i64 + d).clamp(0, v.keys.len() as i64 - 1) as usize;
    if to != v.index {
        v.index = to;
        v.zoom = 1.0;
        v.pan = (0.0, 0.0);
        s.canvas.invalidate();
        app::post(app::WM_RELAYOUT);
    }
}

/// Enter: the photo focused in the viewer, or the album focused.
pub fn open_focused(s: &mut State) {
    let Some(f) = s.canvas.focus else { return };
    if s.view == View::Albums {
        if let Some(a) = s.albums.get(f) {
            let name = a.name.clone();
            app::set_view(s, View::Album(name));
        }
    } else {
        open_viewer(s, f);
    }
}

fn ensure_visible(s: &mut State, i: usize, glide: bool) {
    let Some(l) = &s.canvas.layout else { return };
    if i >= l.row_of.len() {
        return;
    }
    let r = l.item_rect(i);
    // A month's first row shows its header too.
    let ri = l.row_of[i];
    let top = if ri > 0 && l.rows[ri - 1].header.is_some() { l.rows[ri - 1].y } else { r.top - MARGIN / 2.0 };
    let h = s.canvas.size.1;
    let at = if top < s.canvas.target {
        top
    } else if r.bottom + MARGIN / 2.0 > s.canvas.target + h {
        r.bottom + MARGIN / 2.0 - h
    } else {
        return;
    };
    if glide {
        s.canvas.glide_to(at);
    } else {
        s.canvas.scroll_to(at);
    }
}

unsafe fn make_gfx(c: &Canvas) -> Option<Gfx> {
    let mut rc = RECT::default();
    let _ = GetClientRect(c.hwnd, &mut rc);
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_UNKNOWN, alphaMode: D2D1_ALPHA_MODE_UNKNOWN },
        dpiX: c.dpi,
        dpiY: c.dpi,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd: c.hwnd, pixelSize: D2D_SIZE_U { width: rc.right.max(1) as u32, height: rc.bottom.max(1) as u32 }, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
    let target = d2d()?.CreateHwndRenderTarget(&props, &hwnd_props).ok()?;
    let brush = target.CreateSolidColorBrush(&rgba(0.0, 0.0, 0.0, 1.0), None).ok()?;
    let dc = target.cast::<ID2D1DeviceContext>().ok();
    Some(Gfx { target, dc, brush, thumbs: HashMap::new(), images: HashMap::new() })
}

unsafe fn bitmap(target: &ID2D1HwndRenderTarget, px: &Pixels) -> Option<ID2D1Bitmap> {
    let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
    target.CreateBitmap(D2D_SIZE_U { width: px.w, height: px.h }, Some(px.data.as_ptr() as *const _), px.w * 4, &props).ok()
}

impl Gfx {
    unsafe fn fill(&self, r: D2D_RECT_F, color: D2D1_COLOR_F) {
        self.brush.SetColor(&color);
        self.target.FillRectangle(&r, &self.brush);
    }

    unsafe fn text(&self, s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, color: D2D1_COLOR_F) {
        self.brush.SetColor(&color);
        self.target.DrawText(&utf16(s), f, &r, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }

    /// A picture scaled into `r` (high-quality where the device has it).
    unsafe fn picture(&self, b: &ID2D1Bitmap, r: D2D_RECT_F) {
        match &self.dc {
            Some(dc) => dc.DrawBitmap(b, Some(&r), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None),
            None => self.target.DrawBitmap(b, Some(&r), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
        }
    }

    /// The favourite star, white on a shadow, in the bottom-left corner of `r`.
    unsafe fn star(&self, fonts: &Fonts, r: D2D_RECT_F) {
        let at = rect(r.left + 4.0, r.bottom - 26.0, r.left + 26.0, r.bottom - 4.0);
        self.text("\u{E735}", &fonts.glyph, rect(at.left + 1.0, at.top + 1.0, at.right + 1.0, at.bottom + 1.0), rgba(0.0, 0.0, 0.0, 0.55));
        self.text("\u{E735}", &fonts.glyph, at, rgba(1.0, 1.0, 1.0, 1.0));
    }
}

/// Draws everything, then asks for the pictures it lacked.
unsafe fn paint(s: &mut State) {
    let Some(fonts) = fonts() else { return };
    let Some(mut g) = s.canvas.gfx.take().or_else(|| make_gfx(&s.canvas)) else { return };
    s.canvas.frame += 1;
    g.target.BeginDraw();
    g.target.Clear(Some(&sys(COLOR_WINDOW, 1.0)));
    if s.canvas.viewer.is_some() {
        paint_viewer(s, &mut g, &fonts);
    } else if s.view == View::Albums {
        paint_albums(s, &mut g, &fonts);
    } else {
        paint_photos(s, &mut g, &fonts);
    }
    match g.target.EndDraw(None, None) {
        Err(e) if e.code() == D2DERR_RECREATE_TARGET => {
            // The device went away: everything on it is made again.
            s.canvas.last_jobs.clear();
            s.canvas.requested.clear();
            s.canvas.invalidate();
        }
        _ => s.canvas.gfx = Some(g),
    }
    if s.canvas.viewer.is_some() {
        viewer_wants(s);
    } else {
        want_thumbs(s);
    }
}

/// Words in the middle when there's nothing to draw.
unsafe fn empty(s: &State, g: &Gfx, fonts: &Fonts, what: &str) -> bool {
    let (w, h) = s.canvas.size;
    let (title, body) = if let Some(e) = s.error.as_ref().filter(|_| s.all.is_empty()) {
        ("retifs isn't running".to_string(), format!("{e}\n\nStart it from the retifs app; Photos tries again every 15 seconds."))
    } else if let Some(r) = &s.reading {
        ("Reading librariesâ€¦".to_string(), r.clone())
    } else if s.libraries.iter().all(|l| s.hidden.contains(&l.name)) && !s.libraries.is_empty() {
        ("No libraries shown".to_string(), "Tick the libraries on the left whose photos you want here.".into())
    } else {
        return match what {
            "albums" if s.albums.is_empty() => {
                text_block(g, fonts, w, h, "No albums yet", "Select photos and add them to one with + Add to albums. An album is a name on its photos, in whichever libraries they are.");
                true
            }
            "photos" if s.list.is_empty() => {
                text_block(g, fonts, w, h, "No photos here", "Photos in the libraries shown appear here, with their thumbnails once the extractor has made them.");
                true
            }
            _ => false,
        };
    };
    if what == "photos" && !s.list.is_empty() || what == "albums" && !s.albums.is_empty() {
        return false;
    }
    text_block(g, fonts, w, h, &title, &body);
    true
}

unsafe fn text_block(g: &Gfx, fonts: &Fonts, w: f32, h: f32, title: &str, body: &str) {
    let mid = h / 2.5;
    g.text(title, &fonts.centered_big, rect(MARGIN, mid - 40.0, w - MARGIN, mid), sys(COLOR_WINDOWTEXT, 1.0));
    g.text(body, &fonts.centered, rect((w / 2.0 - 260.0).max(MARGIN), mid + 4.0, (w / 2.0 + 260.0).min(w - MARGIN), mid + 120.0), sys(COLOR_GRAYTEXT, 1.0));
}

unsafe fn paint_photos(s: &State, g: &mut Gfx, fonts: &Fonts) {
    if empty(s, g, fonts, "photos") {
        return;
    }
    let c = &s.canvas;
    let Some(l) = &c.layout else { return };
    let (w, h) = c.size;
    let top = c.scroll;
    let text = sys(COLOR_WINDOWTEXT, 1.0);
    let accent = sys(COLOR_HIGHLIGHT, 1.0);
    let placeholder = mix(sys(COLOR_WINDOW, 1.0), text, 0.08);
    let focused = GetFocus() == c.hwnd;
    let first = l.rows.partition_point(|r| r.y + r.h < top);
    for r in &l.rows[first..] {
        if r.y > top + h {
            break;
        }
        let y = r.y - top;
        if let Some(month) = &r.header {
            g.text(month, &fonts.header, rect(MARGIN, y + 14.0, w - MARGIN, y + r.h), text);
            continue;
        }
        for i in r.first..r.first + r.count {
            let p = &s.list[i];
            let cell = l.item_rect(i);
            let cell = rect(cell.left, cell.top - top, cell.right, cell.bottom - top);
            let selected = c.selected.contains(&key(p));
            let mut img = cell;
            if selected {
                g.fill(cell, mix(sys(COLOR_WINDOW, 1.0), accent, 0.25));
                img = inset(cell, (l.cell * 0.07).max(6.0));
            }
            match p.block("thumb").and_then(|cap| g.thumbs.get_mut(cap)) {
                Some((b, used)) => {
                    *used = c.frame;
                    let b = b.clone();
                    g.picture(&b, img);
                }
                None => {
                    g.fill(img, placeholder);
                    if p.block("thumb").is_some_and(|cap| c.failed.contains(cap)) {
                        // Couldn't be read (yet): a broken-picture mark.
                        g.text("\u{EB9F}", &fonts.glyph, img, sys(COLOR_GRAYTEXT, 1.0));
                    }
                }
            }
            if p.favorite() {
                g.star(fonts, img);
            }
            if selected {
                let e = D2D1_ELLIPSE { point: D2D_POINT_2F { x: cell.left + 14.0, y: cell.top + 14.0 }, radiusX: 11.0, radiusY: 11.0 };
                g.brush.SetColor(&accent);
                g.target.FillEllipse(&e, &g.brush);
                g.brush.SetColor(&rgba(1.0, 1.0, 1.0, 1.0));
                g.target.DrawEllipse(&e, &g.brush, 1.5, None::<&ID2D1StrokeStyle>);
                g.text("\u{E73E}", &fonts.glyph_small, rect(cell.left + 3.0, cell.top + 3.0, cell.left + 25.0, cell.top + 25.0), rgba(1.0, 1.0, 1.0, 1.0));
            }
            if focused && c.focus == Some(i) {
                g.brush.SetColor(&accent);
                g.target.DrawRectangle(&inset(cell, 1.0), &g.brush, 2.0, None::<&ID2D1StrokeStyle>);
            }
        }
    }
}

unsafe fn paint_albums(s: &State, g: &mut Gfx, fonts: &Fonts) {
    if empty(s, g, fonts, "albums") {
        return;
    }
    let c = &s.canvas;
    let Some(l) = &c.layout else { return };
    let h = c.size.1;
    let top = c.scroll;
    let text = sys(COLOR_WINDOWTEXT, 1.0);
    let placeholder = mix(sys(COLOR_WINDOW, 1.0), text, 0.08);
    let focused = GetFocus() == c.hwnd;
    let first = l.rows.partition_point(|r| r.y + r.h < top);
    for r in &l.rows[first..] {
        if r.y > top + h {
            break;
        }
        for i in r.first..r.first + r.count {
            let a = &s.albums[i];
            let t = l.item_rect(i);
            let cover = rect(t.left, t.top - top, t.right, t.top - top + l.cell);
            let photo = a.cover.as_ref().and_then(|k| s.index.get(k)).map(|&i| &s.all[i]);
            match photo.and_then(|p| p.block("thumb")).and_then(|cap| g.thumbs.get_mut(cap)) {
                Some((b, used)) => {
                    *used = c.frame;
                    let b = b.clone();
                    g.picture(&b, cover);
                }
                None => g.fill(cover, placeholder),
            }
            g.text(&a.name, &fonts.line, rect(cover.left, cover.bottom + 6.0, cover.right, cover.bottom + 26.0), text);
            g.text(&app::count(a.count, "photo", "photos"), &fonts.line_small, rect(cover.left, cover.bottom + 26.0, cover.right, cover.bottom + 44.0), sys(COLOR_GRAYTEXT, 1.0));
            if focused && c.focus == Some(i) {
                g.brush.SetColor(&sys(COLOR_HIGHLIGHT, 1.0));
                g.target.DrawRectangle(&inset(cover, 1.0), &g.brush, 2.0, None::<&ID2D1StrokeStyle>);
            }
        }
    }
}

/// The viewer's blocks for a photo: its preview, and its original if
/// that's another (else the preview is the original).
fn viewer_blocks(p: &Photo) -> (Option<String>, Option<String>) {
    let preview = p.block("preview").cloned();
    let original = p.blocks.first().filter(|o| p.has_thumb() && Some(*o) != preview.as_ref()).cloned();
    (preview, original)
}

/// The image area: the window less the details panel (none when narrow).
fn image_area(s: &State) -> D2D_RECT_F {
    let (w, h) = s.canvas.size;
    let panel = if w >= 2.2 * PANEL { PANEL } else { 0.0 };
    rect(0.0, 0.0, w - panel, h)
}

/// The best picture there is now for the photo shown: the original once
/// zoomed in and read, else the preview.
fn shown_image(s: &State, g: &Gfx) -> Option<(ID2D1Bitmap, f32, f32)> {
    let p = viewer_photo(s)?;
    let v = s.canvas.viewer.as_ref()?;
    let (preview, original) = viewer_blocks(p);
    let get = |cap: &Option<String>| cap.as_ref().and_then(|c| g.images.get(c)).map(|(b, w, h, _)| (b.clone(), *w, *h));
    if v.zoom > 1.0 {
        get(&original).or_else(|| get(&preview))
    } else {
        get(&preview).or_else(|| get(&original))
    }
}

/// Where the picture goes: fitted to the area, zoomed and panned.
fn image_rect(area: D2D_RECT_F, iw: f32, ih: f32, v: &Viewer) -> D2D_RECT_F {
    let (aw, ah) = (area.right - area.left, area.bottom - area.top);
    let k = (aw / iw).min(ah / ih) * v.zoom;
    let (w, h) = (iw * k, ih * k);
    let cx = area.left + aw / 2.0 + v.pan.0;
    let cy = area.top + ah / 2.0 + v.pan.1;
    rect(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0)
}

/// Keeps a zoomed picture covering the area (no empty margins dragged in).
fn clamp_pan(area: D2D_RECT_F, iw: f32, ih: f32, v: &mut Viewer) {
    let (aw, ah) = (area.right - area.left, area.bottom - area.top);
    let k = (aw / iw).min(ah / ih) * v.zoom;
    let mx = ((iw * k - aw) / 2.0).max(0.0);
    let my = ((ih * k - ah) / 2.0).max(0.0);
    v.pan = (v.pan.0.clamp(-mx, mx), v.pan.1.clamp(-my, my));
}

unsafe fn paint_viewer(s: &mut State, g: &mut Gfx, fonts: &Fonts) {
    let w = s.canvas.size.0;
    let area = image_area(s);
    g.fill(area, rgba(0.09, 0.09, 0.09, 1.0));
    let Some(p) = viewer_photo(s).cloned() else { return };
    let frame = s.canvas.frame;
    let (preview, original) = viewer_blocks(&p);
    for cap in [&preview, &original].into_iter().flatten() {
        if let Some(e) = g.images.get_mut(cap) {
            e.3 = frame;
        }
    }
    match shown_image(s, g) {
        Some((b, iw, ih)) => {
            let v = s.canvas.viewer.as_ref().unwrap();
            g.target.PushAxisAlignedClip(&area, D2D1_ANTIALIAS_MODE_ALIASED);
            g.picture(&b, image_rect(area, iw, ih, v));
            g.target.PopAxisAlignedClip();
        }
        None => {
            let gone = preview.as_ref().is_some_and(|c| s.canvas.failed.contains(c));
            let say = if gone { "This picture can't be read here yet." } else { "Loadingâ€¦" };
            g.text(say, &fonts.centered, area, rgba(0.85, 0.85, 0.85, 1.0));
        }
    }
    if area.right >= w {
        return;
    }
    // The details: what's known of it, its albums.
    let x = area.right + 20.0;
    let width = w - x - MARGIN;
    let text = sys(COLOR_WINDOWTEXT, 1.0);
    let gray = sys(COLOR_GRAYTEXT, 1.0);
    let mut y = 18.0;
    let mut block = |s: &str, f: &IDWriteTextFormat, color: D2D1_COLOR_F, after: f32| {
        let t = utf16(s);
        let Ok(layout) = fonts.dw.CreateTextLayout(&t, f, width, 10_000.0) else { return };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = layout.GetMetrics(&mut m);
        g.brush.SetColor(&color);
        g.target.DrawTextLayout(D2D_POINT_2F { x, y }, &layout, &g.brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        y += m.height + after;
    };
    block(&p.title(), &fonts.bold, text, 14.0);
    for (label, value) in p.details() {
        block(&label, &fonts.small, gray, 1.0);
        block(&value, &fonts.body, text, 10.0);
    }
    let albums = p.albums();
    block("Albums", &fonts.small, gray, 1.0);
    block(&if albums.is_empty() { "None yet: + Add to albums".to_string() } else { albums.join(", ") }, &fonts.body, text, 10.0);
    if p.favorite() {
        block("â˜… Favourite", &fonts.body, text, 10.0);
    }
}

/// Asks the thumbnail workers for what's in view first, then a screen
/// below and one above; what's scrolled past isn't read.
unsafe fn want_thumbs(s: &mut State) {
    let c = &s.canvas;
    let (Some(l), Some(g)) = (&c.layout, &c.gfx) else { return };
    let h = c.size.1;
    let range = |from: f32, to: f32| {
        let a = l.rows.partition_point(|r| r.y + r.h < from);
        let b = l.rows.partition_point(|r| r.y <= to);
        l.rows[a..b.max(a)].iter().flat_map(|r| r.first..r.first + r.count).collect::<Vec<_>>()
    };
    let mut order = range(c.scroll, c.scroll + h);
    order.extend(range(c.scroll + h, c.scroll + 3.0 * h));
    order.extend(range(c.scroll - h, c.scroll).into_iter().rev());
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for i in order {
        let photo = if l.tiles { s.albums.get(i).and_then(|a| a.cover.as_ref()).and_then(|k| s.index.get(k)).map(|&j| &s.all[j]) } else { s.list.get(i) };
        let Some(p) = photo else { continue };
        let Some(cap) = p.block("thumb") else { continue };
        if g.thumbs.contains_key(cap) || c.failed.contains(cap) || !seen.insert(cap.clone()) {
            continue;
        }
        jobs.push(images::Job { ns: p.ns.clone(), cap: cap.clone(), whole: !p.has_thumb() });
    }
    let caps: Vec<String> = jobs.iter().map(|j| j.cap.clone()).collect();
    if caps != s.canvas.last_jobs {
        s.canvas.last_jobs = caps;
        images::want(jobs);
    }
}

/// The viewer's pictures: the preview of the photo shown, its original
/// once zoomed in, and the previews next to it once it's there.
unsafe fn viewer_wants(s: &mut State) {
    let Some(v) = &s.canvas.viewer else { return };
    let (index, zoom, keys) = (v.index, v.zoom, v.keys.clone());
    let photo = |i: usize| keys.get(i).and_then(|k| s.index.get(k)).map(|&j| s.all[j].clone());
    let Some(p) = photo(index) else { return };
    let (preview, original) = viewer_blocks(&p);
    let mut wanted = vec![(p.clone(), preview.clone(), false)];
    if zoom > 1.2 {
        wanted.push((p.clone(), original, true));
    }
    let have_preview = preview.as_ref().zip(s.canvas.gfx.as_ref()).is_some_and(|(c, g)| g.images.contains_key(c));
    if have_preview {
        for i in [index + 1, index.wrapping_sub(1)] {
            if let Some(q) = photo(i) {
                let b = viewer_blocks(&q).0;
                wanted.push((q, b, false));
            }
        }
    }
    for (p, cap, original) in wanted {
        let Some(cap) = cap else { continue };
        let have = s.canvas.gfx.as_ref().is_some_and(|g| g.images.contains_key(&cap));
        if have || s.canvas.requested.contains(&cap) || s.canvas.failed.contains(&cap) {
            continue;
        }
        s.canvas.requested.insert(cap.clone());
        // Thumbnails and previews are made upright; the content is turned as its EXIF says.
        let orientation = if p.blocks.first() == Some(&cap) {
            match p.attrs.get("exif.orientation") {
                Some(libretifs::AttrValue::Int(o)) => *o,
                _ => 1,
            }
        } else {
            1
        };
        let (max, fit) = if original { (256 << 20, Fit::Within(6000)) } else { (64 << 20, Fit::Within(2560)) };
        images::load(p.ns.clone(), cap, max, fit, orientation);
    }
}

/// Thumbnails read: onto the device, the oldest let go of past the limit.
pub unsafe fn thumbs_arrived() {
    let done = images::take_done();
    if done.is_empty() {
        return;
    }
    let mut keep = Some(done);
    let r = app::with(|s| {
        let done = keep.take().unwrap();
        if s.canvas.gfx.is_none() {
            s.canvas.gfx = make_gfx(&s.canvas);
        }
        let frame = s.canvas.frame;
        let Some(g) = s.canvas.gfx.as_mut() else { return };
        for (cap, px) in done {
            match px.and_then(|px| bitmap(&g.target, &px)) {
                Some(b) => {
                    g.thumbs.insert(cap, (b, frame));
                }
                None => {
                    s.canvas.failed.insert(cap);
                }
            }
        }
        if g.thumbs.len() > MAX_THUMBS {
            let mut by_age: Vec<(u64, String)> = g.thumbs.iter().map(|(k, (_, used))| (*used, k.clone())).collect();
            by_age.sort();
            for (_, k) in by_age.into_iter().take(g.thumbs.len() - MAX_THUMBS * 9 / 10) {
                g.thumbs.remove(&k);
            }
            // Those let go may be wanted again later.
            s.canvas.last_jobs.clear();
        }
        s.canvas.invalidate();
    });
    if r.is_none() {
        // Busy: put them back for the next message.
        if let Some(done) = keep {
            for d in done {
                images::put_back(d);
            }
        }
        app::post(app::WM_THUMBS);
    }
}

/// The viewer's pictures read.
pub unsafe fn images_arrived() {
    let loaded = images::take_loaded();
    if loaded.is_empty() {
        return;
    }
    let mut keep = Some(loaded);
    let r = app::with(|s| {
        let loaded = keep.take().unwrap();
        if s.canvas.gfx.is_none() {
            s.canvas.gfx = make_gfx(&s.canvas);
        }
        let current: Vec<String> = viewer_photo(s).map(|p| {
            let (a, b) = viewer_blocks(p);
            [a, b].into_iter().flatten().collect()
        }).unwrap_or_default();
        let frame = s.canvas.frame;
        let Some(g) = s.canvas.gfx.as_mut() else { return };
        for (cap, px) in loaded {
            s.canvas.requested.remove(&cap);
            match px.as_ref().and_then(|px| bitmap(&g.target, px).map(|b| (b, px.w as f32, px.h as f32))) {
                Some((b, w, h)) => {
                    g.images.insert(cap, (b, w, h, frame));
                }
                None => {
                    s.canvas.failed.insert(cap);
                }
            }
        }
        while g.images.len() > MAX_IMAGES {
            let Some(oldest) = g.images.iter().filter(|(k, _)| !current.contains(k)).min_by_key(|(_, e)| e.3).map(|(k, _)| k.clone()) else { break };
            g.images.remove(&oldest);
        }
        s.canvas.invalidate();
    });
    if r.is_none() {
        if let Some(loaded) = keep {
            for d in loaded {
                images::put_back_loaded(d);
            }
        }
        app::post(app::WM_IMAGE);
    }
}

/// The window's new size (and DPI, if it moved to another screen).
fn resized(s: &mut State) {
    let anchor = anchor(s);
    let c = &mut s.canvas;
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(c.hwnd, &mut rc);
        let dpi = GetDpiForWindow(c.hwnd).max(96) as f32;
        if let Some(g) = &c.gfx {
            if dpi != c.dpi {
                g.target.SetDpi(dpi, dpi);
            }
            let _ = g.target.Resize(&D2D_SIZE_U { width: rc.right.max(1) as u32, height: rc.bottom.max(1) as u32 });
        }
        c.dpi = dpi;
    }
    c.size = (c.dips(rc.right), c.dips(rc.bottom));
    if let Some(v) = c.viewer.as_mut() {
        v.zoom = 1.0;
        v.pan = (0.0, 0.0);
    }
    relayout(s, anchor);
}

fn animate(s: &mut State) {
    let c = &mut s.canvas;
    let d = c.target - c.scroll;
    if d.abs() < 0.5 {
        c.scroll = c.target;
        unsafe {
            let _ = KillTimer(c.hwnd, SCROLL_TIMER);
        }
    } else {
        c.scroll += d * 0.3;
    }
    c.update_scrollbar();
    c.invalidate();
}

fn wheel(s: &mut State, delta: f32, x: f32, y: f32) {
    if let Some(v) = s.canvas.viewer.as_mut() {
        // Zooms about the point under the mouse.
        let old = v.zoom;
        v.zoom = (v.zoom * 1.25f32.powf(delta / 120.0)).clamp(1.0, 16.0);
        let area = image_area(s);
        let c = (area.left + (area.right - area.left) / 2.0, area.top + (area.bottom - area.top) / 2.0);
        let v = s.canvas.viewer.as_mut().unwrap();
        let k = v.zoom / old;
        v.pan = (x - c.0 - (x - c.0 - v.pan.0) * k, y - c.1 - (y - c.1 - v.pan.1) * k);
        if v.zoom <= 1.0 {
            v.pan = (0.0, 0.0);
        }
        if let Some((_, iw, ih)) = s.canvas.gfx.as_ref().and_then(|g| shown_image(s, g)) {
            clamp_pan(area, iw, ih, s.canvas.viewer.as_mut().unwrap());
        }
        s.canvas.invalidate();
        return;
    }
    let lines = {
        let mut n = 3u32;
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut n as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        n.clamp(1, 10) as f32
    };
    let at = s.canvas.target - delta / 120.0 * lines * 40.0;
    s.canvas.glide_to(at);
}

/// A click: selects (Ctrl adds or takes away, Shift a range); an album opens.
fn click(s: &mut State, x: f32, y: f32, ctrl: bool, shift: bool) {
    let c = &mut s.canvas;
    let Some(l) = &c.layout else { return };
    let hit = l.hit(x, y + c.scroll);
    if l.tiles {
        if let Some(i) = hit {
            c.focus = Some(i);
            let name = s.albums[i].name.clone();
            app::set_view(s, View::Album(name));
        }
        return;
    }
    match hit {
        Some(i) => {
            let k = key(&s.list[i]);
            if shift {
                let from = c.anchor.unwrap_or(i);
                if !ctrl {
                    c.selected.clear();
                }
                for j in from.min(i)..=from.max(i) {
                    c.selected.insert(key(&s.list[j]));
                }
            } else if ctrl {
                if !c.selected.remove(&k) {
                    c.selected.insert(k);
                }
                c.anchor = Some(i);
            } else {
                c.selected.clear();
                c.selected.insert(k);
                c.anchor = Some(i);
            }
            c.focus = Some(i);
        }
        None if !ctrl => c.clear_selection(),
        None => {}
    }
    c.invalidate();
    app::post(app::WM_RELAYOUT);
}

/// A key: moves in the grid (Shift selects along), opens, steps in the viewer.
fn key_down(s: &mut State, vk: VIRTUAL_KEY) -> bool {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    if s.canvas.viewer.is_some() {
        match vk {
            VK_LEFT | VK_UP | VK_PRIOR | VK_BACK => step(s, -1),
            VK_RIGHT | VK_DOWN | VK_NEXT | VK_SPACE => step(s, 1),
            VK_HOME => step(s, -(1 << 30)),
            VK_END => step(s, 1 << 30),
            VK_ESCAPE | VK_RETURN => close_viewer(s),
            VK_ADD | VK_OEM_PLUS | VK_SUBTRACT | VK_OEM_MINUS => {
                let (w, h) = s.canvas.size;
                let d = if matches!(vk, VK_ADD | VK_OEM_PLUS) { 120.0 } else { -120.0 };
                wheel(s, d, image_area(s).right / 2.0, h / 2.0 + 0.0 * w);
            }
            _ => return false,
        }
        return true;
    }
    let Some(l) = &s.canvas.layout else { return false };
    let n = l.row_of.len();
    if n == 0 {
        return false;
    }
    let f = s.canvas.focus.unwrap_or(0).min(n - 1);
    let page = ((s.canvas.size.1 / (l.cell + l.gap)).floor() as i32).max(1);
    let to = match vk {
        VK_LEFT => f.saturating_sub(1),
        VK_RIGHT => (f + 1).min(n - 1),
        VK_UP => l.vertical(f, -1),
        VK_DOWN => l.vertical(f, 1),
        VK_PRIOR => (0..page).fold(f, |i, _| l.vertical(i, -1)),
        VK_NEXT => (0..page).fold(f, |i, _| l.vertical(i, 1)),
        VK_HOME => 0,
        VK_END => n - 1,
        VK_RETURN => {
            s.canvas.focus = Some(f);
            open_focused(s);
            return true;
        }
        VK_SPACE if !l.tiles => {
            let k = key(&s.list[f]);
            if !s.canvas.selected.remove(&k) {
                s.canvas.selected.insert(k);
            }
            s.canvas.anchor = Some(f);
            s.canvas.focus = Some(f);
            s.canvas.invalidate();
            app::post(app::WM_RELAYOUT);
            return true;
        }
        VK_ESCAPE | VK_BACK => {
            if !s.canvas.selected.is_empty() {
                s.canvas.clear_selection();
                s.canvas.invalidate();
                app::post(app::WM_RELAYOUT);
            } else if matches!(s.view, View::Album(_)) {
                app::set_view(s, View::Albums);
            }
            return true;
        }
        _ => return false,
    };
    s.canvas.focus = Some(to);
    if !l.tiles && !ctrl {
        if shift {
            let from = s.canvas.anchor.unwrap_or(f);
            s.canvas.selected = (from.min(to)..=from.max(to)).map(|j| key(&s.list[j])).collect();
            s.canvas.anchor = Some(from);
        } else {
            s.canvas.selected = [key(&s.list[to])].into_iter().collect();
            s.canvas.anchor = Some(to);
        }
        app::post(app::WM_RELAYOUT);
    }
    ensure_visible(s, to, true);
    s.canvas.invalidate();
    true
}

/// The right-click menu's items for the point clicked (selecting what's
/// under it first, as Explorer does): `(id, text, checked)`.
fn context_items(s: &mut State, at: Option<(f32, f32)>) -> Vec<(i32, &'static str, bool)> {
    let mut items = Vec::new();
    if s.canvas.viewer.is_some() {
        let fav = selected_photos(s).first().is_some_and(|p| p.favorite());
        items.push((app::ID_FAV, "Favourite", fav));
        items.push((app::ID_ADD, "Add to albumsâ€¦", false));
        if matches!(s.view, View::Album(_)) {
            items.push((app::ID_COVER, "Make the album's cover", false));
        }
        items.push((0, "", false));
        items.push((app::ID_BACK, "Back to the photos", false));
        return items;
    }
    let Some(l) = &s.canvas.layout else { return items };
    if l.tiles {
        if let Some(i) = at.and_then(|(x, y)| l.hit(x, y + s.canvas.scroll)) {
            s.canvas.focus = Some(i);
            items.push((app::ID_OPEN, "Open", false));
        }
        return items;
    }
    if let Some(i) = at.and_then(|(x, y)| l.hit(x, y + s.canvas.scroll)) {
        let k = key(&s.list[i]);
        if !s.canvas.selected.contains(&k) {
            s.canvas.selected = [k].into_iter().collect();
            s.canvas.anchor = Some(i);
        }
        s.canvas.focus = Some(i);
        s.canvas.invalidate();
        app::post(app::WM_RELAYOUT);
    }
    let chosen = selected_photos(s);
    if !chosen.is_empty() {
        if chosen.len() == 1 {
            items.push((app::ID_OPEN, "Open", false));
            items.push((0, "", false));
        }
        items.push((app::ID_FAV, "Favourite", chosen.iter().all(|p| p.favorite())));
        items.push((app::ID_ADD, "Add to albumsâ€¦", false));
        if matches!(s.view, View::Album(_)) {
            items.push((app::ID_REMOVE, "Remove from the album", false));
            if chosen.len() == 1 {
                items.push((app::ID_COVER, "Make the album's cover", false));
            }
        }
        items.push((0, "", false));
    }
    items.push((app::ID_SELECT_ALL, "Select all", false));
    if !chosen.is_empty() {
        items.push((app::ID_CLEAR, "Clear selection", false));
    }
    items
}

unsafe fn context_menu(hwnd: HWND, lparam: LPARAM) {
    let (sx, sy) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
    let mut pt = POINT { x: sx, y: sy };
    let keyboard = sx == -1 && sy == -1;
    if keyboard {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        pt = POINT { x: rc.right / 3, y: rc.bottom / 3 };
        let _ = ClientToScreen(hwnd, &mut pt);
    }
    let mut local = pt;
    let _ = ScreenToClient(hwnd, &mut local);
    let Some(items) = app::with(|s| {
        let at = (!keyboard).then(|| (s.canvas.dips(local.x), s.canvas.dips(local.y)));
        context_items(s, at)
    }) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let Ok(menu) = CreatePopupMenu() else { return };
    for (id, text, checked) in &items {
        if *id == 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        } else {
            let flags = if *checked { MF_STRING | MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(menu, flags, *id as usize, &app::wide(text));
        }
    }
    if items.iter().any(|(id, _, _)| *id == app::ID_OPEN) {
        let _ = SetMenuDefaultItem(menu, app::ID_OPEN as u32, 0);
    }
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
    let _ = DestroyMenu(menu);
    if cmd.0 != 0 {
        app::command(cmd.0);
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32)
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let drawn = app::with(|s| paint(s));
            let _ = ValidateRect(hwnd, None);
            if drawn.is_none() {
                let _ = PostMessageW(hwnd, WM_REDRAW, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_REDRAW => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_SIZE | WM_RESIZE => {
            if app::with(resized).is_none() {
                let _ = PostMessageW(hwnd, WM_RESIZE, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_GETDLGCODE => {
            // Arrows, Enter and Esc are ours; Tab moves on to the next control.
            let m = lparam.0 as *const MSG;
            let ours = !m.is_null() && (*m).message == WM_KEYDOWN && matches!(VIRTUAL_KEY((*m).wParam.0 as u16), VK_RETURN | VK_ESCAPE);
            LRESULT((DLGC_WANTARROWS | DLGC_WANTCHARS | if ours { DLGC_WANTMESSAGE } else { 0 }) as isize)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xffff) as i16 as f32;
            let (x, y) = point(lparam);
            let mut pt = POINT { x, y };
            let _ = ScreenToClient(hwnd, &mut pt);
            app::with(|s| {
                let (x, y) = (s.canvas.dips(pt.x), s.canvas.dips(pt.y));
                wheel(s, delta, x, y)
            });
            LRESULT(0)
        }
        WM_VSCROLL => {
            let mut si = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_TRACKPOS, ..Default::default() };
            let _ = GetScrollInfo(hwnd, SB_VERT, &mut si);
            app::with(|s| {
                let c = &mut s.canvas;
                let page = c.size.1;
                match SCROLLBAR_COMMAND((wparam.0 & 0xffff) as i32) {
                    SB_LINEUP => c.glide_to(c.target - 60.0),
                    SB_LINEDOWN => c.glide_to(c.target + 60.0),
                    SB_PAGEUP => c.glide_to(c.target - page * 0.9),
                    SB_PAGEDOWN => c.glide_to(c.target + page * 0.9),
                    SB_TOP => c.glide_to(0.0),
                    SB_BOTTOM => c.glide_to(f32::MAX),
                    SB_THUMBTRACK | SB_THUMBPOSITION => c.scroll_to(si.nTrackPos as f32),
                    _ => {}
                }
            });
            LRESULT(0)
        }
        WM_TIMER => {
            app::with(animate);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let _ = SetFocus(hwnd);
            let (x, y) = point(lparam);
            let ctrl = (wparam.0 & MK_CONTROL.0 as usize) != 0;
            let shift = (wparam.0 & MK_SHIFT.0 as usize) != 0;
            let dragging = app::with(|s| {
                let (x, y) = (s.canvas.dips(x), s.canvas.dips(y));
                if let Some(v) = s.canvas.viewer.as_mut() {
                    if v.zoom > 1.0 {
                        v.drag = Some((x, y, v.pan.0, v.pan.1));
                        return true;
                    }
                    return false;
                }
                click(s, x, y, ctrl, shift);
                false
            });
            if dragging == Some(true) {
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            app::with(|s| {
                let (x, y) = (s.canvas.dips(x), s.canvas.dips(y));
                let area = image_area(s);
                let size = s.canvas.gfx.as_ref().and_then(|g| shown_image(s, g)).map(|(_, w, h)| (w, h));
                let Some(v) = s.canvas.viewer.as_mut() else { return };
                let Some((x0, y0, px, py)) = v.drag else { return };
                v.pan = (px + x - x0, py + y - y0);
                if let Some((iw, ih)) = size {
                    clamp_pan(area, iw, ih, v);
                }
                s.canvas.invalidate();
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            app::with(|s| {
                if let Some(v) = s.canvas.viewer.as_mut() {
                    v.drag = None;
                }
            });
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = point(lparam);
            app::with(|s| {
                let (x, y) = (s.canvas.dips(x), s.canvas.dips(y));
                if let Some(v) = s.canvas.viewer.as_mut() {
                    // In to 2.5Ã— where clicked, or back out to the whole photo.
                    if v.zoom > 1.0 {
                        v.zoom = 1.0;
                        v.pan = (0.0, 0.0);
                        s.canvas.invalidate();
                    } else {
                        wheel(s, 120.0 * 2.5f32.ln() / 1.25f32.ln(), x, y);
                    }
                    return;
                }
                let Some(l) = &s.canvas.layout else { return };
                if let (false, Some(i)) = (l.tiles, l.hit(x, y + s.canvas.scroll)) {
                    open_viewer(s, i);
                }
            });
            LRESULT(0)
        }
        WM_CONTEXTMENU => {
            context_menu(hwnd, lparam);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            if ctrl && vk.0 == b'A' as u16 {
                app::command(app::ID_SELECT_ALL);
                return LRESULT(0);
            }
            match app::with(|s| key_down(s, vk)) {
                Some(true) => LRESULT(0),
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
