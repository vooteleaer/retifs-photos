package org.retifs.photos

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * The photos core (Rust, `android/jni`), the same as the GTK and Haiku
 * front ends use. Every call blocks: call it off the main thread. Answers
 * are JSON; errors are a string, "" when fine.
 */
object Core {
    init {
        System.loadLibrary("retifs_photos")
    }

    /** `retifs`: the class whose static `connect()` gives connections. */
    external fun init(retifs: Class<*>)
    /** The libraries this app may use: `[{name, writable}]`, or `{"error"}`. */
    external fun libraries(): String
    /** Every picture in `libraries` (a JSON array), newest first, or `{"error"}`. */
    external fun photos(libraries: String): String
    /** Block `cap` of a record in `ns` if at most `max` bytes, else null. */
    external fun read(ns: String, cap: String, max: Long): ByteArray?
    /** Imports the picture open at `fd` (owned by the core from now on): its record ID, or "error: …". */
    external fun importPhoto(ns: String, fd: Int, path: String, mtimeMs: Long): String
    /** `photos`: a JSON array of `{ns, id}`. */
    external fun setFavorite(photos: String, on: Boolean): String
    external fun people(libraries: String): String
    external fun fileInAlbum(photos: String, name: String, on: Boolean): String
    external fun setCover(libraries: String, name: String, photo: String): String
    external fun renameAlbum(libraries: String, from: String, to: String): String
    /** Moves `photos` (`[{ns, id}]`) into library `to`: content by reference, out of where they were. */
    external fun movePhotos(photos: String, to: String): String
    /** Moves every photo of album `name` in `libraries` into library `to`. */
    external fun moveAlbum(libraries: String, name: String, to: String): String
    /** `{library: change count}`: reload when they differ. */
    external fun changeCounts(libraries: String): String
    /** Waits (at most `timeoutMs`) until a library in `known` (`{library: count}`) changes: `{changed, known}`. */
    external fun waitChange(known: String, timeoutMs: Long): String
}

suspend fun <T> core(work: () -> T): T = withContext(Dispatchers.IO) { work() }
