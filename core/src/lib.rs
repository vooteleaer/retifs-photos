//! Photos on retifs: every picture in the user's libraries (`type` an image
//! MIME type, ADR-0025), shown small by the thumbnails the extractor keeps
//! with each (ADR-0026); albums (records photos refer to as
//! `album:<album id>`); favourites; and people, who are the user's contacts
//! (`person:<contact id>`, ADR-0024). Front ends share this.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use libretifs::{types, AttrValue, Client, RecordView, Request, Response};

pub const ME: &str = "me";
pub const ALBUM_TYPE: &str = "application/x-vnd.retifs.album";

/// A picture: where it lives and its record.
#[derive(Debug, Clone, PartialEq)]
pub struct Photo {
    pub ns: String,
    pub id: String,
    pub attrs: BTreeMap<String, AttrValue>,
    pub blocks: Vec<String>,
}

impl Photo {
    fn from_record(ns: &str, r: &RecordView) -> Photo {
        Photo { ns: ns.into(), id: r.id.clone(), attrs: r.attrs.iter().cloned().collect(), blocks: r.blocks.clone() }
    }

    pub fn text(&self, key: &str) -> Option<String> {
        match self.attrs.get(key)? {
            AttrValue::Str(s) => Some(s.clone()),
            AttrValue::Int(n) => Some(n.to_string()),
            AttrValue::Float(f) => Some(f.to_string()),
            AttrValue::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    /// When it was taken (`YYYY-MM-DDTHH:MM:SS`), else when the file was
    /// last changed, else nothing ("" sorts last).
    pub fn when(&self) -> String {
        self.text("taken").unwrap_or_else(|| match self.attrs.get("mtime") {
            Some(AttrValue::Int(ms)) => format_ms(*ms),
            _ => String::new(),
        })
    }

    /// Its month, for grouping: `YYYY-MM`.
    pub fn month(&self) -> String {
        self.when().get(..7).unwrap_or("").to_string()
    }

    pub fn title(&self) -> String {
        self.text("title")
            .or_else(|| self.text("path").map(|p| p.rsplit('/').next().unwrap_or(&p).to_string()))
            .or_else(|| self.text("name"))
            .unwrap_or_else(|| self.id[..8].to_string())
    }

    pub fn favorite(&self) -> bool {
        matches!(self.attrs.get("favorite"), Some(AttrValue::Bool(true)))
    }

    /// The block for `role` (`thumb`, `preview`; ADR-0026), else the content.
    pub fn block(&self, role: &str) -> Option<&String> {
        let i = match self.attrs.get(&format!("block:{role}")) {
            Some(AttrValue::Int(i)) if *i > 0 => *i as usize,
            _ => 0,
        };
        self.blocks.get(i).or_else(|| self.blocks.first())
    }

    /// Whether it has a small image of its own (else its whole content is shown).
    pub fn has_thumb(&self) -> bool {
        matches!(self.attrs.get("block:thumb"), Some(AttrValue::Int(i)) if (*i as usize) < self.blocks.len() && *i > 0)
    }

    /// The albums it's in: (album id, name hint).
    pub fn albums(&self) -> Vec<(String, String)> {
        self.refs("album:")
    }

    /// The people tagged in it: (contact id, name hint).
    pub fn people(&self) -> Vec<(String, String)> {
        self.refs("person:")
    }

    fn refs(&self, prefix: &str) -> Vec<(String, String)> {
        self.attrs
            .iter()
            .filter_map(|(k, v)| {
                let id = k.strip_prefix(prefix)?;
                let hint = if let AttrValue::Str(s) = v { s.clone() } else { String::new() };
                Some((id.to_string(), hint))
            })
            .collect()
    }

    /// Lines about it for the viewer: (label, value).
    pub fn details(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let when = self.when().replace('T', " ");
        if !when.is_empty() {
            out.push(("Taken".into(), when));
        }
        if let (Some(w), Some(h)) = (self.text("width"), self.text("height")) {
            let mb = match self.attrs.get("size") {
                Some(AttrValue::Int(n)) => format!(" · {:.1} MB", *n as f64 / 1_048_576.0),
                _ => String::new(),
            };
            out.push(("Size".into(), format!("{w} × {h}{mb}")));
        }
        if let Some(c) = self.text("camera") {
            out.push(("Camera".into(), c));
        }
        let exif: Vec<String> = [("exif.exposure", "s"), ("exif.fnumber", ""), ("exif.iso", ""), ("exif.focal", " mm")]
            .iter()
            .filter_map(|(k, unit)| {
                self.text(k).map(|v| match *k {
                    "exif.fnumber" => format!("f/{v}"),
                    "exif.iso" => format!("ISO {v}"),
                    _ => format!("{v}{unit}"),
                })
            })
            .collect();
        if !exif.is_empty() {
            out.push(("Exposure".into(), exif.join("  ")));
        }
        if let Some(AttrValue::List(g)) = self.attrs.get("geo") {
            if let [AttrValue::Float(lat), AttrValue::Float(lon)] = g.as_slice() {
                out.push(("Place".into(), format!("{lat:.5}, {lon:.5}")));
            }
        }
        if let Some(p) = self.text("path") {
            out.push(("File".into(), format!("{p} · {}", self.ns)));
        }
        out
    }
}

fn format_ms(ms: i64) -> String {
    // Days since 1970 to a civil date (Howard Hinnant's algorithm), no clock crate.
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem / 60 % 60, rem % 60)
}

/// An album: a record photos refer to.
#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub ns: String,
    pub id: String,
    pub name: String,
    pub cover: Option<String>,
}

/// A person: a contact (ADR-0024), with their profile photo's record ID.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub ns: String,
    pub id: String,
    pub name: String,
    pub photo: Option<String>,
}

/// The connection to `retifsd`.
pub struct Library {
    rt: tokio::runtime::Runtime,
    store: PathBuf,
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

impl Library {
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        Library { rt, store: libretifs::default_store_dir() }
    }

    fn client(&self) -> Result<Client, String> {
        self.rt.block_on(Client::connect(&self.store)).map_err(|e| e.to_string())
    }

    fn call(&self, c: &mut Client, req: Request) -> Result<Response, String> {
        match self.rt.block_on(c.call(req)).map_err(|e| e.to_string())? {
            Response::Error { message } => Err(message),
            r => Ok(r),
        }
    }

    fn query(&self, c: &mut Client, ns: &str, term: String) -> Vec<RecordView> {
        match self.call(c, Request::Query { ns: ns.into(), terms: vec![term], at: None }) {
            Ok(Response::Records { records }) => records,
            _ => Vec::new(),
        }
    }

    /// Every library on this device, `me` first.
    pub fn libraries(&self) -> Result<Vec<String>, String> {
        let mut c = self.client()?;
        let mut names = match self.call(&mut c, Request::Namespaces)? {
            Response::Names { names } => names,
            _ => Vec::new(),
        };
        names.retain(|n| n != ME && !n.starts_with("ch-"));
        names.sort_by_key(|n| n.to_lowercase());
        names.insert(0, ME.into());
        Ok(names)
    }

    /// Every picture in `libraries`, newest first.
    pub fn photos(&self, libraries: &[String]) -> Result<Vec<Photo>, String> {
        let mut c = self.client()?;
        let mut out: Vec<Photo> = Vec::new();
        for ns in libraries {
            // Image types (ADR-0025), and the words used before.
            for term in ["type^=image/", "type=image", "type=photo"] {
                out.extend(self.query(&mut c, ns, term.into()).iter().filter(|r| !r.blocks.is_empty()).map(|r| Photo::from_record(ns, r)));
            }
        }
        out.sort_by(|a, b| b.when().cmp(&a.when()).then(a.id.cmp(&b.id)));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    /// The bytes of a photo's block for `role` (`thumb`, `preview`), at most `max`.
    pub fn image(&self, photo: &Photo, role: &str, max: u64) -> Option<Vec<u8>> {
        let cap = photo.block(role)?.clone();
        let size: u64 = cap.rsplit(':').next()?.parse().ok()?;
        if size > max {
            return None;
        }
        let mut c = self.client().ok()?;
        match self.call(&mut c, Request::ReadRange { cap, ns: Some(photo.ns.clone()), offset: 0, len: size, wait_ms: 30_000 }) {
            Ok(Response::Range { data }) => Some(data),
            _ => None,
        }
    }

    /// The albums in `libraries`, by name.
    pub fn albums(&self, libraries: &[String]) -> Vec<Album> {
        let Ok(mut c) = self.client() else { return Vec::new() };
        let mut out: Vec<Album> = libraries
            .iter()
            .flat_map(|ns| {
                self.query(&mut c, ns, format!("type={ALBUM_TYPE}")).into_iter().map(move |r| {
                    let get = |k: &str| r.attrs.iter().find(|(key, _)| key == k).and_then(|(_, v)| if let AttrValue::Str(s) = v { Some(s.clone()) } else { None });
                    Album { ns: ns.clone(), id: r.id.clone(), name: get("name").unwrap_or_else(|| "Untitled album".into()), cover: get("cover") }
                })
            })
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    /// The people (contacts) in `libraries`, by name.
    pub fn people(&self, libraries: &[String]) -> Vec<Person> {
        let Ok(mut c) = self.client() else { return Vec::new() };
        let mut out: Vec<Person> = Vec::new();
        for ns in libraries {
            for term in types::terms(types::PERSON) {
                for r in self.query(&mut c, ns, term) {
                    let get = |k: &str| r.attrs.iter().find(|(key, _)| key == k).and_then(|(_, v)| if let AttrValue::Str(s) = v { Some(s.clone()) } else { None });
                    let full = [get("given"), get("family")].into_iter().flatten().collect::<Vec<_>>().join(" ");
                    let name = [get("name"), Some(full)].into_iter().flatten().find(|s| !s.trim().is_empty()).unwrap_or_else(|| "No name".into());
                    out.push(Person { ns: ns.clone(), id: r.id.clone(), name, photo: get("photo") });
                }
            }
        }
        out.sort_by_key(|p| p.name.to_lowercase());
        out
    }

    fn set(&self, ns: &str, id: &str, attrs: Vec<(String, AttrValue)>, unset: Vec<String>) -> Result<(), String> {
        let mut c = self.client()?;
        self.call(&mut c, Request::RecordSet { ns: ns.into(), id: id.into(), attrs, unset, data: None }).map(|_| ())
    }

    pub fn set_favorite(&self, p: &Photo, on: bool) -> Result<(), String> {
        if on {
            self.set(&p.ns, &p.id, vec![("favorite".into(), AttrValue::Bool(true))], vec![])
        } else {
            self.set(&p.ns, &p.id, vec![], vec!["favorite".into()])
        }
    }

    /// Tags (or untags) a person on a photo: `person:<contact id> = name`.
    pub fn tag_person(&self, p: &Photo, person: &Person, on: bool) -> Result<(), String> {
        let key = format!("person:{}", person.id);
        if on {
            self.set(&p.ns, &p.id, vec![(key, AttrValue::Str(person.name.clone()))], vec![])
        } else {
            self.set(&p.ns, &p.id, vec![], vec![key])
        }
    }

    /// Puts a photo in an album (or takes it out): `album:<album id> = name`.
    pub fn file_in_album(&self, p: &Photo, album: &Album, on: bool) -> Result<(), String> {
        let key = format!("album:{}", album.id);
        if on {
            self.set(&p.ns, &p.id, vec![(key, AttrValue::Str(album.name.clone()))], vec![])
        } else {
            self.set(&p.ns, &p.id, vec![], vec![key])
        }
    }

    pub fn set_cover(&self, album: &Album, p: &Photo) -> Result<(), String> {
        self.set(&album.ns, &album.id, vec![("cover".into(), AttrValue::Str(p.id.clone()))], vec![])
    }

    fn create(&self, ns: &str, attrs: Vec<(String, AttrValue)>) -> Result<String, String> {
        let mut c = self.client()?;
        match self.call(&mut c, Request::RecordCreate { ns: ns.into(), attrs, data: None })? {
            Response::Records { records } => records.first().map(|r| r.id.clone()).ok_or_else(|| "nothing made".into()),
            other => Err(format!("unexpected answer: {other:?}")),
        }
    }

    /// A new album in library `ns`.
    pub fn new_album(&self, ns: &str, name: &str) -> Result<Album, String> {
        let id = self.create(ns, vec![("type".into(), AttrValue::Str(ALBUM_TYPE.into())), ("name".into(), AttrValue::Str(name.into()))])?;
        Ok(Album { ns: ns.into(), id, name: name.into(), cover: None })
    }

    /// A new contact in the user's address book (`me`), for tagging someone
    /// not there yet (ADR-0024).
    pub fn new_person(&self, name: &str) -> Result<Person, String> {
        let id = self.create(ME, vec![("type".into(), AttrValue::Str(types::PERSON.into())), ("name".into(), AttrValue::Str(name.into()))])?;
        Ok(Person { ns: ME.into(), id, name: name.into(), photo: None })
    }

    /// Each library's change count: a front end reloads when these differ.
    pub fn change_counts(&self, libraries: &[String]) -> HashMap<String, u64> {
        let Ok(mut c) = self.client() else { return HashMap::new() };
        libraries
            .iter()
            .filter_map(|ns| match self.rt.block_on(c.call(Request::Watch { ns: ns.clone(), since: 0, timeout_ms: 0 })) {
                Ok(Response::Changed { count }) => Some((ns.clone(), count)),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(attrs: &[(&str, AttrValue)], blocks: usize) -> Photo {
        Photo { ns: "photos".into(), id: "0123456789abcdef".into(), attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(), blocks: (0..blocks).map(|i| format!("cap{i}")).collect() }
    }

    #[test]
    fn blocks_by_role() {
        let p = photo(&[("block:thumb", AttrValue::Int(1)), ("block:preview", AttrValue::Int(2))], 3);
        assert_eq!(p.block("thumb").unwrap(), "cap1");
        assert_eq!(p.block("preview").unwrap(), "cap2");
        assert!(p.has_thumb());
        // Content replaced: the roles point past the list, the content is shown.
        let p = photo(&[("block:thumb", AttrValue::Int(1))], 1);
        assert_eq!(p.block("thumb").unwrap(), "cap0");
        assert!(!p.has_thumb());
    }

    #[test]
    fn dates_refs_and_details() {
        let p = photo(&[("taken", AttrValue::Str("2023-04-09T11:53:13".into())), ("album:aa", AttrValue::Str("Prangli".into())), ("person:bb", AttrValue::Str("Ema".into())), ("favorite", AttrValue::Bool(true)), ("exif.fnumber", AttrValue::Str("1.8".into()))], 1);
        assert_eq!(p.month(), "2023-04");
        assert_eq!(p.albums(), [("aa".to_string(), "Prangli".to_string())]);
        assert_eq!(p.people(), [("bb".to_string(), "Ema".to_string())]);
        assert!(p.favorite());
        assert!(p.details().iter().any(|(k, v)| k == "Exposure" && v == "f/1.8"));
        let q = photo(&[("mtime", AttrValue::Int(1_759_842_332_000))], 1);
        assert_eq!(q.when(), "2025-10-07T13:05:32");
    }
}
