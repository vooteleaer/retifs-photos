//! retifs Photos for Android: the photos core to Kotlin through JNI
//! (`org.retifs.photos.Core`), answers as JSON (Kotlin parses them with
//! org.json). The same core as the GTK and Haiku front ends.
//!
//! retifsd's socket is in the retifs app's files, out of this app's reach:
//! every connection comes from the retifs app's access service, asked
//! through `org.retifs.photos.Retifs.connect()` (a descriptor, or -1).

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::FromRawFd;
use std::sync::OnceLock;

use jni::objects::{GlobalRef, JByteArray, JClass, JString};
use jni::sys::{jboolean, jbyteArray, jint, jlong, jstring, JNI_TRUE};
use jni::{JNIEnv, JavaVM};
use libretifs::AttrValue;
use retifs_photos_core::{Library, Person, Photo};
use serde_json::{json, Value};

static VM: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

thread_local! {
    /// A core per thread (each with its own small runtime).
    static LIB: RefCell<Option<Library>> = const { RefCell::new(None) };
}

/// A connection from the access service, through Kotlin.
fn connect() -> Result<std::os::unix::net::UnixStream, String> {
    let (vm, class) = VM.get().ok_or("not set up")?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let class: &JClass = class.as_obj().into();
    let fd = env.call_static_method(class, "connect", "()I", &[]).and_then(|v| v.i()).map_err(|e| e.to_string())?;
    if fd < 0 {
        return Err("retifs hasn't let this app in, or isn't running".into());
    }
    // SAFETY: the descriptor was detached from its ParcelFileDescriptor for us.
    Ok(unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) })
}

fn with<T>(f: impl FnOnce(&Library) -> T) -> T {
    LIB.with(|l| {
        let mut l = l.borrow_mut();
        f(l.get_or_insert_with(|| Library::with_connector(Box::new(connect))))
    })
}

fn arg(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(Into::into).unwrap_or_default()
}

fn out(env: &mut JNIEnv, s: impl Into<String>) -> jstring {
    env.new_string(s.into()).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

fn list(env: &mut JNIEnv, s: &JString) -> Vec<String> {
    serde_json::from_str(&arg(env, s)).unwrap_or_default()
}

fn photo_json(p: &Photo) -> Value {
    let geo = match p.attrs.get("geo") {
        Some(AttrValue::List(g)) => match g.as_slice() {
            [AttrValue::Float(lat), AttrValue::Float(lon)] => json!([lat, lon]),
            _ => Value::Null,
        },
        _ => Value::Null,
    };
    let thumb = p.has_thumb();
    json!({
        "ns": p.ns, "id": p.id, "when": p.when(), "title": p.title(), "favorite": p.favorite(),
        "thumb": if thumb { p.block("thumb").cloned() } else { None },
        "preview": if thumb { p.block("preview").cloned() } else { None },
        "original": p.blocks.first(),
        "type": p.text("type"),
        "details": p.details(), "geo": geo,
        "albums": p.albums(), "cover_of": p.cover_of(),
        "people": p.people().into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
    })
}

fn person_json(p: &Person) -> Value {
    json!({ "ns": p.ns, "id": p.id, "name": p.name, "photo": p.photo })
}

fn str_of(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

/// A photo as the core needs it to change one: only where it is (`{ns, id}`).
fn photo_from(v: &Value) -> Photo {
    Photo { ns: str_of(v, "ns"), id: str_of(v, "id"), attrs: Default::default(), blocks: Vec::new() }
}

fn value(env: &mut JNIEnv, s: &JString) -> Value {
    serde_json::from_str(&arg(env, s)).unwrap_or_default()
}

/// Called once: the class whose static `connect()` gives connections.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_init<'l>(env: JNIEnv<'l>, _: JClass<'l>, retifs: JClass<'l>) {
    if VM.get().is_some() {
        return;
    }
    if let (Ok(vm), Ok(class)) = (env.get_java_vm(), env.new_global_ref(retifs)) {
        let _ = VM.set((vm, class));
    }
}

/// The libraries this app was let in to: `[{name, writable}]`, or `{"error"}`.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_libraries<'l>(mut env: JNIEnv<'l>, _: JClass<'l>) -> jstring {
    let r = with(|l| match l.libraries() {
        // The core always lists `me`; retifsd answers only for those the app may use.
        Ok(names) => Value::Array(
            names
                .iter()
                .filter(|n| *n != retifs_photos_core::ME)
                .map(|n| json!({ "name": n, "writable": l.writable(n) }))
                .collect(),
        ),
        Err(e) => json!({ "error": e }),
    });
    out(&mut env, r.to_string())
}

/// Every picture in `libraries` (a JSON array), newest first, or `{"error"}`.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_photos<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, libraries: JString<'l>) -> jstring {
    let libraries = list(&mut env, &libraries);
    let r = with(|l| match l.photos(&libraries) {
        Ok(p) => Value::Array(p.iter().map(photo_json).collect()),
        Err(e) => json!({ "error": e }),
    });
    out(&mut env, r.to_string())
}

/// Block `cap` of a record in `ns` if at most `max` bytes, else null.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_read<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, ns: JString<'l>, cap: JString<'l>, max: jlong) -> jbyteArray {
    let (ns, cap) = (arg(&mut env, &ns), arg(&mut env, &cap));
    match with(|l| l.read(&ns, &cap, max.max(0) as u64)) {
        Some(data) => env.byte_array_from_slice(&data).map(JByteArray::into_raw).unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// Imports the picture open at `fd` (owned from now on) into `ns`: its record ID, or "error: …".
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_importPhoto<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, ns: JString<'l>, fd: jint, path: JString<'l>, mtime_ms: jlong) -> jstring {
    use std::io::Read;
    let (ns, path) = (arg(&mut env, &ns), arg(&mut env, &path));
    // SAFETY: Kotlin detached the descriptor for us.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut bytes = Vec::new();
    let r = match file.read_to_end(&mut bytes) {
        Ok(_) => with(|l| l.import(&ns, bytes, &path, mtime_ms)).unwrap_or_else(|e| format!("error: {e}")),
        Err(e) => format!("error: {e}"),
    };
    out(&mut env, r)
}

/// Marks photos (a JSON array of `{ns, id}`) favourite, or not.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_setFavorite<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, photos: JString<'l>, on: jboolean) -> jstring {
    let photos = value(&mut env, &photos);
    let mut err = String::new();
    with(|l| {
        for p in photos.as_array().into_iter().flatten() {
            if let Err(e) = l.set_favorite(&photo_from(p), on == JNI_TRUE) {
                err = e;
            }
        }
    });
    out(&mut env, err)
}

/// The people (contacts) in `libraries`: `[{ns, id, name, photo}]`.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_people<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, libraries: JString<'l>) -> jstring {
    let libraries = list(&mut env, &libraries);
    let r = with(|l| Value::Array(l.people(&libraries).iter().map(person_json).collect()));
    out(&mut env, r.to_string())
}

/// Puts photos (a JSON array of `{ns, id}`) in album `name`, or takes them out.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_fileInAlbum<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, photos: JString<'l>, name: JString<'l>, on: jboolean) -> jstring {
    let (photos, name) = (value(&mut env, &photos), arg(&mut env, &name));
    let mut err = String::new();
    with(|l| {
        for p in photos.as_array().into_iter().flatten() {
            if let Err(e) = l.file_in_album(&photo_from(p), &name, on == JNI_TRUE) {
                err = e;
            }
        }
    });
    out(&mut env, err)
}

/// Makes `photo` (`{ns, id}`) the cover of album `name` in `libraries`.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_setCover<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, libraries: JString<'l>, name: JString<'l>, photo: JString<'l>) -> jstring {
    let (libraries, name, photo) = (list(&mut env, &libraries), arg(&mut env, &name), value(&mut env, &photo));
    let r = with(|l| l.set_cover(&libraries, &name, &photo_from(&photo)));
    out(&mut env, r.err().unwrap_or_default())
}

/// Renames album `from` to `to` on its photos in `libraries`.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_renameAlbum<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, libraries: JString<'l>, from: JString<'l>, to: JString<'l>) -> jstring {
    let (libraries, from, to) = (list(&mut env, &libraries), arg(&mut env, &from), arg(&mut env, &to));
    let r = with(|l| l.rename_album(&libraries, &from, &to));
    out(&mut env, r.err().unwrap_or_default())
}

/// Moves `photos` (`[{ns, id}]`) into library `to`; the last error, "" when fine.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_movePhotos<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, photos: JString<'l>, to: JString<'l>) -> jstring {
    let (photos, to) = (value(&mut env, &photos), arg(&mut env, &to));
    let mut err = String::new();
    with(|l| {
        for p in photos.as_array().into_iter().flatten() {
            if let Err(e) = l.move_photo(&photo_from(p), &to) {
                err = e;
            }
        }
    });
    out(&mut env, err)
}

/// Moves album `name`'s photos in `libraries` into library `to`: "" when fine, else the error.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_moveAlbum<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, libraries: JString<'l>, name: JString<'l>, to: JString<'l>) -> jstring {
    let (libraries, name, to) = (list(&mut env, &libraries), arg(&mut env, &name), arg(&mut env, &to));
    let r = with(|l| l.move_album(&libraries, &name, &to));
    out(&mut env, r.err().unwrap_or_default())
}

/// Each library's change count (`libraries` a JSON array): `{name: count}`.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_changeCounts<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, libraries: JString<'l>) -> jstring {
    let libraries = list(&mut env, &libraries);
    let r = with(|l| json!(l.change_counts(&libraries)));
    out(&mut env, r.to_string())
}

/// Waits until a library in `known` (`{name: count}`) changes, for at most
/// `timeout_ms`: `{"changed", "known"}`, the counts brought up to date. On
/// this thread's own core, so the other calls aren't held up meanwhile.
#[no_mangle]
pub extern "system" fn Java_org_retifs_photos_Core_waitChange<'l>(mut env: JNIEnv<'l>, _: JClass<'l>, known: JString<'l>, timeout_ms: jlong) -> jstring {
    let mut known: HashMap<String, u64> = serde_json::from_value(value(&mut env, &known)).unwrap_or_default();
    let changed = with(|l| l.wait_change(&mut known, std::time::Duration::from_millis(timeout_ms.max(0) as u64)));
    out(&mut env, json!({ "changed": changed, "known": known }).to_string())
}
