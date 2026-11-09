package org.retifs.photos

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import kotlinx.coroutines.launch
import org.json.JSONArray
import java.io.File
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import java.util.Locale

/** A row of the timeline: a day's heading, or a picture. */
private sealed interface Cell {
    data class Day(val day: String) : Cell
    data class Pic(val index: Int) : Cell
}

private val dayFormat = DateTimeFormatter.ofPattern("EEE, d MMM yyyy", Locale.getDefault())
private val monthFormat = DateTimeFormatter.ofPattern("MMMM yyyy", Locale.getDefault())

fun dayLabel(day: String): String = runCatching { LocalDate.parse(day).format(dayFormat) }.getOrDefault(day.ifEmpty { "No date" })
fun monthLabel(day: String): String = runCatching { LocalDate.parse(day).format(monthFormat) }.getOrDefault(day.take(7))

/**
 * Every picture by the day it was taken, newest first (Immich's timeline):
 * four to a row, a heading per day, a handle on the right to scroll by
 * month. Tap opens one; a long press selects, for marking favourite,
 * filing in an album or sharing.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun Timeline(gallery: Gallery, photos: List<Photo>, open: (Int) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val cells = remember(photos) {
        val out = ArrayList<Cell>()
        var day: String? = null
        photos.forEachIndexed { i, p ->
            if (p.day != day) {
                day = p.day
                out += Cell.Day(p.day)
            }
            out += Cell.Pic(i)
        }
        out
    }
    val selected = remember { mutableStateListOf<Int>() }
    var filing by remember { mutableStateOf(false) }
    val grid = rememberLazyGridState()
    if (photos.isEmpty() && gallery.reading == null) {
        Box(Modifier.fillMaxSize().padding(24.dp)) {
            Text("No photos in the libraries shown. Back up this phone's pictures (Backup), or show other libraries (☰).", style = MaterialTheme.typography.bodyMedium)
        }
        return
    }
    Column {
        AnimatedVisibility(selected.isNotEmpty()) {
            Surface(tonalElevation = 3.dp) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = { selected.clear() }) { Icon(Icons.Filled.Close, "Clear") }
                    Text("${selected.size} selected", Modifier.weight(1f))
                    val chosen = { selected.map { photos[it] } }
                    val allFavorite = selected.all { photos.getOrNull(it)?.favorite == true }
                    IconButton(onClick = {
                        val list = chosen()
                        scope.launch {
                            val err = core { Core.setFavorite(JSONArray(list.map { it.ref() }).toString(), !allFavorite) }
                            if (err.isEmpty()) gallery.update(list.map { it.copy(favorite = !allFavorite) }) else gallery.error = err
                        }
                    }) { Icon(Icons.Filled.Star, "Favourite", tint = if (allFavorite) MaterialTheme.colorScheme.primary else LocalContentColor.current.copy(alpha = 0.4f)) }
                    IconButton(onClick = { filing = true }) { Icon(Icons.Filled.Add, "Add to albums") }
                    IconButton(onClick = { val list = chosen(); scope.launch { share(context, list) } }) { Icon(Icons.Filled.Share, "Share") }
                }
            }
        }
        Box(Modifier.fillMaxSize()) {
            LazyVerticalGrid(columns = GridCells.Fixed(4), state = grid, modifier = Modifier.fillMaxSize()) {
                items(cells.size, key = { i -> cells[i].let { c -> if (c is Cell.Pic) photos[c.index].let { "${it.ns}/${it.id}" } else "d:${(c as Cell.Day).day}:$i" } }, span = { i -> GridItemSpan(if (cells[i] is Cell.Day) maxLineSpan else 1) }) { i ->
                    when (val c = cells[i]) {
                        is Cell.Day -> Text(dayLabel(c.day), style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 12.dp, top = 16.dp, bottom = 6.dp))
                        is Cell.Pic -> {
                            val p = photos[c.index]
                            val isSelected = c.index in selected
                            Box(
                                Modifier.aspectRatio(1f).padding(1.dp).combinedClickable(
                                    onClick = {
                                        if (selected.isEmpty()) open(c.index) else if (isSelected) selected.remove(c.index) else selected.add(c.index)
                                    },
                                    onLongClick = { if (isSelected) selected.remove(c.index) else selected.add(c.index) },
                                ),
                            ) {
                                Thumb(p, Modifier.fillMaxSize().padding(if (isSelected) 8.dp else 0.dp).clip(RoundedCornerShape(if (isSelected) 8.dp else 0.dp)))
                                if (p.favorite) Icon(Icons.Filled.Star, null, tint = Color.White, modifier = Modifier.align(Alignment.BottomStart).padding(4.dp).size(14.dp))
                                if (isSelected) Icon(Icons.Filled.CheckCircle, null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.align(Alignment.TopStart).padding(2.dp).size(20.dp))
                            }
                        }
                    }
                }
            }
            Scrubber(grid, cells.size, label = { i ->
                var j = i.coerceIn(0, cells.size - 1)
                while (j > 0 && cells[j] !is Cell.Pic) j--
                (cells.getOrNull(j) as? Cell.Pic)?.let { monthLabel(photos[it.index].day) } ?: ""
            })
        }
    }
    if (filing) AlbumPicker(gallery, onDismiss = { filing = false }) { albums ->
        filing = false
        val list = selected.map { photos[it] }
        scope.launch {
            val refs = JSONArray(list.map { it.ref() }).toString()
            val errs = albums.map { a -> core { Core.fileInAlbum(refs, a.name, true) } }.filter { it.isNotEmpty() }.toMutableList()
            // An album in one library takes its new photos there (the first such, if several).
            val to = albums.firstNotNullOfOrNull { a -> gallery.albums.find { it.name.equals(a.name, true) }?.library }
            val away = if (errs.isEmpty() && to != null) list.filter { it.ns != to } else emptyList()
            if (away.isNotEmpty()) core { Core.movePhotos(JSONArray(away.map { it.ref() }).toString(), to!!) }.let { if (it.isNotEmpty()) errs += it }
            if (errs.isEmpty()) {
                gallery.update(list.map { p -> p.copy(albums = p.albums + albums.map { it.name }.filter { !p.inAlbum(it) }) })
                selected.clear()
                if (away.isNotEmpty()) gallery.load()
            } else {
                gallery.error = errs.first()
            }
        }
    }
}

/** A picture's thumbnail, read when shown. */
@Composable
fun Thumb(p: Photo, modifier: Modifier) {
    val side = 320
    val image by produceState<Bitmap?>(p.thumb?.let { Images.cached(it, side) }, p.ns, p.id) {
        if (value == null) value = Images.best(p, side)
    }
    Box(modifier.background(MaterialTheme.colorScheme.surfaceVariant)) {
        image?.let { Image(it.asImageBitmap(), p.title, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize()) }
    }
}

/** A handle on the right edge: dragged, the grid follows, and the month shows. */
@Composable
private fun Scrubber(grid: LazyGridState, count: Int, label: (Int) -> String) {
    if (count < 60) return
    val scope = rememberCoroutineScope()
    var dragging by remember { mutableStateOf(false) }
    var at by remember { mutableStateOf(0f) }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val height = constraints.maxHeight.toFloat()
        val handle = 48.dp
        val handlePx = with(LocalDensity.current) { handle.toPx() }
        val shown = if (dragging) at else grid.firstVisibleItemIndex.toFloat() / count.coerceAtLeast(1)
        val y = (shown * (height - handlePx)).coerceIn(0f, height - handlePx)
        Box(
            Modifier.align(Alignment.TopEnd).fillMaxHeight().width(32.dp).pointerDrag(
                start = { offset -> dragging = true; at = (offset / height).coerceIn(0f, 1f); scope.launch { grid.scrollToItem((at * (count - 1)).toInt()) } },
                drag = { offset -> at = (offset / height).coerceIn(0f, 1f); scope.launch { grid.scrollToItem((at * (count - 1)).toInt()) } },
                end = { dragging = false },
            ),
        ) {
            Box(Modifier.offset { androidx.compose.ui.unit.IntOffset(0, y.toInt()) }.align(Alignment.TopEnd).padding(end = 2.dp).size(width = 6.dp, height = handle).clip(RoundedCornerShape(3.dp)).background(MaterialTheme.colorScheme.primary.copy(alpha = if (dragging) 1f else 0.5f)))
        }
        if (dragging) {
            Surface(shape = RoundedCornerShape(16.dp), tonalElevation = 6.dp, modifier = Modifier.align(Alignment.TopEnd).offset { androidx.compose.ui.unit.IntOffset(0, y.toInt()) }.padding(end = 40.dp)) {
                Text(label((at * (count - 1)).toInt()), Modifier.padding(horizontal = 12.dp, vertical = 6.dp), style = MaterialTheme.typography.labelLarge)
            }
        }
    }
}

/** Vertical drags on this: where they start and go (y in pixels), and their end. */
private fun Modifier.pointerDrag(start: (Float) -> Unit, drag: (Float) -> Unit, end: () -> Unit): Modifier =
    this.pointerInput(Unit) {
        detectVerticalDragGestures(
            onDragStart = { start(it.y) },
            onDragEnd = end,
            onDragCancel = end,
            onVerticalDrag = { change, _ -> change.consume(); drag(change.position.y) },
        )
    }

/**
 * Chooses the albums to add to, any number of them (and makes new ones,
 * chosen once made). Nothing is added until Add.
 */
@Composable
fun AlbumPicker(gallery: Gallery, onDismiss: () -> Unit, chose: (List<Album>) -> Unit) {
    var making by remember { mutableStateOf(false) }
    val picked = remember { mutableStateListOf<Album>() }
    if (making) {
        NewAlbum(gallery, onDismiss = { making = false }) { a ->
            if (picked.none { it.name.equals(a.name, true) }) picked.add(a)
            making = false
        }
        return
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Add to albums") },
        text = {
            // The albums there are, and new ones named here (not there until Add).
            val shown = gallery.albums + picked.filter { p -> gallery.albums.none { it.name.equals(p.name, true) } }
            LazyColumn {
                items(shown, key = { it.name.lowercase() }) { a ->
                    val on = picked.any { it.name.equals(a.name, true) }
                    val flip = { if (on) picked.removeAll { it.name.equals(a.name, true) } else picked.add(a) }
                    Row(Modifier.fillMaxWidth().clickable { flip() }, verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = on, onCheckedChange = { flip() })
                        Column {
                            Text(a.name)
                            Text(if (a.count > 0) "${a.count}" else "new", style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
                if (shown.isEmpty()) item { Text("No albums yet.") }
                item { TextButton(onClick = { making = true }) { Text("New album…") } }
            }
        },
        confirmButton = { TextButton(enabled = picked.isNotEmpty(), onClick = { chose(picked.toList()) }) { Text("Add") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Hands pictures to another app: copies in the cache, through a FileProvider. */
suspend fun share(context: Context, photos: List<Photo>) {
    val dir = File(context.cacheDir, "shared")
    dir.deleteRecursively()
    dir.mkdirs()
    val uris = ArrayList<android.net.Uri>()
    for (p in photos) {
        val cap = p.original ?: continue
        val bytes = core { Core.read(p.ns, cap, Images.MAX_ORIGINAL) } ?: core { p.preview?.let { Core.read(p.ns, it, Images.MAX_ORIGINAL) } } ?: continue
        val name = p.title.replace('/', '_').ifEmpty { "${p.id}.jpg" }
        val f = File(dir, name)
        f.writeBytes(bytes)
        uris += FileProvider.getUriForFile(context, "org.retifs.photos.shared", f)
    }
    if (uris.isEmpty()) return
    val intent = if (uris.size == 1) {
        Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris[0]).setType(photos[0].type.ifEmpty { "image/*" })
    } else {
        Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris).setType("image/*")
    }
    intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    context.startActivity(Intent.createChooser(intent, "Share").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}
