package org.retifs.photos

import android.content.Context
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.DateRange
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.List
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            val dark = isSystemInDarkTheme()
            val colors = when {
                Build.VERSION.SDK_INT >= 31 && dark -> dynamicDarkColorScheme(this)
                Build.VERSION.SDK_INT >= 31 -> dynamicLightColorScheme(this)
                dark -> darkColorScheme()
                else -> lightColorScheme()
            }
            MaterialTheme(colorScheme = colors) {
                Surface(Modifier.fillMaxSize()) { App() }
            }
        }
    }
}

/** A library this app may use. */
data class Lib(val name: String, val writable: Boolean)

/**
 * What the app shows: the libraries it may use (each shown or not), their
 * pictures and albums, read library by library so each shows as soon as
 * it's read.
 */
class Gallery(val context: Context) {
    var libraries by mutableStateOf<List<Lib>>(emptyList())
    val hidden = mutableStateListOf<String>().apply { addAll(prefs().getStringSet("hidden", emptySet())!!) }
    var photos by mutableStateOf<List<Photo>>(emptyList())
    val albums: List<Album> by derivedStateOf { Album.of(photos) }
    /** "Reading libraries…": which, how many of how many; null when done. */
    var reading by mutableStateOf<String?>(null)
    var progress by mutableStateOf(0f)
    var error by mutableStateOf("")
    private var counts = ""

    private fun prefs() = context.getSharedPreferences("gallery", Context.MODE_PRIVATE)

    val shown: List<String> get() = libraries.map { it.name }.filter { it !in hidden }

    fun toggle(name: String) {
        if (name in hidden) hidden.remove(name) else hidden.add(name)
        prefs().edit().putStringSet("hidden", hidden.toSet()).apply()
    }

    suspend fun load() {
        val answer = core { Core.libraries() }
        if (answer.trimStart().startsWith("{")) {
            error = JSONObject(answer).optString("error")
            return
        }
        error = ""
        val a = JSONArray(answer)
        libraries = (0 until a.length()).map { a.getJSONObject(it) }.map { Lib(it.getString("name"), it.optBoolean("writable")) }
        Backup.choose(context, libraries.filter { it.writable }.map { it.name })
        Backup.start(context)
        val names = shown
        val all = ArrayList<Photo>()
        names.forEachIndexed { i, ns ->
            reading = "Reading libraries… $ns (${i + 1} of ${names.size})"
            progress = i.toFloat() / names.size
            Photo.list(core { Core.photos(JSONArray(listOf(ns)).toString()) }).onSuccess { list ->
                all.addAll(list)
                all.sortWith(compareByDescending<Photo> { it.whenTaken }.thenBy { it.id })
                photos = all.toList()
            }.onFailure { error = it.message ?: "" }
        }
        if (names.isEmpty()) photos = emptyList()
        counts = core { Core.changeCounts(JSONArray(names).toString()) }
        reading = null
    }

    /**
     * What to wait on (`{library: count}`): the libraries shown, with their
     * counts in `known` (the last wait's), a newly shown one's from its read.
     */
    fun watching(known: JSONObject): String {
        val read = JSONObject(counts.ifEmpty { "{}" })
        val w = JSONObject()
        for (ns in shown) {
            if (known.has(ns)) w.put(ns, known.getLong(ns)) else if (read.has(ns)) w.put(ns, read.getLong(ns))
        }
        return w.toString()
    }

    /** Changes a photo here at once, as the core was asked to. */
    fun update(changed: List<Photo>) {
        val byId = changed.associateBy { it.ns to it.id }
        photos = photos.map { byId[it.ns to it.id] ?: it }
    }
}

enum class Tab { Photos, Albums, Backup }

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun App() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val gallery = remember { Gallery(context) }
    var tab by remember { mutableStateOf(Tab.Photos) }
    var viewing by remember { mutableStateOf<Pair<List<Photo>, Int>?>(null) }
    var album by remember { mutableStateOf<Album?>(null) }
    val drawer = rememberDrawerState(DrawerValue.Closed)
    var reloads by remember { mutableStateOf(0) }
    val letIn = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { reloads++ }
    // Backing up is on from the start: ask once to see the phone's pictures.
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { Backup.start(context) }
    LaunchedEffect(Unit) {
        val prefs = context.getSharedPreferences("backup", Context.MODE_PRIVATE)
        if (!Backup.allowed(context) && !prefs.getBoolean("asked", false)) {
            prefs.edit().putBoolean("asked", true).apply()
            ask.launch(Backup.permissions())
        }
    }

    LaunchedEffect(reloads, Backup.backedUp, gallery.hidden.size) { gallery.load() }
    // What's backed up leaves the phone (if the user wants), without asking: media management.
    val remove = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) {
        // The next ones, if there were more than one request takes.
        if (it.resultCode == android.app.Activity.RESULT_OK) Backup.removals++
    }
    LaunchedEffect(Backup.backedUp, Backup.removals) {
        withContext(Dispatchers.IO) { Backup.removeRequest(context) }?.let { remove.launch(IntentSenderRequest.Builder(it).build()) }
    }
    // Changes from elsewhere (another device, the backup): read again the
    // moment retifsd says a shown library changed, while the app is on screen.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(Unit) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            var known = JSONObject()
            while (true) {
                val answer = JSONObject(core { Core.waitChange(gallery.watching(known), 60_000) })
                known = answer.optJSONObject("known") ?: JSONObject()
                if (answer.optBoolean("changed") && Retifs.problem == null) {
                    // A read under way may have passed the change already: after it, again.
                    snapshotFlow { gallery.reading }.first { it == null }
                    gallery.load()
                }
            }
        }
    }

    viewing?.let { (list, start) ->
        BackHandler { viewing = null }
        Viewer(gallery, list, start, close = { viewing = null })
        return
    }

    // The album as the photos say now (its count and libraries change as they move).
    val current = album?.let { a -> gallery.albums.find { it.name.equals(a.name, true) } ?: a }
    var moving by remember { mutableStateOf(false) }
    if (moving && current != null) AlbumLibrary(gallery, current, onDismiss = { moving = false }) { to ->
        moving = false
        scope.launch {
            val e = core { Core.moveAlbum(JSONArray(gallery.libraries.map { it.name }).toString(), current.name, to) }
            if (to in gallery.hidden) gallery.toggle(to)
            gallery.load()
            if (e.isNotEmpty()) gallery.error = e
        }
    }

    ModalNavigationDrawer(drawerState = drawer, drawerContent = {
        ModalDrawerSheet { Drawer(gallery, letIn = { letIn.launch(Retifs.letIn()) }) }
    }) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(album?.name ?: tab.name) },
                    navigationIcon = {
                        if (album != null) {
                            IconButton(onClick = { album = null }) { Icon(Icons.Filled.Close, "Close album") }
                        } else {
                            IconButton(onClick = { scope.launch { drawer.open() } }) { Icon(Icons.Filled.Menu, "Libraries") }
                        }
                    },
                    actions = {
                        current?.let { a -> TextButton(onClick = { moving = true }) { Text(a.library ?: "Libraries…") } }
                        IconButton(onClick = { reloads++ }) { Icon(Icons.Filled.Refresh, "Read again") }
                    },
                )
            },
            bottomBar = {
                NavigationBar {
                    NavigationBarItem(selected = tab == Tab.Photos, onClick = { tab = Tab.Photos; album = null }, icon = { Icon(Icons.Filled.Home, null) }, label = { Text("Photos") })
                    NavigationBarItem(selected = tab == Tab.Albums, onClick = { tab = Tab.Albums }, icon = { Icon(Icons.Filled.List, null) }, label = { Text("Albums") })
                    NavigationBarItem(selected = tab == Tab.Backup, onClick = { tab = Tab.Backup; album = null }, icon = { Icon(Icons.Filled.Refresh, null) }, label = { Text("Backup") })
                }
            },
        ) { pad ->
            Column(Modifier.padding(pad).fillMaxSize()) {
                gallery.reading?.let {
                    LinearProgressIndicator(progress = { gallery.progress }, modifier = Modifier.fillMaxWidth())
                    Text(it, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 16.dp, vertical = 2.dp))
                }
                if (Retifs.problem != null) {
                    Problem(Retifs.problem!!, letIn = { letIn.launch(Retifs.letIn()) }, retry = { reloads++ })
                    return@Column
                }
                if (gallery.error.isNotEmpty()) Text(gallery.error, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(16.dp))
                when {
                    album != null -> {
                        val a = album!!
                        val list = gallery.photos.filter { it.inAlbum(a.name) }
                        Timeline(gallery, list, open = { i -> viewing = list to i })
                    }
                    tab == Tab.Photos -> Timeline(gallery, gallery.photos, open = { i -> viewing = gallery.photos to i })
                    tab == Tab.Albums -> Albums(gallery, open = { album = it })
                    tab == Tab.Backup -> BackupScreen(gallery)
                }
            }
        }
    }
}

/** retifs can't be used yet: why, and what to do. */
@Composable
fun Problem(p: Retifs.Problem, letIn: () -> Unit, retry: () -> Unit) {
    val context = LocalContext.current
    Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        when (p) {
            Retifs.Problem.NOT_INSTALLED -> {
                Text("retifs isn't installed", style = MaterialTheme.typography.titleMedium)
                Text("Photos shows the pictures in your retifs libraries. Install the retifs app (from the retifs-installers library) first.")
            }
            Retifs.Problem.NOT_LET_IN -> {
                Text("Let Photos use retifs", style = MaterialTheme.typography.titleMedium)
                Text("retifs asks which of your libraries Photos may see, and whether it may add to them (to back up this phone's pictures).")
                Button(onClick = letIn) { Text("Choose libraries…") }
            }
            Retifs.Problem.NOT_RUNNING -> {
                Text("retifs isn't running", style = MaterialTheme.typography.titleMedium)
                Text("Open retifs: it starts once Reticulum (Prns) is up.")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Retifs.open(context)?.let { i -> OutlinedButton(onClick = { context.startActivity(i) }) { Text("Open retifs") } }
                    Button(onClick = retry) { Text("Try again") }
                }
            }
        }
    }
}

/** The libraries: Announces above (in the retifs app), each library a toggle. */
@Composable
fun Drawer(gallery: Gallery, letIn: () -> Unit) {
    val context = LocalContext.current
    LazyColumn(Modifier.padding(vertical = 8.dp)) {
        item {
            OutlinedButton(onClick = { Retifs.open(context)?.let { context.startActivity(it) } }, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp)) { Text("Announces") }
            Text("Libraries", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 4.dp))
        }
        items(gallery.libraries, key = { it.name }) { l ->
            val count = gallery.photos.count { it.ns == l.name }
            Row(Modifier.fillMaxWidth().clickable { gallery.toggle(l.name) }.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Checkbox(checked = l.name !in gallery.hidden, onCheckedChange = { gallery.toggle(l.name) })
                Column {
                    Text(l.name)
                    val access = if (l.writable) "read and write" else "read only"
                    Text(if (l.name in gallery.hidden) access else "$access · $count photo(s)", style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        item {
            HorizontalDivider(Modifier.padding(vertical = 8.dp))
            Text("Photos sees only the libraries retifs let it in to.", style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 16.dp))
            Box(Modifier.padding(16.dp)) { OutlinedButton(onClick = letIn) { Text("Choose libraries…") } }
        }
    }
}
