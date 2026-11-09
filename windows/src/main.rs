//! retifs Photos for Windows: the libraries on the left, the timeline,
//! albums and the viewer on the right. Win32 controls for everything a
//! user types into or picks from; the grid and the viewer are one window
//! drawn with Direct2D, its pictures decoded with WIC off the UI thread.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod canvas;
#[cfg(windows)]
mod dialogs;
#[cfg(windows)]
mod images;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("retifs-photos-win is the Windows front end; on Linux use retifs-photos-gtk");
}
