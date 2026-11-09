package org.retifs.photos

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import android.util.LruCache
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/** A picture in a library, as the core describes it. */
data class Photo(
    val ns: String,
    val id: String,
    val whenTaken: String,
    val title: String,
    val favorite: Boolean,
    val thumb: String?,
    val preview: String?,
    val original: String?,
    val type: String,
    val details: List<Pair<String, String>>,
    val geo: Pair<Double, Double>?,
    /** The albums it's in, by name. */
    val albums: List<String>,
    /** The albums it's the cover of. */
    val coverOf: List<String> = emptyList(),
) {
    fun inAlbum(name: String) = albums.any { it.equals(name, ignoreCase = true) }

    /** `YYYY-MM-DD`, or "" when nothing says. */
    val day: String get() = whenTaken.take(10)

    /** Where it is, as the core takes it to change it. */
    fun ref(): JSONObject = JSONObject().put("ns", ns).put("id", id)

    companion object {
        fun of(j: JSONObject): Photo {
            val details = j.optJSONArray("details") ?: JSONArray()
            val geo = j.optJSONArray("geo")
            val albums = j.optJSONArray("albums") ?: JSONArray()
            val coverOf = j.optJSONArray("cover_of") ?: JSONArray()
            return Photo(
                ns = j.getString("ns"),
                id = j.getString("id"),
                whenTaken = j.optString("when"),
                title = j.optString("title"),
                favorite = j.optBoolean("favorite"),
                thumb = j.optString("thumb").ifEmpty { null }.takeUnless { j.isNull("thumb") },
                preview = j.optString("preview").ifEmpty { null }.takeUnless { j.isNull("preview") },
                original = j.optString("original").ifEmpty { null }.takeUnless { j.isNull("original") },
                type = j.optString("type").takeUnless { j.isNull("type") } ?: "",
                details = (0 until details.length()).map { details.getJSONArray(it).let { d -> d.getString(0) to d.getString(1) } },
                geo = if (geo != null && geo.length() == 2) geo.getDouble(0) to geo.getDouble(1) else null,
                albums = (0 until albums.length()).map { albums.getString(it) },
                coverOf = (0 until coverOf.length()).map { coverOf.getString(it) },
            )
        }

        fun list(answer: String): Result<List<Photo>> = runCatching {
            if (answer.trimStart().startsWith("{")) error(JSONObject(answer).optString("error"))
            val a = JSONArray(answer)
            (0 until a.length()).map { of(a.getJSONObject(it)) }
        }

        /** The size a read-cap names (its last field). */
        fun size(cap: String): Long = cap.substringAfterLast(':').toLongOrNull() ?: Long.MAX_VALUE
    }
}

/**
 * An album: a name photos say they're in (one name in any case, in any
 * library, is one album). There while a photo is in it.
 */
data class Album(val name: String, val cover: Photo? = null, val count: Int = 0, val libraries: Set<String> = emptySet()) {
    /** The library it's in, if all its photos are in one. */
    val library: String? get() = libraries.singleOrNull()

    companion object {
        /** The albums `photos` are in: each with its cover (the one that says so, else the newest) and count. */
        fun of(photos: List<Photo>): List<Album> {
            val by = LinkedHashMap<String, Album>()
            for (p in photos) {
                for (name in p.albums) {
                    val key = name.lowercase()
                    val a = by[key] ?: Album(name)
                    val cover = if (a.cover == null || p.coverOf.any { it.equals(name, true) }) p else a.cover
                    by[key] = a.copy(cover = cover, count = a.count + 1, libraries = a.libraries + p.ns)
                }
            }
            return by.values.sortedBy { it.name.lowercase() }
        }
    }
}

/**
 * Pictures as bitmaps: read through the core, decoded no bigger than shown,
 * kept in memory while there's room (a grid scrolled back is there at once).
 */
object Images {
    /** The most the viewer reads of an original (bigger ones show their preview). */
    const val MAX_ORIGINAL = 40L shl 20

    private val cache = object : LruCache<String, Bitmap>((Runtime.getRuntime().maxMemory() / 1024 / 6).toInt()) {
        override fun sizeOf(key: String, value: Bitmap): Int = value.byteCount / 1024
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private val reads = Dispatchers.IO.limitedParallelism(4)

    fun cached(cap: String, side: Int): Bitmap? = cache.get("$cap@$side")

    /** Block `cap` of a record in `ns`, decoded to about `side` pixels on its longest side. */
    suspend fun load(ns: String, cap: String, side: Int, max: Long = MAX_ORIGINAL): Bitmap? {
        val key = "$cap@$side"
        cache.get(key)?.let { return it }
        val bytes = withContext(reads) { Core.read(ns, cap, max) } ?: return null
        val bitmap = withContext(Dispatchers.Default) { decode(bytes, side) } ?: return null
        cache.put(key, bitmap)
        return bitmap
    }

    /** The smallest picture of `p` that fills `side` pixels: thumb, preview, else the original. */
    suspend fun best(p: Photo, side: Int): Bitmap? {
        val caps = listOfNotNull(p.thumb.takeIf { side <= 400 }, p.preview.takeIf { side <= 1800 }, p.original, p.preview, p.thumb)
        for (cap in caps.distinct()) load(p.ns, cap, side)?.let { return it }
        return null
    }

    /** Decoded at most about `side` on its longest side, turned as its EXIF says. */
    fun decode(bytes: ByteArray, side: Int): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outWidth <= 0) return null
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= side) sample *= 2
        val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample }) ?: return null
        val orientation = runCatching { ExifInterface(bytes.inputStream()).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL) }.getOrDefault(ExifInterface.ORIENTATION_NORMAL)
        val m = Matrix()
        when (orientation) {
            ExifInterface.ORIENTATION_ROTATE_90 -> m.postRotate(90f)
            ExifInterface.ORIENTATION_ROTATE_180 -> m.postRotate(180f)
            ExifInterface.ORIENTATION_ROTATE_270 -> m.postRotate(270f)
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> m.postScale(-1f, 1f)
            ExifInterface.ORIENTATION_FLIP_VERTICAL -> m.postScale(1f, -1f)
            ExifInterface.ORIENTATION_TRANSPOSE -> { m.postRotate(90f); m.postScale(-1f, 1f) }
            ExifInterface.ORIENTATION_TRANSVERSE -> { m.postRotate(270f); m.postScale(-1f, 1f) }
            else -> return bitmap
        }
        return Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, m, true)
    }
}
