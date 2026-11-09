//! Pictures: read from retifsd and decoded with WIC on worker threads, then
//! handed to the UI thread as premultiplied BGRA pixels (what Direct2D
//! takes as they are). Thumbnails come from a few long-lived workers fed
//! by what the grid shows now; the viewer's larger pictures from a thread each.

use std::cell::RefCell;
use std::sync::{Condvar, Mutex};

use retifs_photos_core::Library;
use windows::core::GUID;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

/// A thumbnail's side: the extractor makes them 320 on the long side
/// (ADR-0026); cut square to this, they fill a cell at up to 160 % zoom.
pub const THUMB: u32 = 256;
/// How many thumbnails are read at once (a block not held here may take a
/// while to come from another node; the others go on meanwhile).
const WORKERS: usize = 4;

/// Decoded pixels: premultiplied BGRA, `w * 4` bytes a row.
pub struct Pixels {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// How a picture is cut and scaled.
#[derive(Clone, Copy)]
pub enum Fit {
    /// Its middle square, at most this side.
    Square(u32),
    /// Whole, its longer side at most this.
    Within(u32),
}

thread_local! {
    static WIC: RefCell<Option<IWICImagingFactory>> = const { RefCell::new(None) };
}

fn factory() -> windows::core::Result<IWICImagingFactory> {
    WIC.with(|f| {
        if let Some(f) = f.borrow().as_ref() {
            return Ok(f.clone());
        }
        unsafe {
            // Each worker thread is its own apartment; already set up is fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let made: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            *f.borrow_mut() = Some(made.clone());
            Ok(made)
        }
    })
}

/// A JPEG, PNG (or anything else WIC has a codec for) decoded, turned as
/// EXIF `orientation` says (1 for pictures already upright), cut and scaled.
pub fn decode(bytes: &[u8], fit: Fit, orientation: i64) -> Option<Pixels> {
    unsafe { decode_wic(bytes, fit, orientation).ok() }
}

unsafe fn decode_wic(bytes: &[u8], fit: Fit, orientation: i64) -> windows::core::Result<Pixels> {
    let wic = factory()?;
    let stream = wic.CreateStream()?;
    stream.InitializeFromMemory(bytes)?;
    let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null::<GUID>(), WICDecodeMetadataCacheOnDemand)?;
    let mut source: IWICBitmapSource = decoder.GetFrame(0)?.into();
    let turn = match orientation {
        2 => WICBitmapTransformFlipHorizontal,
        3 => WICBitmapTransformRotate180,
        4 => WICBitmapTransformFlipVertical,
        5 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
        6 => WICBitmapTransformRotate90,
        7 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
        8 => WICBitmapTransformRotate270,
        _ => WICBitmapTransformRotate0,
    };
    if turn != WICBitmapTransformRotate0 {
        let r = wic.CreateBitmapFlipRotator()?;
        r.Initialize(&source, turn)?;
        source = r.into();
    }
    let (mut w, mut h) = (0, 0);
    source.GetSize(&mut w, &mut h)?;
    if w == 0 || h == 0 {
        return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
    }
    // Scaled first (a JPEG decoder scales as it decodes: fast), cut after.
    let k = match fit {
        Fit::Square(side) => (side as f64 / w.min(h) as f64).min(1.0),
        Fit::Within(side) => (side as f64 / w.max(h) as f64).min(1.0),
    };
    if k < 1.0 {
        let (sw, sh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
        let s = wic.CreateBitmapScaler()?;
        s.Initialize(&source, sw, sh, WICBitmapInterpolationModeFant)?;
        source = s.into();
        (w, h) = (sw, sh);
    }
    if let Fit::Square(_) = fit {
        let side = w.min(h);
        let c = wic.CreateBitmapClipper()?;
        let rc = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        c.Initialize(&source, &rc)?;
        source = c.into();
        (w, h) = (side, side);
    }
    let conv = wic.CreateFormatConverter()?;
    conv.Initialize(&source, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
    let mut data = vec![0u8; (w * h * 4) as usize];
    conv.CopyPixels(std::ptr::null(), w * 4, &mut data)?;
    Ok(Pixels { w, h, data })
}

/// A thumbnail to read: its block (the cache's key), in library `ns`.
#[derive(Clone, PartialEq)]
pub struct Job {
    pub ns: String,
    pub cap: String,
    /// The block is the whole picture (no thumbnail of its own): it may
    /// be large, and is scaled down here.
    pub whole: bool,
}

struct Queue {
    /// Most wanted first.
    jobs: Vec<Job>,
    busy: Vec<String>,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue { jobs: Vec::new(), busy: Vec::new() });
static WAKE: Condvar = Condvar::new();
/// Thumbnails made (none: it couldn't be read or decoded), by block.
static DONE: Mutex<Vec<(String, Option<Pixels>)>> = Mutex::new(Vec::new());

pub fn start() {
    for _ in 0..WORKERS {
        std::thread::spawn(worker);
    }
}

/// What the grid wants now, most wanted first: replaces what it wanted
/// before (scrolled past, those aren't read).
pub fn want(jobs: Vec<Job>) {
    let mut q = QUEUE.lock().unwrap();
    let busy = std::mem::take(&mut q.busy);
    q.jobs = jobs.into_iter().filter(|j| !busy.contains(&j.cap)).collect();
    q.busy = busy;
    WAKE.notify_all();
}

/// The thumbnails made since last asked.
pub fn take_done() -> Vec<(String, Option<Pixels>)> {
    std::mem::take(&mut *DONE.lock().unwrap())
}

/// One taken but not used yet (the UI was busy): for next time.
pub fn put_back(done: (String, Option<Pixels>)) {
    DONE.lock().unwrap().push(done);
}

fn worker() {
    let lib = Library::new();
    loop {
        let job = {
            let mut q = QUEUE.lock().unwrap();
            loop {
                if !q.jobs.is_empty() {
                    let j = q.jobs.remove(0);
                    q.busy.push(j.cap.clone());
                    break j;
                }
                q = WAKE.wait(q).unwrap();
            }
        };
        let max = if job.whole { 64 << 20 } else { 4 << 20 };
        let pixels = lib.read(&job.ns, &job.cap, max).and_then(|b| decode(&b, Fit::Square(THUMB), 1));
        QUEUE.lock().unwrap().busy.retain(|c| *c != job.cap);
        DONE.lock().unwrap().push((job.cap, pixels));
        crate::app::post(crate::app::WM_THUMBS);
    }
}

/// A larger picture for the viewer (a preview or the original), by block.
static LOADED: Mutex<Vec<(String, Option<Pixels>)>> = Mutex::new(Vec::new());

/// Reads and decodes block `cap` of library `ns` on a thread of its own;
/// the UI thread hears of it as `WM_IMAGE` ([`take_loaded`]).
pub fn load(ns: String, cap: String, max_bytes: u64, fit: Fit, orientation: i64) {
    std::thread::spawn(move || {
        let lib = Library::new();
        let pixels = lib.read(&ns, &cap, max_bytes).and_then(|b| decode(&b, fit, orientation));
        LOADED.lock().unwrap().push((cap, pixels));
        crate::app::post(crate::app::WM_IMAGE);
    });
}

pub fn take_loaded() -> Vec<(String, Option<Pixels>)> {
    std::mem::take(&mut *LOADED.lock().unwrap())
}

pub fn put_back_loaded(loaded: (String, Option<Pixels>)) {
    LOADED.lock().unwrap().push(loaded);
}
