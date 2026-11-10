//! Photos on retifs: every picture in the user's libraries (`type` an image
//! MIME type, ADR-0025), shown small by the thumbnails the extractor keeps
//! with each (ADR-0026); albums, which are only what photos say (`album`: a
//! name, or a list of them; an album is there while a photo is in it, and
//! one name in several libraries is one album); favourites; and people, who
//! are the user's contacts (`person:<contact id>`, ADR-0024). Front ends
//! share this.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use libretifs::{types, AttrValue, Client, RecordView, Request, Response, TxStep};

pub const ME: &str = "me";
/// The albums a photo is in: a name, or a list of names.
pub const ALBUM: &str = "album";
/// The albums a photo is the cover of, the same way.
pub const COVER_OF: &str = "cover-of";
/// Albums as records of their own, before 2026-11-05: photos said
/// `album:<album record id> = name`. Moved to [`ALBUM`] when seen.
const OLD_ALBUM_TYPE: &str = "application/x-vnd.retifs.album";

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

    /// The albums it's in, by name.
    pub fn albums(&self) -> Vec<String> {
        names(self.attrs.get(ALBUM))
    }

    /// The albums it's the cover of.
    pub fn cover_of(&self) -> Vec<String> {
        names(self.attrs.get(COVER_OF))
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
        out.push(("Library".into(), self.ns.clone()));
        // Where it was before it came into the library (a phone's `DCIM/…`), not where it is.
        if let Some(p) = self.text("path") {
            out.push(("Imported from".into(), p));
        }
        out
    }
}

/// Names from an attribute: one, or a list.
fn names(v: Option<&AttrValue>) -> Vec<String> {
    match v {
        Some(AttrValue::Str(s)) if !s.trim().is_empty() => vec![s.clone()],
        Some(AttrValue::List(l)) => l.iter().filter_map(|v| if let AttrValue::Str(s) = v { Some(s.clone()) } else { None }).filter(|s| !s.trim().is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// The attribute for `names`: one is a string, more a list, none unset.
fn names_value(names: &[String]) -> Option<AttrValue> {
    match names {
        [] => None,
        [one] => Some(AttrValue::Str(one.clone())),
        many => Some(AttrValue::List(many.iter().cloned().map(AttrValue::Str).collect())),
    }
}

/// The albums `photos` are in, by name (any case is one album): how many
/// each, and its cover (the photo that says so, else the newest).
pub fn albums_of(photos: &[Photo]) -> Vec<Album> {
    let mut by: BTreeMap<String, Album> = BTreeMap::new();
    for p in photos {
        for name in p.albums() {
            let a = by.entry(name.to_lowercase()).or_insert_with(|| Album { name: name.clone(), cover: None, count: 0 });
            a.count += 1;
            if a.cover.is_none() || p.cover_of().iter().any(|c| c.eq_ignore_ascii_case(&name)) {
                a.cover = Some((p.ns.clone(), p.id.clone()));
            }
        }
    }
    by.into_values().collect()
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

/// An album: a name photos say they're in.
#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub name: String,
    /// The cover photo: (library, record ID).
    pub cover: Option<(String, String)>,
    pub count: usize,
}

/// A person: a contact (ADR-0024), with their profile photo's record ID.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub ns: String,
    pub id: String,
    pub name: String,
    pub photo: Option<String>,
}

/// Opens a connection to `retifsd` some other way than its socket (on
/// Android: from the retifs app's access service).
#[cfg(unix)]
pub type Connector = Box<dyn Fn() -> Result<std::os::unix::net::UnixStream, String> + Send + Sync>;

/// The connection to `retifsd`.
pub struct Library {
    rt: tokio::runtime::Runtime,
    store: PathBuf,
    #[cfg(unix)]
    connector: Option<Connector>,
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}

impl Library {
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        Library {
            rt,
            store: libretifs::default_store_dir(),
            #[cfg(unix)]
            connector: None,
        }
    }

    /// Connections from `connector` instead of the daemon's socket.
    #[cfg(unix)]
    pub fn with_connector(connector: Connector) -> Self {
        Library { connector: Some(connector), ..Self::new() }
    }

    fn client(&self) -> Result<Client, String> {
        #[cfg(unix)]
        if let Some(connect) = &self.connector {
            let stream = connect()?;
            return self.rt.block_on(async { Client::from_std(stream) }).map_err(|e| e.to_string());
        }
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

    /// Whether this device may add to library `ns`.
    pub fn writable(&self, ns: &str) -> bool {
        let Ok(mut c) = self.client() else { return false };
        match self.call(&mut c, Request::NsInfo { ns: ns.into() }) {
            Ok(Response::Namespace(v)) => v.writers.iter().any(|(w, r)| *w == v.own_writer && r.iter().any(|x| x == "write")),
            _ => false,
        }
    }

    /// Every picture in `libraries`, newest first.
    pub fn photos(&self, libraries: &[String]) -> Result<Vec<Photo>, String> {
        let mut c = self.client()?;
        let mut out: Vec<Photo> = Vec::new();
        for ns in libraries {
            if let Err(e) = self.move_old_albums(&mut c, ns) {
                eprintln!("retifs-photos: {ns}: moving albums: {e}");
            }
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
        self.read(&photo.ns, photo.block(role)?, max)
    }

    /// The bytes of block `cap` of a record in `ns`, if at most `max`.
    pub fn read(&self, ns: &str, cap: &str, max: u64) -> Option<Vec<u8>> {
        let size: u64 = cap.rsplit(':').next()?.parse().ok()?;
        if size > max {
            return None;
        }
        let mut c = self.client().ok()?;
        match self.call(&mut c, Request::ReadRange { cap: cap.into(), ns: Some(ns.into()), offset: 0, len: size, wait_ms: 30_000 }) {
            Ok(Response::Range { data }) => Some(data),
            _ => None,
        }
    }

    /// A picture taken here, into library `ns` (a phone's camera roll, as
    /// Immich backs one up): described at once, as the extractor would
    /// (ADR-0023), with its thumbnails (ADR-0026), so every device shows it
    /// without waiting for one. `path` is where it was (`DCIM/Camera/…`),
    /// `mtime_ms` when it last changed there. Its record ID.
    pub fn import(&self, ns: &str, bytes: Vec<u8>, path: &str, mtime_ms: i64) -> Result<String, String> {
        use retifs_extract::{extract, marker, thumbnails, MARKER};
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let found = extract(&mut std::io::Cursor::new(&bytes), bytes.len() as u64, Some(&name));
        let orientation = found.attrs.iter().find_map(|(k, v)| match (k.as_str(), v) {
            ("exif.orientation", AttrValue::Int(o)) => Some(*o),
            _ => None,
        });
        let small = thumbnails(&bytes, orientation);
        let mut attrs = found.attrs.clone();
        let mime = attrs.iter().find(|(k, _)| k == "mime").and_then(|(_, v)| if let AttrValue::Str(m) = v { Some(m.clone()) } else { None });
        attrs.push(("type".into(), AttrValue::Str(mime.unwrap_or_else(|| "image/jpeg".into()))));
        attrs.push(("path".into(), AttrValue::Str(path.into())));
        attrs.push(("size".into(), AttrValue::Int(bytes.len() as i64)));
        attrs.push(("mtime".into(), AttrValue::Int(mtime_ms)));
        let mut c = self.client()?;
        let mut put = |data: Vec<u8>| match self.call(&mut c, Request::Put { data, public: false, compress: false, publish: false, record: None })? {
            Response::Put { cap } => Ok::<String, String>(cap),
            other => Err(format!("unexpected answer: {other:?}")),
        };
        let mut blocks = vec![put(bytes)?];
        attrs.push((MARKER.into(), AttrValue::Str(marker(&blocks[0]))));
        if let Some((thumb, preview)) = small {
            blocks.push(put(thumb)?);
            blocks.push(put(preview)?);
            attrs.push(("block:thumb".into(), AttrValue::Int(1)));
            attrs.push(("block:preview".into(), AttrValue::Int(2)));
        }
        // The record, then its content by reference (a new record can't name blocks).
        let id = match self.call(&mut c, Request::RecordCreate { ns: ns.into(), attrs, data: None })? {
            Response::Records { records } => records.first().map(|r| r.id.clone()).ok_or("nothing made")?,
            other => return Err(format!("unexpected answer: {other:?}")),
        };
        self.call(&mut c, Request::Transaction { ns: ns.into(), steps: vec![TxStep::Set { id: id.clone(), attrs: vec![], unset: vec![], data: None, blocks: Some(blocks) }] })?;
        Ok(id)
    }

    /// The albums in `libraries`, by name.
    pub fn albums(&self, libraries: &[String]) -> Vec<Album> {
        self.photos(libraries).map(|p| albums_of(&p)).unwrap_or_default()
    }

    /// Albums kept as records (before 2026-11-05) moved onto their photos:
    /// each photo's `album:<id>` keys become names in `album`, an album's
    /// cover the photo's `cover-of`, and the album records go. Only where
    /// this device may write; nothing to do once moved.
    fn move_old_albums(&self, c: &mut Client, ns: &str) -> Result<(), String> {
        let old = self.query(c, ns, format!("type={OLD_ALBUM_TYPE}"));
        if old.is_empty() || !self.writable(ns) {
            return Ok(());
        }
        let text = |r: &RecordView, k: &str| r.attrs.iter().find(|(key, _)| key == k).and_then(|(_, v)| if let AttrValue::Str(s) = v { Some(s.clone()) } else { None });
        let named: HashMap<String, String> = old.iter().map(|r| (r.id.clone(), text(r, "name").unwrap_or_else(|| "Untitled album".into()))).collect();
        let mut covers: HashMap<String, Vec<String>> = HashMap::new();
        for r in &old {
            if let Some(photo) = text(r, "cover") {
                covers.entry(photo).or_default().push(named[&r.id].clone());
            }
        }
        let mut steps = Vec::new();
        for r in self.query(c, ns, "has:type".into()) {
            let keys: Vec<(String, String)> = r.attrs.iter().filter_map(|(k, v)| Some((k.strip_prefix("album:")?.to_string(), if let AttrValue::Str(s) = v { s.clone() } else { String::new() }))).collect();
            let cover = covers.get(&r.id);
            if keys.is_empty() && cover.is_none() {
                continue;
            }
            let p = Photo::from_record(ns, &r);
            let mut albums = p.albums();
            for (id, hint) in &keys {
                let name = named.get(id).cloned().unwrap_or_else(|| hint.clone());
                if !name.trim().is_empty() && !albums.iter().any(|a| a.eq_ignore_ascii_case(&name)) {
                    albums.push(name);
                }
            }
            let mut attrs = Vec::new();
            if let Some(v) = names_value(&albums) {
                attrs.push((ALBUM.to_string(), v));
            }
            if let Some(names) = cover {
                let mut of = p.cover_of();
                of.extend(names.iter().cloned());
                if let Some(v) = names_value(&of) {
                    attrs.push((COVER_OF.to_string(), v));
                }
            }
            steps.push(TxStep::Set { id: r.id.clone(), attrs, unset: keys.iter().map(|(id, _)| format!("album:{id}")).collect(), data: None, blocks: None });
        }
        steps.extend(old.iter().map(|r| TxStep::Delete { id: r.id.clone() }));
        self.call(c, Request::Transaction { ns: ns.into(), steps })?;
        eprintln!("retifs-photos: {ns}: {} album(s) moved onto their photos", old.len());
        Ok(())
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

    /// A photo's names in `key` as its record says now (not as it was read).
    fn names_now(&self, c: &mut Client, ns: &str, id: &str, key: &str) -> Result<Vec<String>, String> {
        match self.call(c, Request::RecordGet { ns: ns.into(), id: id.into() })? {
            Response::Records { records } => Ok(records.first().map(|r| names(r.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v))).unwrap_or_default()),
            other => Err(format!("unexpected answer: {other:?}")),
        }
    }

    /// Changes the names in a photo's `key`: `change` gets them as they are now.
    fn change_names(&self, ns: &str, id: &str, key: &str, change: impl FnOnce(&mut Vec<String>)) -> Result<(), String> {
        let mut c = self.client()?;
        let mut names = self.names_now(&mut c, ns, id, key)?;
        let before = names.clone();
        change(&mut names);
        if names == before {
            return Ok(());
        }
        let (attrs, unset) = match names_value(&names) {
            Some(v) => (vec![(key.to_string(), v)], vec![]),
            None => (vec![], vec![key.to_string()]),
        };
        self.call(&mut c, Request::RecordSet { ns: ns.into(), id: id.into(), attrs, unset, data: None }).map(|_| ())
    }

    /// Puts a photo in album `name` (or takes it out). A new name is a new album.
    pub fn file_in_album(&self, p: &Photo, name: &str, on: bool) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("an album needs a name".into());
        }
        self.change_names(&p.ns, &p.id, ALBUM, |names| {
            names.retain(|n| !n.eq_ignore_ascii_case(name));
            if on {
                names.push(name.to_string());
            }
        })?;
        if !on {
            self.change_names(&p.ns, &p.id, COVER_OF, |names| names.retain(|n| !n.eq_ignore_ascii_case(name)))?;
        }
        Ok(())
    }

    /// Makes `p` album `name`'s cover, in place of the one in `libraries` that was.
    pub fn set_cover(&self, libraries: &[String], name: &str, p: &Photo) -> Result<(), String> {
        for q in self.in_album(libraries, name, COVER_OF)?.iter().filter(|q| (&q.ns, &q.id) != (&p.ns, &p.id)) {
            self.change_names(&q.ns, &q.id, COVER_OF, |names| names.retain(|n| !n.eq_ignore_ascii_case(name)))?;
        }
        self.change_names(&p.ns, &p.id, COVER_OF, |names| {
            if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name.to_string());
            }
        })
    }

    /// Renames album `from` to `to` on every photo in it in `libraries`.
    pub fn rename_album(&self, libraries: &[String], from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("an album needs a name".into());
        }
        for key in [ALBUM, COVER_OF] {
            for p in self.in_album(libraries, from, key)? {
                self.change_names(&p.ns, &p.id, key, |names| {
                    names.retain(|n| !n.eq_ignore_ascii_case(from) && !n.eq_ignore_ascii_case(to));
                    names.push(to.to_string());
                })?;
            }
        }
        Ok(())
    }

    /// Moves a photo into library `to`: a record there with what its record
    /// says now and the same content (by reference: nothing is copied or
    /// encrypted again), and it leaves the library it was in. Its ID there.
    pub fn move_photo(&self, p: &Photo, to: &str) -> Result<String, String> {
        if p.ns == to {
            return Ok(p.id.clone());
        }
        if !self.writable(&p.ns) {
            return Err(format!("{} may not be changed here, so its photos can't leave it", p.ns));
        }
        let mut c = self.client()?;
        let r = match self.call(&mut c, Request::RecordGet { ns: p.ns.clone(), id: p.id.clone() })? {
            Response::Records { records } => records.into_iter().next().ok_or("the photo is gone")?,
            other => return Err(format!("unexpected answer: {other:?}")),
        };
        let id = match self.call(&mut c, Request::RecordCreate { ns: to.into(), attrs: r.attrs, data: None })? {
            Response::Records { records } => records.first().map(|r| r.id.clone()).ok_or("nothing made")?,
            other => return Err(format!("unexpected answer: {other:?}")),
        };
        self.call(&mut c, Request::Transaction { ns: to.into(), steps: vec![TxStep::Set { id: id.clone(), attrs: vec![], unset: vec![], data: None, blocks: Some(r.blocks) }] })?;
        self.call(&mut c, Request::RecordDelete { ns: p.ns.clone(), id: p.id.clone() })?;
        Ok(id)
    }

    /// Moves album `name` into library `to`: every photo of it in
    /// `libraries` that's elsewhere ([`Library::move_photo`]). How many moved.
    pub fn move_album(&self, libraries: &[String], name: &str, to: &str) -> Result<usize, String> {
        if !self.writable(to) {
            return Err(format!("Photos may not add to {to}"));
        }
        let mut moved = 0;
        for p in self.in_album(libraries, name, ALBUM)?.iter().filter(|p| p.ns != to) {
            self.move_photo(p, to)?;
            moved += 1;
        }
        Ok(moved)
    }

    /// The records in `libraries` whose `key` names album `name` (in any case).
    fn in_album(&self, libraries: &[String], name: &str, key: &str) -> Result<Vec<Photo>, String> {
        let mut c = self.client()?;
        let mut out = Vec::new();
        for ns in libraries {
            for r in self.query(&mut c, ns, format!("has:{key}")) {
                let p = Photo::from_record(ns, &r);
                if names(p.attrs.get(key)).iter().any(|n| n.eq_ignore_ascii_case(name)) {
                    out.push(p);
                }
            }
        }
        Ok(out)
    }

    fn create(&self, ns: &str, attrs: Vec<(String, AttrValue)>) -> Result<String, String> {
        let mut c = self.client()?;
        match self.call(&mut c, Request::RecordCreate { ns: ns.into(), attrs, data: None })? {
            Response::Records { records } => records.first().map(|r| r.id.clone()).ok_or_else(|| "nothing made".into()),
            other => Err(format!("unexpected answer: {other:?}")),
        }
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

    /// Waits until one of the libraries in `known` (with their change counts)
    /// changes, for at most `timeout`: whether one did, its count in `known`
    /// brought up to date. retifsd answers the moment a change comes, so a
    /// front end waiting on this (on a thread, with a `Library` of its own)
    /// shows new photos at once instead of asking every few seconds.
    pub fn wait_change(&self, known: &mut HashMap<String, u64>, timeout: std::time::Duration) -> bool {
        let watches: Vec<_> = known.iter().filter_map(|(ns, n)| self.client().ok().map(|c| (c, ns.clone(), *n))).collect();
        if watches.is_empty() {
            std::thread::sleep(timeout.min(std::time::Duration::from_secs(5)));
            return false;
        }
        match self.rt.block_on(libretifs::watch_any(watches, timeout)) {
            Ok(Some((ns, count))) => {
                known.insert(ns, count);
                // Changes come in bursts (a sync merging many): a second
                // more, then every count as it is, so one reload takes them all.
                std::thread::sleep(std::time::Duration::from_secs(1));
                let libraries: Vec<String> = known.keys().cloned().collect();
                known.extend(self.change_counts(&libraries));
                true
            }
            Ok(None) => false,
            // retifsd away (restarting): not at once again.
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_secs(5));
                false
            }
        }
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
        let p = photo(&[("taken", AttrValue::Str("2023-04-09T11:53:13".into())), ("album", AttrValue::List(vec![AttrValue::Str("Prangli".into()), AttrValue::Str("Suvi".into())])), ("person:bb", AttrValue::Str("Ema".into())), ("favorite", AttrValue::Bool(true)), ("exif.fnumber", AttrValue::Str("1.8".into()))], 1);
        assert_eq!(p.month(), "2023-04");
        assert_eq!(p.albums(), ["Prangli", "Suvi"]);
        assert_eq!(p.people(), [("bb".to_string(), "Ema".to_string())]);
        assert!(p.favorite());
        assert!(p.details().iter().any(|(k, v)| k == "Exposure" && v == "f/1.8"));
        let q = photo(&[("mtime", AttrValue::Int(1_759_842_332_000))], 1);
        assert_eq!(q.when(), "2025-10-07T13:05:32");
    }

    #[test]
    fn albums_from_photos() {
        let mut a = photo(&[("album", AttrValue::Str("Prangli".into()))], 1);
        a.id = "a".into();
        let mut b = photo(&[("album", AttrValue::List(vec![AttrValue::Str("prangli".into()), AttrValue::Str("Suvi".into())])), ("cover-of", AttrValue::Str("Prangli".into()))], 1);
        b.id = "b".into();
        let albums = albums_of(&[a, b]);
        assert_eq!(albums.iter().map(|a| (a.name.as_str(), a.count)).collect::<Vec<_>>(), [("Prangli", 2), ("Suvi", 1)], "one name in any case is one album");
        assert_eq!(albums[0].cover, Some(("photos".to_string(), "b".to_string())));
        assert_eq!(names_value(&[]), None);
        assert_eq!(names_value(&["x".into()]), Some(AttrValue::Str("x".into())));
    }
}
