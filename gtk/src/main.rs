//! retifs Photos for Linux: native GTK 4 / libadwaita over the shared core.
//! A timeline of every picture by month, favourites, albums and people (the
//! user's contacts); a viewer with what's known of a photo, where it can be
//! made a favourite, put in albums, and have people tagged, from contacts
//! or as a new one. Grids show the thumbnails the extractor keeps with each
//! photo (ADR-0026), so originals stay where they are.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use retifs_photos_core::{Album, Library, Person, Photo};

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id("org.retifs.Photos").build();
    app.connect_activate(build);
    app.run()
}

fn bg<T: Send + 'static>(lib: &Arc<Library>, work: impl FnOnce(&Library) -> T + Send + 'static, then: impl FnOnce(T) + 'static) {
    let lib = lib.clone();
    glib::spawn_future_local(async move {
        if let Ok(r) = gio::spawn_blocking(move || work(&lib)).await {
            then(r);
        }
    });
}

/// What the grid shows.
#[derive(Clone, PartialEq)]
enum View {
    All,
    Favorites,
    Album(Album),
    Person(Person),
}

const CELL: i32 = 150;
/// Thumbnails (or small originals) larger than this aren't fetched for grids.
const MAX_THUMB: u64 = 4 << 20;

struct App {
    lib: Arc<Library>,
    window: adw::ApplicationWindow,
    nav: adw::NavigationView,
    grid_page: adw::NavigationPage,
    grid_box: gtk::Box,
    side: gtk::ListBox,
    side_rows: RefCell<Vec<View>>,
    view: RefCell<View>,
    libraries: RefCell<Vec<String>>,
    photos: RefCell<Vec<Photo>>,
    albums: RefCell<Vec<Album>>,
    people: RefCell<Vec<Person>>,
    /// Thumbnails as they are (the viewer, until the preview comes).
    textures: RefCell<HashMap<String, gdk::Texture>>,
    /// Thumbnails cut square to the grid's size.
    squares: RefCell<HashMap<String, gdk::Texture>>,
    pictures: RefCell<HashMap<String, gtk::Picture>>,
    counts: RefCell<HashMap<String, u64>>,
    status: gtk::Label,
}

fn build(gapp: &adw::Application) {
    let lib = Arc::new(Library::new());

    let side = gtk::ListBox::new();
    side.add_css_class("navigation-sidebar");
    let status = gtk::Label::builder().xalign(0.0).margin_start(12).margin_bottom(8).css_classes(["dim-label", "caption"]).build();
    let side_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    side_box.append(&gtk::ScrolledWindow::builder().child(&side).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build());
    side_box.append(&status);
    let new_album = gtk::Button::from_icon_name("list-add-symbolic");
    new_album.set_tooltip_text(Some("New album"));
    let side_header = adw::HeaderBar::new();
    side_header.pack_start(&new_album);
    let side_view = adw::ToolbarView::new();
    side_view.add_top_bar(&side_header);
    side_view.set_content(Some(&side_box));
    let side_page = adw::NavigationPage::builder().title("Photos").child(&side_view).build();

    let grid_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).margin_start(12).margin_end(12).margin_bottom(12).build();
    let grid_view = adw::ToolbarView::new();
    grid_view.add_top_bar(&adw::HeaderBar::new());
    grid_view.set_content(Some(&gtk::ScrolledWindow::builder().child(&grid_box).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build()));
    let grid_page = adw::NavigationPage::builder().title("Photos").tag("grid").child(&grid_view).build();
    let nav = adw::NavigationView::new();
    nav.add(&grid_page);
    let content_page = adw::NavigationPage::builder().title("Photos").child(&nav).build();

    let split = adw::NavigationSplitView::builder().sidebar(&side_page).content(&content_page).build();
    let window = adw::ApplicationWindow::builder().application(gapp).title("Photos").default_width(1100).default_height(720).content(&split).build();

    let me = Rc::new(App {
        lib,
        window: window.clone(),
        nav,
        grid_page,
        grid_box,
        side: side.clone(),
        side_rows: RefCell::new(Vec::new()),
        view: RefCell::new(View::All),
        libraries: RefCell::new(Vec::new()),
        photos: RefCell::new(Vec::new()),
        albums: RefCell::new(Vec::new()),
        people: RefCell::new(Vec::new()),
        textures: RefCell::new(HashMap::new()),
        squares: RefCell::new(HashMap::new()),
        pictures: RefCell::new(HashMap::new()),
        counts: RefCell::new(HashMap::new()),
        status,
    });
    {
        let me = me.clone();
        side.connect_row_activated(move |_, row| {
            let v = me.side_rows.borrow().get(row.index() as usize).cloned();
            if let Some(v) = v {
                *me.view.borrow_mut() = v;
                me.nav.pop_to_tag("grid");
                show_grid(&me);
            }
        });
    }
    {
        let me = me.clone();
        new_album.connect_clicked(move |_| ask_new_album(&me, None));
    }
    reload(&me, true);
    {
        let me = me.clone();
        glib::timeout_add_seconds_local(3, move || {
            let libs = me.libraries.borrow().clone();
            let me2 = me.clone();
            bg(&me.lib, move |l| l.change_counts(&libs), move |counts| {
                if !counts.is_empty() && counts != *me2.counts.borrow() {
                    reload(&me2, false);
                }
            });
            glib::ControlFlow::Continue
        });
    }
    snapshot_on_request(&window);
    window.present();
}

/// Everything again: libraries, photos, albums, people.
fn reload(me: &Rc<App>, first: bool) {
    let me2 = me.clone();
    bg(&me.lib, |l| {
        let (libs, photos) = match l.libraries() {
            Ok(libs) => {
                let photos = l.photos(&libs);
                (libs, photos)
            }
            // No daemon: said, not shown as an empty library.
            Err(e) => (Vec::new(), Err(e)),
        };
        let albums = l.albums(&libs);
        let people = l.people(&libs);
        let counts = l.change_counts(&libs);
        (libs, photos, albums, people, counts)
    }, move |(libs, photos, albums, people, counts)| {
        *me2.counts.borrow_mut() = counts;
        *me2.libraries.borrow_mut() = libs;
        match photos {
            Ok(p) => *me2.photos.borrow_mut() = p,
            Err(e) => {
                me2.photos.borrow_mut().clear();
                me2.status.set_text(&format!("retifsd isn't answering: {e}"));
            }
        }
        *me2.albums.borrow_mut() = albums;
        *me2.people.borrow_mut() = people;
        show_sidebar(&me2);
        show_grid(&me2);
        if first {
            open_on_start(&me2);
        }
    });
}

fn side_label(text: &str, header: bool) -> gtk::Widget {
    let l = gtk::Label::builder().label(text).xalign(0.0).build();
    if header {
        l.add_css_class("dim-label");
        l.add_css_class("caption-heading");
        l.set_margin_top(12);
    }
    l.upcast()
}

fn show_sidebar(me: &Rc<App>) {
    me.side.remove_all();
    let mut rows = Vec::new();
    let photos = me.photos.borrow();
    let mut add = |me: &Rc<App>, widget: gtk::Widget, v: Option<View>| {
        let row = gtk::ListBoxRow::builder().child(&widget).build();
        if v.is_none() {
            row.set_activatable(false);
            row.set_selectable(false);
        }
        me.side.append(&row);
        rows.push(v.unwrap_or(View::All));
        if rows.last() == Some(&*me.view.borrow()) && v_is(&rows, &me.view.borrow()) {
            me.side.select_row(Some(&row));
        }
    };
    add(me, line("image-x-generic-symbolic", &format!("Photos  ·  {}", photos.len()), None), Some(View::All));
    add(me, line("starred-symbolic", &format!("Favorites  ·  {}", photos.iter().filter(|p| p.favorite()).count()), None), Some(View::Favorites));
    add(me, side_label("Albums", true), None);
    for a in me.albums.borrow().iter() {
        let n = photos.iter().filter(|p| p.albums().iter().any(|(id, _)| *id == a.id)).count();
        add(me, line("folder-pictures-symbolic", &format!("{}  ·  {n}", a.name), None), Some(View::Album(a.clone())));
    }
    add(me, side_label("People", true), None);
    for p in me.people.borrow().iter() {
        let n = photos.iter().filter(|ph| ph.people().iter().any(|(id, _)| *id == p.id)).count();
        if n == 0 && p.photo.is_none() {
            continue; // people without photos stay in Contacts
        }
        let avatar = adw::Avatar::new(24, Some(&p.name), true);
        if let Some(t) = p.photo.as_ref().and_then(|id| me.textures.borrow().get(id).cloned()) {
            avatar.set_custom_image(Some(&t));
        }
        add(me, line_with(avatar.upcast(), &format!("{}  ·  {n}", p.name)), Some(View::Person(p.clone())));
    }
    drop(add);
    *me.side_rows.borrow_mut() = rows;
}

fn v_is(rows: &[View], v: &View) -> bool {
    rows.iter().filter(|r| *r == v).count() == 1
}

fn line(icon: &str, text: &str, _sub: Option<&str>) -> gtk::Widget {
    line_with(gtk::Image::from_icon_name(icon).upcast(), text)
}

fn line_with(prefix: gtk::Widget, text: &str) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.append(&prefix);
    b.append(&gtk::Label::builder().label(text).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build());
    b.upcast()
}

/// The photos the current view shows, newest first.
fn shown(me: &Rc<App>) -> Vec<Photo> {
    let view = me.view.borrow().clone();
    me.photos
        .borrow()
        .iter()
        .filter(|p| match &view {
            View::All => true,
            View::Favorites => p.favorite(),
            View::Album(a) => p.albums().iter().any(|(id, _)| *id == a.id),
            View::Person(person) => p.people().iter().any(|(id, _)| *id == person.id),
        })
        .cloned()
        .collect()
}

fn month_name(ym: &str) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    match (ym.get(..4), ym.get(5..7).and_then(|m| m.parse::<usize>().ok())) {
        (Some(y), Some(m)) if (1..=12).contains(&m) => format!("{} {y}", MONTHS[m - 1]),
        _ => "No date".into(),
    }
}

fn show_grid(me: &Rc<App>) {
    while let Some(c) = me.grid_box.first_child() {
        me.grid_box.remove(&c);
    }
    me.pictures.borrow_mut().clear();
    let photos = shown(me);
    let title = match &*me.view.borrow() {
        View::All => "Photos".to_string(),
        View::Favorites => "Favorites".into(),
        View::Album(a) => a.name.clone(),
        View::Person(p) => p.name.clone(),
    };
    me.grid_page.set_title(&title);
    if !me.status.text().starts_with("retifsd") {
        me.status.set_text(&format!("{} photo(s)", photos.len()));
    }
    if photos.is_empty() {
        let empty = adw::StatusPage::builder().icon_name("image-x-generic-symbolic").title("No photos here").description("Photos in your libraries show up here, with their thumbnails once the extractor has made them.").vexpand(true).build();
        me.grid_box.append(&empty);
        return;
    }
    let mut month = String::new();
    let mut flow: Option<gtk::FlowBox> = None;
    for (i, p) in photos.iter().enumerate() {
        if p.month() != month || flow.is_none() {
            month = p.month();
            let head = gtk::Label::builder().label(month_name(&month)).xalign(0.0).css_classes(["title-4"]).margin_top(12).build();
            me.grid_box.append(&head);
            let f = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).homogeneous(false).min_children_per_line(2).max_children_per_line(40).column_spacing(4).row_spacing(4).halign(gtk::Align::Start).build();
            me.grid_box.append(&f);
            flow = Some(f);
        }
        let pic = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).width_request(CELL).height_request(CELL).build();
        pic.add_css_class("card");
        if let Some(t) = me.squares.borrow().get(&p.id) {
            pic.set_paintable(Some(t));
        }
        // A fixed square, however wide the window.
        let frame = gtk::Overlay::builder().child(&pic).width_request(CELL).height_request(CELL).halign(gtk::Align::Start).valign(gtk::Align::Start).hexpand(false).build();
        if p.favorite() {
            let star = gtk::Image::builder().icon_name("starred-symbolic").halign(gtk::Align::End).valign(gtk::Align::Start).margin_top(4).margin_end(4).build();
            frame.add_overlay(&star);
        }
        let click = gtk::GestureClick::new();
        let me2 = me.clone();
        click.connect_released(move |_, _, _, _| open_viewer(&me2, i));
        frame.add_controller(click);
        frame.set_tooltip_text(Some(&p.title()));
        flow.as_ref().unwrap().append(&frame);
        me.pictures.borrow_mut().insert(p.id.clone(), pic);
    }
    load_thumbs(me, photos);
}

/// Fetches the thumbnails the grid lacks, a batch at a time.
fn load_thumbs(me: &Rc<App>, photos: Vec<Photo>) {
    let missing: Vec<Photo> = photos.into_iter().filter(|p| !me.squares.borrow().contains_key(&p.id)).collect();
    if missing.is_empty() {
        return;
    }
    let (batch, rest): (Vec<Photo>, Vec<Photo>) = {
        let n = missing.len().min(24);
        let mut m = missing;
        let rest = m.split_off(n);
        (m, rest)
    };
    let me2 = me.clone();
    bg(&me.lib, move |l| batch.iter().map(|p| (p.id.clone(), l.image(p, "thumb", MAX_THUMB))).collect::<Vec<_>>(), move |got| {
        for (id, bytes) in got {
            let Some(bytes) = bytes else { continue };
            let Some(t) = gdk::Texture::from_bytes(&glib::Bytes::from(&bytes)).ok() else { continue };
            let sq = square(&bytes).unwrap_or_else(|| t.clone());
            if let Some(pic) = me2.pictures.borrow().get(&id) {
                pic.set_paintable(Some(&sq));
            }
            me2.textures.borrow_mut().insert(id.clone(), t);
            me2.squares.borrow_mut().insert(id, sq);
        }
        // People's avatars may be among them.
        if me2.people.borrow().iter().any(|p| p.photo.as_ref().is_some_and(|id| me2.textures.borrow().contains_key(id))) {
            show_sidebar(&me2);
        }
        load_thumbs(&me2, rest);
    });
}

/// A thumbnail cut to a centred square and scaled to the grid's cell, so
/// the cells have its size. (Sharper on HiDPI screens: by the scale, later.)
fn square(bytes: &[u8]) -> Option<gdk::Texture> {
    let pb = gtk::gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(bytes.to_vec())).ok()?;
    let side = pb.width().min(pb.height());
    let cut = pb.new_subpixbuf((pb.width() - side) / 2, (pb.height() - side) / 2, side, side);
    let px = CELL;
    let small = cut.scale_simple(px, px, gtk::gdk_pixbuf::InterpType::Bilinear)?;
    #[allow(deprecated)]
    Some(gdk::Texture::for_pixbuf(&small))
}

/// The viewer for the `i`th photo of the current view.
fn open_viewer(me: &Rc<App>, i: usize) {
    let photos = shown(me);
    let Some(p) = photos.get(i).cloned() else { return };
    let pic = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).hexpand(true).vexpand(true).build();
    if let Some(t) = me.textures.borrow().get(&p.id) {
        pic.set_paintable(Some(t));
    }
    {
        let (pic, p2) = (pic.clone(), p.clone());
        bg(&me.lib, move |l| l.image(&p2, "preview", 32 << 20), move |bytes| {
            if let Some(t) = bytes.and_then(|b| gdk::Texture::from_bytes(&glib::Bytes::from_owned(b)).ok()) {
                pic.set_paintable(Some(&t));
            }
        });
    }
    let info = adw::PreferencesPage::new();
    info.set_width_request(340);

    let about = adw::PreferencesGroup::builder().title(glib::markup_escape_text(&p.title())).build();
    let fav = adw::SwitchRow::builder().title("Favorite").active(p.favorite()).build();
    {
        let (me2, p2) = (me.clone(), p.clone());
        fav.connect_active_notify(move |s| {
            let (on, p3, me3) = (s.is_active(), p2.clone(), me2.clone());
            bg(&me2.lib, move |l| l.set_favorite(&p3, on), move |_| reload(&me3, false));
        });
    }
    about.add(&fav);
    for (k, v) in p.details() {
        about.add(&adw::ActionRow::builder().title(k).subtitle(glib::markup_escape_text(&v)).subtitle_selectable(true).css_classes(["property"]).build());
    }
    info.add(&about);

    // People: tagged contacts, and tagging another (or a new contact).
    let people = adw::PreferencesGroup::builder().title("People").description("Your contacts: their cards in Contacts list these photos.").build();
    let tagged = p.people();
    for (id, hint) in &tagged {
        let name = me.people.borrow().iter().find(|x| x.id == *id).map(|x| x.name.clone()).unwrap_or_else(|| hint.clone());
        let row = adw::ActionRow::builder().title(glib::markup_escape_text(&name)).build();
        row.add_prefix(&adw::Avatar::new(28, Some(&name), true));
        let remove = gtk::Button::builder().icon_name("list-remove-symbolic").valign(gtk::Align::Center).css_classes(["flat"]).tooltip_text("Untag").build();
        let person = Person { ns: String::new(), id: id.clone(), name: name.clone(), photo: None };
        let (me2, p2) = (me.clone(), p.clone());
        remove.connect_clicked(move |_| {
            let (me3, p3, person, id) = (me2.clone(), p2.clone(), person.clone(), p2.id.clone());
            bg(&me2.lib, move |l| l.tag_person(&p3, &person, false), move |_| reopen(&me3, &id));
        });
        row.add_suffix(&remove);
        people.add(&row);
    }
    let candidates: Vec<Person> = me.people.borrow().iter().filter(|x| !tagged.iter().any(|(id, _)| *id == x.id)).cloned().collect();
    let mut names: Vec<String> = vec!["Tag a person…".into()];
    names.extend(candidates.iter().map(|x| x.name.clone()));
    names.push("New contact…".into());
    let pick = adw::ComboRow::builder().title("Add").model(&gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>())).build();
    {
        let (me2, p2, candidates) = (me.clone(), p.clone(), candidates.clone());
        pick.connect_selected_notify(move |c| {
            let i = c.selected() as usize;
            if i == 0 {
                return;
            }
            if let Some(person) = candidates.get(i - 1).cloned() {
                let (me3, p3, id) = (me2.clone(), p2.clone(), p2.id.clone());
                bg(&me2.lib, move |l| l.tag_person(&p3, &person, true), move |_| reopen(&me3, &id));
            } else {
                ask_new_person(&me2, &p2);
            }
        });
    }
    people.add(&pick);
    info.add(&people);

    // Albums: in or out of each, a new one, this photo as a cover.
    let albums = adw::PreferencesGroup::builder().title("Albums").build();
    let in_albums = p.albums();
    for a in me.albums.borrow().iter() {
        let on = in_albums.iter().any(|(id, _)| *id == a.id);
        let row = adw::SwitchRow::builder().title(glib::markup_escape_text(&a.name)).active(on).build();
        let (me2, p2, a2) = (me.clone(), p.clone(), a.clone());
        row.connect_active_notify(move |s| {
            let (on, me3, p3, a3) = (s.is_active(), me2.clone(), p2.clone(), a2.clone());
            bg(&me2.lib, move |l| l.file_in_album(&p3, &a3, on), move |_| reload(&me3, false));
        });
        albums.add(&row);
    }
    let add_album = gtk::Button::builder().label("New album with this photo").css_classes(["flat"]).halign(gtk::Align::Start).build();
    {
        let (me2, p2) = (me.clone(), p.clone());
        add_album.connect_clicked(move |_| ask_new_album(&me2, Some(p2.clone())));
    }
    albums.add(&add_album);
    info.add(&albums);

    let pane = gtk::Paned::builder().orientation(gtk::Orientation::Horizontal).start_child(&pic).end_child(&info).resize_end_child(false).shrink_end_child(false).build();
    let header = adw::HeaderBar::new();
    let prev = gtk::Button::from_icon_name("go-previous-symbolic");
    let next = gtk::Button::from_icon_name("go-next-symbolic");
    prev.set_sensitive(i > 0);
    next.set_sensitive(i + 1 < photos.len());
    header.pack_end(&next);
    header.pack_end(&prev);
    {
        let me2 = me.clone();
        prev.connect_clicked(move |_| {
            me2.nav.pop();
            open_viewer(&me2, i - 1);
        });
        let me2 = me.clone();
        next.connect_clicked(move |_| {
            me2.nav.pop();
            open_viewer(&me2, i + 1);
        });
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&pane));
    let page = adw::NavigationPage::builder().title(glib::markup_escape_text(&p.title())).tag(format!("photo:{}", p.id)).child(&view).build();
    me.nav.push(&page);
}

/// The viewer of photo `id` again, after a change, once reloaded.
fn reopen(me: &Rc<App>, id: &str) {
    let (me2, id) = (me.clone(), id.to_string());
    bg(&me.lib, |l| {
        let libs = l.libraries().unwrap_or_default();
        (l.photos(&libs), l.albums(&libs), l.people(&libs))
    }, move |(photos, albums, people)| {
        if let Ok(p) = photos {
            *me2.photos.borrow_mut() = p;
        }
        *me2.albums.borrow_mut() = albums;
        *me2.people.borrow_mut() = people;
        show_sidebar(&me2);
        show_grid(&me2);
        if let Some(i) = shown(&me2).iter().position(|p| p.id == id) {
            me2.nav.pop_to_tag("grid");
            open_viewer(&me2, i);
        }
    });
}

fn ask_text(me: &Rc<App>, heading: &str, body: &str, then: impl Fn(String) + 'static) {
    let entry = gtk::Entry::builder().activates_default(true).build();
    let dialog = adw::MessageDialog::builder().heading(heading).body(body).extra_child(&entry).default_response("ok").close_response("cancel").transient_for(&me.window).build();
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", "Create")]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.connect_response(None, move |_, r| {
        let text = entry.text().trim().to_string();
        if r == "ok" && !text.is_empty() {
            then(text);
        }
    });
    dialog.present();
}

/// A new album, in the library of `with` (the photo it starts with) or the first other than `me`.
fn ask_new_album(me: &Rc<App>, with: Option<Photo>) {
    let ns = with.as_ref().map(|p| p.ns.clone()).or_else(|| me.libraries.borrow().iter().find(|l| *l != "me").cloned()).unwrap_or_else(|| "me".into());
    let me2 = me.clone();
    ask_text(me, "New album", &format!("Albums are records in a library ({ns}): share one and its photos go with it."), move |name| {
        let (me3, with, with_id) = (me2.clone(), with.clone(), with.as_ref().map(|p| p.id.clone()));
        let ns = ns.clone();
        bg(&me2.lib, move |l| {
            let album = l.new_album(&ns, &name)?;
            if let Some(p) = &with {
                l.file_in_album(p, &album, true)?;
                l.set_cover(&album, p)?;
            }
            Ok::<_, String>(())
        }, move |r| {
            if let Err(e) = r {
                me3.status.set_text(&e);
            }
            match &with_id {
                Some(id) => reopen(&me3, id),
                None => reload(&me3, false),
            }
        });
    });
}

/// Tagging someone not in contacts yet: a new contact in `me`.
fn ask_new_person(me: &Rc<App>, p: &Photo) {
    let (me2, p2) = (me.clone(), p.clone());
    ask_text(me, "New contact", "They're added to your address book (me) and tagged here.", move |name| {
        let (me3, p3, id) = (me2.clone(), p2.clone(), p2.id.clone());
        bg(&me2.lib, move |l| {
            let person = l.new_person(&name)?;
            l.tag_person(&p3, &person, true)
        }, move |_| reopen(&me3, &id));
    });
}

/// RETIFS_PHOTOS_VIEW=favorites|album:<name>|person:<name>, RETIFS_PHOTOS_OPEN=<n>: on start (demos).
fn open_on_start(me: &Rc<App>) {
    if let Ok(v) = std::env::var("RETIFS_PHOTOS_VIEW") {
        let view = if v == "favorites" {
            Some(View::Favorites)
        } else if let Some(name) = v.strip_prefix("album:") {
            me.albums.borrow().iter().find(|a| a.name == name).cloned().map(View::Album)
        } else if let Some(name) = v.strip_prefix("person:") {
            me.people.borrow().iter().find(|p| p.name == name).cloned().map(View::Person)
        } else {
            None
        };
        if let Some(view) = view {
            *me.view.borrow_mut() = view;
            show_sidebar(me);
            show_grid(me);
        }
    }
    if let Some(i) = std::env::var("RETIFS_PHOTOS_OPEN").ok().and_then(|s| s.parse().ok()) {
        let me2 = me.clone();
        glib::timeout_add_seconds_local_once(3, move || open_viewer(&me2, i));
    }
}

/// RETIFS_PHOTOS_SNAPSHOT=<file.png>: the window drawn to a file every 2 s.
fn snapshot_on_request(window: &adw::ApplicationWindow) {
    let Ok(path) = std::env::var("RETIFS_PHOTOS_SNAPSHOT") else { return };
    let window = window.clone();
    glib::timeout_add_seconds_local(2, move || {
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
        if let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) {
            let _ = renderer.render_texture(&node, None).save_to_png(&path);
        }
        glib::ControlFlow::Continue
    });
}
