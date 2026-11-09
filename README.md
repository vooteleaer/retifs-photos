# retifs Photos

Photos on [retifs](https://github.com/vooteleaer/retifs), in the spirit of Immich: every picture in your libraries, wherever they came from (a phone, a mount, Explorer, an app), encrypted and synced like everything else.

> A demo app of [retifs](https://github.com/vooteleaer/retifs), part of its test release. To try it, start with [getting started](https://github.com/vooteleaer/retifs/blob/main/docs/getting-started.md).

- **Timeline:** every photo by month, newest first, as square thumbnails. The thumbnails are blocks the extractor keeps with each photo (ADR-0026), so a device shows the grid without fetching originals.
- **Favourites** (`favorite = true`, a star in every retifs app), **albums** (a name on its photos: `album` is a name or a list of them, so a photo is in any number of albums, in whichever libraries; an album can be moved to another library with all its photos; file views show them under `Albums/`), **people**: your contacts (ADR-0024), with their profile photos, and the photos they're tagged in (`person:<contact id>`).
- **Viewer:** the preview, what's known of the photo (when, size, camera, exposure, place, file), favourite, people (tag a contact, or a new one: they go into your address book), albums (in or out, a new one with this photo as its cover).
- Updates as the libraries change, here or on another device. Contacts and Calendar see the same records.

Faces come later from their own daemon, as proposals the user confirms (ADR-0026).

`core/` talks to `retifsd` through libretifs; `gtk/` is the Linux front end (GTK 4, libadwaita). Build on Linux (or WSL): `cargo build -p retifs-photos-gtk`. For demos: `RETIFS_PHOTOS_VIEW=favorites|album:<name>|person:<name>`, `RETIFS_PHOTOS_OPEN=<n>` (open the nth photo), `RETIFS_PHOTOS_SNAPSHOT=<file.png>`.

## Android

`android/` is the phone app (Kotlin, Compose, the same core through JNI in `android/jni`), laid out like Immich's: the timeline by day with a handle to scroll by month, long press to select (favourite, add to album, share), a full-screen viewer (swipe, pinch or double tap to zoom; the original is read once zoomed in; Info), albums, and **Backup**: the pictures taken on the phone (the camera's folders, or others chosen) go into a library you pick, each described and given its thumbnails on the phone (as the extractor would, ADR-0023/0026), so every device shows it at once. New pictures are backed up when they appear (a job the system starts), after a restart, and every few hours to catch up. If you choose, what's backed up then leaves the phone (to its trash, kept 30 days; with media management allowed once, without asking each time), and the library is where the photos are.

retifsd's sockets are in the retifs app's files, so the app asks the retifs app (`org.retifs.desktop`) for each connection: the user lets Photos in once there, to the libraries chosen, and retifs hands over connections made with its own app token for it (retifs ADR-0004). Libraries are joined in retifs; the drawer's Announces opens it.

Build: `powershell -ExecutionPolicy Bypass -File android\build-apk.ps1 [-Abis arm64-v8a]` (NDK 27, JDK 17, retifs next to this repository).

## License

[MIT](LICENSE)
