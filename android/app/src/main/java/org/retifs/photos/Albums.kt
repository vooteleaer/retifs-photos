package org.retifs.photos

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.Checkbox
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The albums in the libraries shown, each with its cover and how many it holds. */
@Composable
fun Albums(gallery: Gallery, open: (Album) -> Unit) {
    Box(Modifier.fillMaxSize()) {
        if (gallery.albums.isEmpty() && gallery.reading == null) {
            Text("No albums yet. Select photos (long press) and add them to one with +.", Modifier.padding(24.dp))
        }
        LazyVerticalGrid(columns = GridCells.Adaptive(160.dp), modifier = Modifier.fillMaxSize().padding(8.dp)) {
            items(gallery.albums, key = { it.name.lowercase() }) { a ->
                Column(Modifier.padding(6.dp).clickable { open(a) }) {
                    Box(Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(12.dp))) {
                        if (a.cover != null) Thumb(a.cover, Modifier.fillMaxSize()) else Box(Modifier.fillMaxSize())
                    }
                    Text(a.name, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 6.dp))
                    Text("${a.count}", style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

/** A new album: a name (it's there once a photo is in it). */
@Composable
fun NewAlbum(gallery: Gallery, onDismiss: () -> Unit, made: (Album) -> Unit) {
    var name by remember { mutableStateOf("") }
    val taken = gallery.albums.any { it.name.equals(name.trim(), ignoreCase = true) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("New album") },
        text = {
            Column {
                OutlinedTextField(value = name, onValueChange = { name = it }, label = { Text("Name") }, singleLine = true)
                if (taken) Text("There's an album of that name: the photos go into it.", style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = { Button(enabled = name.isNotBlank(), onClick = { made(Album(name.trim())) }) { Text("Make") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * Which library an album is in: all its photos move into the one chosen
 * (their content by reference; they leave the library they were in).
 */
@Composable
fun AlbumLibrary(gallery: Gallery, album: Album, onDismiss: () -> Unit, chose: (String) -> Unit) {
    val writable = gallery.libraries.filter { it.writable }
    var to by remember { mutableStateOf(album.library) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Library of ${album.name}") },
        text = {
            Column {
                Text(
                    if (album.library != null) "Its ${album.count} photo(s) are in ${album.library}. Choosing another library moves them all there, out of ${album.library}."
                    else "Its photos are in ${album.libraries.sorted().joinToString(", ")}. All of them move into the library chosen.",
                    style = MaterialTheme.typography.bodySmall,
                )
                if (writable.isEmpty()) Text("Photos may add to none of its libraries.", style = MaterialTheme.typography.bodySmall)
                LazyColumn {
                    items(writable, key = { it.name }) { l ->
                        Row(Modifier.fillMaxWidth().clickable { to = l.name }, verticalAlignment = Alignment.CenterVertically) {
                            RadioButton(selected = to == l.name, onClick = { to = l.name })
                            Text(l.name)
                        }
                    }
                }
            }
        },
        confirmButton = { Button(enabled = to != null && (to != album.library), onClick = { chose(to!!) }) { Text("Move") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * Backing up this phone's pictures (Immich's backup screen): on or off,
 * into which library, from which folders, and how far it is.
 */
@Composable
fun BackupScreen(gallery: Gallery) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var allowed by remember { mutableStateOf(Backup.allowed(context)) }
    var enabled by remember { mutableStateOf(Backup.enabled(context)) }
    var library by remember { mutableStateOf(Backup.library(context)) }
    var folders by remember { mutableStateOf<List<Backup.Folder>>(emptyList()) }
    var chosen by remember { mutableStateOf(Backup.folders(context)) }
    var counts by remember { mutableStateOf<Pair<Int, Int>?>(null) }
    var removing by remember { mutableStateOf(Backup.removing(context)) }
    var mayRemove by remember { mutableStateOf(Backup.mayRemove(context)) }
    var onPhone by remember { mutableStateOf(0) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { allowed = Backup.allowed(context) }
    val manage = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        mayRemove = Backup.mayRemove(context)
        Backup.removals++
    }
    LaunchedEffect(allowed, chosen, Backup.backedUp, Backup.running, Backup.removals) {
        if (!allowed) return@LaunchedEffect
        withContext(Dispatchers.IO) {
            folders = Backup.allFolders(context)
            val (all, todo) = Backup.pending(context)
            counts = all to (all - todo.size)
            onPhone = Backup.inLibrary(context).size
        }
    }
    val writable = gallery.libraries.filter { it.writable }
    LazyColumn(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            Card {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("Back up this phone's photos", style = MaterialTheme.typography.titleMedium)
                            Text("New pictures go into the library chosen, encrypted like everything in it; your other devices see them as it syncs.", style = MaterialTheme.typography.bodySmall)
                        }
                        Switch(checked = enabled, enabled = allowed && library != null, onCheckedChange = {
                            enabled = it
                            Backup.set(context, enabled = it)
                            if (it) scope.launch(Dispatchers.IO) { Backup.run(context) }
                        })
                    }
                    if (!allowed) {
                        Text("Photos needs to see this phone's pictures to back them up.", style = MaterialTheme.typography.bodySmall)
                        Button(onClick = { ask.launch(Backup.permissions()) }) { Text("Allow") }
                        OutlinedButton(onClick = { context.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))) }) { Text("App settings") }
                    }
                    counts?.let { (all, done) ->
                        if (all > 0) LinearProgressIndicator(progress = { done.toFloat() / all }, modifier = Modifier.fillMaxWidth())
                        Text("$done of $all backed up", style = MaterialTheme.typography.bodyMedium)
                    }
                    if (Backup.note.isNotEmpty()) Text(Backup.note, style = MaterialTheme.typography.bodySmall)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(enabled = allowed && library != null && !Backup.running, onClick = { scope.launch(Dispatchers.IO) { Backup.run(context) } }) { Text("Back up now") }
                        if (Backup.running) OutlinedButton(onClick = { Backup.cancel() }) { Text("Stop") }
                    }
                }
            }
        }
        if (Build.VERSION.SDK_INT >= 31) item {
            Card {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("Remove from this phone once backed up", style = MaterialTheme.typography.titleMedium)
                            Text("The photos are then in the library only. Those removed go to the phone's trash, kept there for 30 days.", style = MaterialTheme.typography.bodySmall)
                        }
                        Switch(checked = removing, enabled = allowed, onCheckedChange = {
                            removing = it
                            Backup.set(context, remove = it)
                            Backup.removals++
                        })
                    }
                    if (removing && !mayRemove) {
                        Text("Photos needs media management to remove pictures without asking about each.", style = MaterialTheme.typography.bodySmall)
                        Button(onClick = { manage.launch(Intent(Settings.ACTION_REQUEST_MANAGE_MEDIA, Uri.parse("package:${context.packageName}"))) }) { Text("Allow") }
                    }
                    if (removing && onPhone > 0) Text("$onPhone backed up still on this phone", style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        item { Text("Into library", style = MaterialTheme.typography.titleSmall) }
        if (writable.isEmpty()) item { Text("Photos may add to none of its libraries. Choose libraries… (☰) and let it change one.", style = MaterialTheme.typography.bodySmall) }
        items(writable, key = { "lib:" + it.name }) { l ->
            Row(Modifier.fillMaxWidth().clickable { library = l.name; Backup.set(context, library = l.name) }, verticalAlignment = Alignment.CenterVertically) {
                RadioButton(selected = library == l.name, onClick = { library = l.name; Backup.set(context, library = l.name) })
                Text(l.name)
            }
        }
        if (allowed) {
            item { Text("From folders", style = MaterialTheme.typography.titleSmall) }
            items(folders, key = { "f:" + it.id }) { f ->
                val on = chosen?.contains(f.id) ?: f.camera
                val flip = {
                    val now = (chosen ?: folders.filter { it.camera }.map { it.id }.toSet()).toMutableSet()
                    if (on) now.remove(f.id) else now.add(f.id)
                    chosen = now
                    Backup.set(context, folders = now)
                }
                Row(Modifier.fillMaxWidth().clickable { flip() }, verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(checked = on, onCheckedChange = { flip() })
                    Column {
                        Text(f.name)
                        Text("${f.count} picture(s)" + if (f.camera) " · camera" else "", style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        }
    }
}
