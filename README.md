# retifs Photos

Photos on [retifs](../retifs), in the spirit of Immich: every picture in your libraries, wherever they came from (a phone, a mount, Explorer, an app), encrypted and synced like everything else.

- **Timeline:** every photo by month, newest first, as square thumbnails. The thumbnails are blocks the extractor keeps with each photo (ADR-0026), so a device shows the grid without fetching originals.
- **Favourites** (`favorite = true`), **albums** (records photos refer to as `album:<album id>`: a photo is in several, renaming touches no photo, sharing an album is a share of its query), **people**: your contacts (ADR-0024), with their profile photos, and the photos they're tagged in (`person:<contact id>`).
- **Viewer:** the preview, what's known of the photo (when, size, camera, exposure, place, file), favourite, people (tag a contact, or a new one: they go into your address book), albums (in or out, a new one with this photo as its cover).
- Updates as the libraries change, here or on another device. Contacts and Calendar see the same records.

Faces come later from their own daemon, as proposals the user confirms (ADR-0026).

`core/` talks to `retifsd` through libretifs; `gtk/` is the Linux front end (GTK 4, libadwaita). Build on Linux (or WSL): `cargo build -p retifs-photos-gtk`. For demos: `RETIFS_PHOTOS_VIEW=favorites|album:<name>|person:<name>`, `RETIFS_PHOTOS_OPEN=<n>` (open the nth photo), `RETIFS_PHOTOS_SNAPSHOT=<file.png>`.
