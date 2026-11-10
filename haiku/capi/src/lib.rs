//! retifs Photos for Haiku: the shared core to C (and so to the BeAPI front
//! end), JSON in and out, images as bytes. The same calls as the GTK front
//! end makes.
//!
//! The last load (`rp_load`) is kept here, so the other calls name photos
//! and people by ID, and albums by their name in lower case (an album is a
//! name on its photos, in any case). Every returned string is the caller's to free
//! with `rp_free`, every image with `rp_free_bytes`. Calls block: the front
//! end makes them off its window threads, one at a time (but for
//! `rp_wait_change`, which waits beside them).

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int};
use std::sync::{Mutex, OnceLock};

use retifs_photos_core::{Album, Library, Person, Photo, albums_of};
use serde_json::{Value, json};

#[derive(Default)]
struct Loaded {
    libraries: Vec<String>,
    /// Each library's change count as it was read.
    counts: HashMap<String, u64>,
    photos: HashMap<String, Photo>,
    albums: HashMap<String, Album>,
    people: HashMap<String, Person>,
}

fn library() -> &'static Library {
    static LIBRARY: OnceLock<Library> = OnceLock::new();
    LIBRARY.get_or_init(Library::new)
}

fn loaded() -> std::sync::MutexGuard<'static, Loaded> {
    static LOADED: OnceLock<Mutex<Loaded>> = OnceLock::new();
    LOADED.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn out(v: Value) -> *mut c_char {
    CString::new(v.to_string().replace('\0', " ")).expect("no NUL left").into_raw()
}

fn done(r: Result<(), String>) -> *mut c_char {
    out(match r {
        Ok(()) => json!({}),
        Err(e) => json!({ "error": e }),
    })
}

/// # Safety
/// `s` is null or a NUL-terminated string.
unsafe fn arg(s: *const c_char) -> String {
    if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }
}

fn ids(list: Vec<(String, String)>) -> Vec<String> {
    list.into_iter().map(|(id, _)| id).collect()
}

/// A photo's albums by ID (the name in lower case).
fn album_ids(p: &Photo) -> Vec<String> {
    p.albums().iter().map(|n| n.to_lowercase()).collect()
}

/// Everything the window shows, read again: `{"libraries", "photos": [{"ns",
/// "id", "title", "month", "favorite", "albums": [id], "people": [id]}]`
/// (newest first), `"albums": [{"id", "name", "cover"}]`, `"people": [{"id",
/// "name", "photo"}]`, and `"error"` when retifsd doesn't answer.
#[unsafe(no_mangle)]
pub extern "C" fn rp_load() -> *mut c_char {
    let lib = library();
    let (libraries, counts, photos) = match lib.libraries() {
        Ok(libs) => {
            // Counted first: a change while reading is another load, not missed.
            let counts = lib.change_counts(&libs);
            let photos = lib.photos(&libs);
            (libs, counts, photos)
        }
        Err(e) => (Vec::new(), HashMap::new(), Err(e)),
    };
    let people = lib.people(&libraries);
    let error = photos.as_ref().err().cloned();
    let photos = photos.unwrap_or_default();
    let albums = albums_of(&photos);
    let v = json!({
        "libraries": libraries,
        "photos": photos.iter().map(|p| json!({
            "ns": p.ns, "id": p.id, "title": p.title(), "month": p.month(),
            "favorite": p.favorite(), "albums": album_ids(p), "people": ids(p.people()),
        })).collect::<Vec<_>>(),
        "albums": albums.iter().map(|a| json!({ "id": a.name.to_lowercase(), "name": a.name, "cover": a.cover.as_ref().map(|(_, id)| id) })).collect::<Vec<_>>(),
        "people": people.iter().map(|p| json!({ "id": p.id, "name": p.name, "photo": p.photo })).collect::<Vec<_>>(),
        "error": error,
    });
    let mut l = loaded();
    l.libraries = libraries;
    l.counts = counts;
    l.photos = photos.into_iter().map(|p| (p.id.clone(), p)).collect();
    l.albums = albums.into_iter().map(|a| (a.name.to_lowercase(), a)).collect();
    l.people = people.into_iter().map(|p| (p.id.clone(), p)).collect();
    out(v)
}

/// What the viewer shows of photo `id`: `{"title", "ns", "favorite",
/// "details": [[label, value]], "people": [[id, name]], "albums": [id]}`.
///
/// # Safety
/// `id` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_details(id: *const c_char) -> *mut c_char {
    let id = unsafe { arg(id) };
    let l = loaded();
    let Some(p) = l.photos.get(&id) else { return out(json!({ "error": "no such photo" })) };
    let people: Vec<Value> = p
        .people()
        .into_iter()
        .map(|(pid, hint)| {
            let name = l.people.get(&pid).map(|x| x.name.clone()).unwrap_or(hint);
            json!([pid, name])
        })
        .collect();
    out(json!({
        "title": p.title(),
        "ns": p.ns,
        "favorite": p.favorite(),
        "details": p.details(),
        "people": people,
        "albums": album_ids(p),
    }))
}

/// The bytes of photo `id`'s block for `role` (`thumb`, `preview`), at most
/// `max`; null if there is none (or it's bigger). Its length in `*len`.
///
/// # Safety
/// `id` and `role` are NUL-terminated strings; `len` points to a size_t.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_image(id: *const c_char, role: *const c_char, max: u64, len: *mut usize) -> *mut u8 {
    let (id, role) = unsafe { (arg(id), arg(role)) };
    let photo = loaded().photos.get(&id).cloned();
    let Some(bytes) = photo.and_then(|p| library().image(&p, &role, max)) else {
        unsafe { *len = 0 };
        return std::ptr::null_mut();
    };
    let boxed = bytes.into_boxed_slice();
    unsafe { *len = boxed.len() };
    Box::into_raw(boxed) as *mut u8
}

/// Frees an image this library returned.
///
/// # Safety
/// `bytes` and `len` came from `rp_image` (or `bytes` is null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_free_bytes(bytes: *mut u8, len: usize) {
    if !bytes.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(bytes, len)) });
    }
}

/// Marks photo `id` a favourite or not: `{}` or `{"error"}`.
///
/// # Safety
/// `id` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_set_favorite(id: *const c_char, on: c_int) -> *mut c_char {
    let photo = loaded().photos.get(&unsafe { arg(id) }).cloned();
    done(photo.ok_or_else(|| "no such photo".to_string()).and_then(|p| library().set_favorite(&p, on != 0)))
}

/// Tags (or untags) contact `person` on photo `id`.
///
/// # Safety
/// The strings are NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_tag(id: *const c_char, person: *const c_char, on: c_int) -> *mut c_char {
    let (id, person) = unsafe { (arg(id), arg(person)) };
    let (photo, who) = {
        let l = loaded();
        let who = l.people.get(&person).cloned().unwrap_or(Person { ns: String::new(), id: person.clone(), name: String::new(), photo: None });
        (l.photos.get(&id).cloned(), who)
    };
    done(photo.ok_or_else(|| "no such photo".to_string()).and_then(|p| library().tag_person(&p, &who, on != 0)))
}

/// Puts photo `id` in album `album` (or takes it out).
///
/// # Safety
/// The strings are NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_file(id: *const c_char, album: *const c_char, on: c_int) -> *mut c_char {
    let (id, album) = unsafe { (arg(id), arg(album)) };
    let (photo, a) = {
        let l = loaded();
        (l.photos.get(&id).cloned(), l.albums.get(&album).cloned())
    };
    match (photo, a) {
        (Some(p), Some(a)) => done(library().file_in_album(&p, &a.name, on != 0)),
        _ => done(Err("no such photo or album".into())),
    }
}

/// A new album called `name`, with photo `with` in it (an album is there
/// once a photo is in it).
///
/// # Safety
/// The strings are NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_new_album(name: *const c_char, with: *const c_char) -> *mut c_char {
    let (name, with) = unsafe { (arg(name), arg(with)) };
    let photo = loaded().photos.get(&with).cloned();
    done(photo.ok_or_else(|| "an album starts with a photo".to_string()).and_then(|p| library().file_in_album(&p, &name, true)))
}

/// A new contact `name` in the user's address book, tagged on photo `id`.
///
/// # Safety
/// The strings are NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_new_person(name: *const c_char, id: *const c_char) -> *mut c_char {
    let (name, id) = unsafe { (arg(name), arg(id)) };
    let photo = loaded().photos.get(&id).cloned();
    let lib = library();
    done((|| {
        let person = lib.new_person(name.trim())?;
        if let Some(p) = &photo {
            lib.tag_person(p, &person, true)?;
        }
        Ok(())
    })())
}

/// Each library's change count (of the last load), `{ns: count}`: reload
/// when these differ.
#[unsafe(no_mangle)]
pub extern "C" fn rp_change_counts() -> *mut c_char {
    let libs = loaded().libraries.clone();
    out(json!(library().change_counts(&libs)))
}

/// Waits until a library of the last load changes, for at most
/// `timeout_ms`: `{"changed", "known": {ns: count}}`. `known` is this call's
/// last answer (or its "known"), or null at first; a library not in it
/// counts from the last load. retifsd answers the moment a change comes.
/// It waits with a `Library` of its own, so it may run beside the other
/// calls: on a thread of its own, called again and again.
///
/// # Safety
/// `known` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_wait_change(known: *const c_char, timeout_ms: i32) -> *mut c_char {
    static WATCHER: OnceLock<Library> = OnceLock::new();
    let given: Value = serde_json::from_str(&unsafe { arg(known) }).unwrap_or(Value::Null);
    let given = given.get("known").cloned().unwrap_or(given);
    let mut known: HashMap<String, u64> = serde_json::from_value(given).unwrap_or_default();
    {
        let l = loaded();
        known.retain(|ns, _| l.libraries.contains(ns));
        for (ns, n) in &l.counts {
            known.entry(ns.clone()).or_insert(*n);
        }
    }
    let changed = WATCHER.get_or_init(Library::new).wait_change(&mut known, std::time::Duration::from_millis(timeout_ms.max(0) as u64));
    out(json!({ "changed": changed, "known": known }))
}

/// Frees a string this library returned.
///
/// # Safety
/// `s` came from this library and isn't used afterwards (or is null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rp_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}
