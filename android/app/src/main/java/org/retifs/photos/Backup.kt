package org.retifs.photos

import android.Manifest
import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.ContentUris
import android.content.Context
import android.content.Intent
import android.content.IntentSender
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.io.File

/**
 * Backing up the pictures taken on this phone into a library, as Immich
 * does: the folders chosen (the camera's, unless the user picks others),
 * each picture once, described and given its thumbnails here so every
 * device shows it at once. On unless the user turns it off: runs when
 * the app opens, whenever new pictures appear (a job the system starts),
 * every few hours to catch up, and again soon after retifs couldn't be
 * reached. If the user wants, what's backed up then leaves the phone (its
 * trash) while the app is open, and the library is where the photos are.
 */
object Backup {
    private const val TAG = "retifs-photos-backup"
    private const val JOB = 1
    private const val JOB_CATCH_UP = 2
    private const val JOB_RETRY = 3
    private const val MAX_REMOVE = 500

    /** A folder of pictures on the phone (a MediaStore bucket). */
    data class Folder(val id: String, val name: String, val count: Int, val camera: Boolean)

    /** A picture on the phone. */
    data class Item(val id: Long, val uri: Uri, val path: String, val modifiedMs: Long, val folder: String)

    var running by mutableStateOf(false)
        private set
    /** What it's doing ("3 of 120"), or the last outcome. */
    var note by mutableStateOf("")
        private set
    /** Bumped when something was backed up (the timeline reloads). */
    var backedUp by mutableStateOf(0)
        private set
    /** Bumped when removing was turned on or allowed (it's tried again). */
    var removals by mutableStateOf(0)

    @Volatile private var stop = false

    private fun prefs(context: Context) = context.getSharedPreferences("backup", Context.MODE_PRIVATE)

    fun enabled(context: Context) = prefs(context).getBoolean("enabled", true)
    /** Whether pictures backed up leave the phone (to its trash). */
    fun removing(context: Context) = prefs(context).getBoolean("remove", false)
    /** Whether they may leave without asking each time (media management, Android 12+). */
    fun mayRemove(context: Context) = Build.VERSION.SDK_INT >= 31 && MediaStore.canManageMedia(context)
    fun library(context: Context): String? = prefs(context).getString("library", null)
    /** The folders chosen; null: the camera's. */
    fun folders(context: Context): Set<String>? = prefs(context).getStringSet("folders", null)

    fun set(context: Context, enabled: Boolean? = null, library: String? = null, folders: Set<String>? = null, remove: Boolean? = null) {
        prefs(context).edit().apply {
            enabled?.let { putBoolean("enabled", it) }
            remove?.let { putBoolean("remove", it) }
            library?.let { putString("library", it) }
            folders?.let { putStringSet("folders", it) }
        }.apply()
        schedule(context)
    }

    /**
     * The library to back up into when none is chosen (or the one chosen
     * may no longer be added to): the first Photos may add to.
     */
    fun choose(context: Context, writable: List<String>) {
        val now = library(context)
        if (writable.isEmpty() || (now != null && now in writable)) return
        set(context, library = writable.first())
    }

    /** Backs up off the main thread, if it's on. */
    fun start(context: Context) {
        if (enabled(context) && library(context) != null && allowed(context) && !running) Thread { run(context) }.start()
    }

    fun allowed(context: Context): Boolean {
        val read = if (Build.VERSION.SDK_INT >= 33) Manifest.permission.READ_MEDIA_IMAGES else Manifest.permission.READ_EXTERNAL_STORAGE
        return context.checkSelfPermission(read) == PackageManager.PERMISSION_GRANTED
    }

    fun permissions(): Array<String> = buildList {
        add(if (Build.VERSION.SDK_INT >= 33) Manifest.permission.READ_MEDIA_IMAGES else Manifest.permission.READ_EXTERNAL_STORAGE)
        if (Build.VERSION.SDK_INT >= 29) add(Manifest.permission.ACCESS_MEDIA_LOCATION)
    }.toTypedArray()

    /** What was backed up: phone picture ID → "modified recordId". */
    private fun doneFile(context: Context) = File(context.filesDir, "backed-up")

    private fun done(context: Context): MutableMap<Long, String> {
        val out = HashMap<Long, String>()
        runCatching {
            doneFile(context).forEachLine { line ->
                val parts = line.split('\t')
                if (parts.size >= 2) parts[0].toLongOrNull()?.let { out[it] = parts[1] }
            }
        }
        return out
    }

    private fun remember(context: Context, item: Item, record: String) {
        doneFile(context).appendText("${item.id}\t${item.modifiedMs}\t$record\n")
    }

    /** Every folder with pictures, the camera's first. */
    fun allFolders(context: Context): List<Folder> {
        val counts = LinkedHashMap<String, Pair<String, Int>>()
        val camera = HashSet<String>()
        scan(context) { _, folder, name, path ->
            val c = counts[folder]
            counts[folder] = name to ((c?.second ?: 0) + 1)
            if (path.startsWith("DCIM/") || name.equals("Camera", true)) camera += folder
        }
        return counts.map { (id, v) -> Folder(id, v.first, v.second, id in camera) }.sortedWith(compareBy({ !it.camera }, { it.name.lowercase() }))
    }

    private fun chosen(context: Context): Set<String> = folders(context) ?: allFolders(context).filter { it.camera }.map { it.id }.toSet()

    /** The pictures in the chosen folders: (all, not backed up yet), oldest first. */
    fun pending(context: Context): Pair<Int, List<Item>> {
        val folders = chosen(context)
        val done = done(context)
        val all = ArrayList<Item>()
        scan(context) { item, folder, _, _ -> if (folder in folders) all += item }
        all.sortBy { it.modifiedMs }
        return all.size to all.filter { it.id !in done }
    }

    /**
     * The pictures in the chosen folders that are in the library as they
     * are now (backed up and not changed since): those that may leave.
     */
    fun inLibrary(context: Context): List<Item> {
        val folders = chosen(context)
        val done = done(context)
        val out = ArrayList<Item>()
        scan(context) { item, folder, _, _ -> if (folder in folders && done[item.id] == item.modifiedMs.toString()) out += item }
        return out
    }

    /**
     * Asks the system to move what's backed up to the phone's trash (kept
     * there for 30 days), at most [MAX_REMOVE] at a time; null when there's
     * nothing to, or it would ask the user (removing without asking needs
     * media management).
     */
    fun removeRequest(context: Context): IntentSender? {
        if (Build.VERSION.SDK_INT < 30 || !removing(context) || !mayRemove(context)) return null
        val uris = inLibrary(context).take(MAX_REMOVE).map { it.uri }
        if (uris.isEmpty()) return null
        return MediaStore.createTrashRequest(context.contentResolver, uris, true).intentSender
    }

    private fun scan(context: Context, each: (Item, String, String, String) -> Unit) {
        if (!allowed(context)) return
        val uri = MediaStore.Images.Media.EXTERNAL_CONTENT_URI
        val pathCol = if (Build.VERSION.SDK_INT >= 29) MediaStore.Images.Media.RELATIVE_PATH else MediaStore.Images.Media.DATA
        val cols = arrayOf(MediaStore.Images.Media._ID, MediaStore.Images.Media.BUCKET_ID, MediaStore.Images.Media.BUCKET_DISPLAY_NAME, pathCol, MediaStore.Images.Media.DISPLAY_NAME, MediaStore.Images.Media.DATE_MODIFIED)
        context.contentResolver.query(uri, cols, null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val id = c.getLong(0)
                val folder = c.getString(1) ?: continue
                val folderName = c.getString(2) ?: ""
                val name = c.getString(4) ?: "$id.jpg"
                val raw = c.getString(3) ?: ""
                // Relative to the phone's storage: `DCIM/Camera/IMG_1.jpg`.
                val path = if (Build.VERSION.SDK_INT >= 29) raw + name else raw.substringAfter("/0/", raw.trimStart('/'))
                each(Item(id, ContentUris.withAppendedId(uri, id), path, c.getLong(5) * 1000, folder), folder, folderName, path)
            }
        }
    }

    /** Backs up what's new now (once at a time); whether to try again soon. */
    fun run(context: Context): Boolean {
        synchronized(this) {
            if (running) return false
            running = true
            stop = false
        }
        try {
            val ns = library(context)
            if (ns == null) {
                note = "Choose a library to back up into"
                return false
            }
            val (all, todo) = pending(context)
            var n = 0
            var failed = 0
            for (item in todo) {
                if (stop) break
                note = "Backing up ${n + 1} of ${todo.size}"
                val fd = try {
                    val uri = if (Build.VERSION.SDK_INT >= 29 && context.checkSelfPermission(Manifest.permission.ACCESS_MEDIA_LOCATION) == PackageManager.PERMISSION_GRANTED) MediaStore.setRequireOriginal(item.uri) else item.uri
                    context.contentResolver.openFileDescriptor(uri, "r")?.detachFd()
                } catch (e: Exception) {
                    Log.w(TAG, "${item.path}: ${e.message}")
                    null
                }
                if (fd == null) {
                    failed++
                    continue
                }
                val r = Core.importPhoto(ns, fd, item.path, item.modifiedMs)
                if (r.startsWith("error: ")) {
                    Log.w(TAG, "${item.path}: $r")
                    failed++
                    // retifs went away: the rest later.
                    if (Retifs.problem != null) break
                    continue
                }
                remember(context, item, r)
                n++
                if (n % 5 == 0) backedUp++
            }
            if (n > 0) backedUp++
            val left = todo.size - n
            note = when {
                Retifs.problem != null -> "Waiting for retifs"
                left == 0 -> "All $all backed up"
                failed > 0 -> "$left not backed up ($failed failed)"
                else -> "$left left"
            }
            return !stop && Retifs.problem != null
        } finally {
            running = false
        }
    }

    fun cancel() {
        stop = true
    }

    /**
     * The jobs that back up (or drop them when off): one when new pictures
     * appear, which the system forgets on a restart (so [BootReceiver]
     * schedules it again), and one every few hours, which it keeps.
     */
    fun schedule(context: Context) {
        val js = context.getSystemService(JobScheduler::class.java)
        if (!enabled(context) || library(context) == null) {
            js.cancel(JOB)
            js.cancel(JOB_CATCH_UP)
            js.cancel(JOB_RETRY)
            return
        }
        val job = ComponentName(context, BackupJob::class.java)
        js.schedule(JobInfo.Builder(JOB, job)
            .addTriggerContentUri(JobInfo.TriggerContentUri(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, JobInfo.TriggerContentUri.FLAG_NOTIFY_FOR_DESCENDANTS))
            .setTriggerContentUpdateDelay(10_000)
            .setTriggerContentMaxDelay(60_000)
            .setRequiresBatteryNotLow(true)
            .build())
        if (js.getPendingJob(JOB_CATCH_UP) == null) {
            js.schedule(JobInfo.Builder(JOB_CATCH_UP, job)
                .setPeriodic(6 * 3600_000L)
                .setRequiresBatteryNotLow(true)
                .setPersisted(true)
                .build())
        }
    }

    /** Tries again in a few minutes (retifs wasn't there). */
    fun retry(context: Context) {
        if (!enabled(context)) return
        context.getSystemService(JobScheduler::class.java).schedule(JobInfo.Builder(JOB_RETRY, ComponentName(context, BackupJob::class.java))
            .setMinimumLatency(2 * 60_000L)
            .setRequiresBatteryNotLow(true)
            .build())
    }
}

/** Started by the system (pictures added, catching up, trying again); schedules itself again. */
class BackupJob : JobService() {
    override fun onStartJob(params: JobParameters?): Boolean {
        Thread {
            var again = false
            try {
                again = Backup.run(applicationContext)
            } finally {
                jobFinished(params, false)
                Backup.schedule(applicationContext)
                if (again) Backup.retry(applicationContext)
            }
        }.start()
        return true
    }

    override fun onStopJob(params: JobParameters?): Boolean {
        Backup.cancel()
        return true
    }
}

/** After a restart or an update: the jobs the system forgot. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        Backup.schedule(context)
    }
}
