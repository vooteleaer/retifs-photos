package org.retifs.photos

import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.List
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Star
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.json.JSONArray

/**
 * One picture filling the screen, the others a swipe away: pinch or double
 * tap to zoom (the original is read once zoomed in), a tap hides the bars,
 * Info shows what's known of it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Viewer(gallery: Gallery, photos: List<Photo>, start: Int, close: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val pager = rememberPagerState(initialPage = start.coerceIn(0, (photos.size - 1).coerceAtLeast(0))) { photos.size }
    var chrome by remember { mutableStateOf(true) }
    var zoomed by remember { mutableStateOf(false) }
    var info by remember { mutableStateOf(false) }
    // Favourites changed here show at once.
    val current = { gallery.photos.firstOrNull { it.ns == photos[pager.currentPage].ns && it.id == photos[pager.currentPage].id } ?: photos[pager.currentPage] }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        HorizontalPager(state = pager, userScrollEnabled = !zoomed, beyondViewportPageCount = 1, key = { "${photos[it].ns}/${photos[it].id}" }) { page ->
            Zoomable(photos[page], active = page == pager.currentPage, tap = { chrome = !chrome }, zoomedChanged = { if (page == pager.currentPage) zoomed = it })
        }
        if (chrome && photos.isNotEmpty()) {
            val p = current()
            Row(Modifier.fillMaxWidth().background(Color.Black.copy(alpha = 0.4f)).statusBarsPadding(), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = close) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White) }
                Column(Modifier.weight(1f)) {
                    Text(dayLabel(p.day), color = Color.White, style = MaterialTheme.typography.titleSmall)
                    Text(p.whenTaken.drop(11).take(5), color = Color.White.copy(alpha = 0.7f), style = MaterialTheme.typography.bodySmall)
                }
            }
            Row(Modifier.align(Alignment.BottomCenter).fillMaxWidth().background(Color.Black.copy(alpha = 0.4f)).navigationBarsPadding(), horizontalArrangement = androidx.compose.foundation.layout.Arrangement.SpaceEvenly) {
                IconButton(onClick = { scope.launch { share(context, listOf(p)) } }) { Icon(Icons.Filled.Share, "Share", tint = Color.White) }
                IconButton(onClick = {
                    scope.launch {
                        val err = core { Core.setFavorite(JSONArray(listOf(p.ref())).toString(), !p.favorite) }
                        if (err.isEmpty()) gallery.update(listOf(p.copy(favorite = !p.favorite)))
                    }
                }) { Icon(Icons.Filled.Star, "Favourite", tint = if (p.favorite) Color.White else Color.White.copy(alpha = 0.4f)) }
                IconButton(onClick = { info = true }) { Icon(Icons.Filled.Info, "Info", tint = Color.White) }
            }
        }
    }
    if (info && photos.isNotEmpty()) {
        val p = current()
        ModalBottomSheet(onDismissRequest = { info = false }) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 32.dp)) {
                Text(p.title, style = MaterialTheme.typography.titleMedium)
                p.details.forEach { (label, value) ->
                    Row(Modifier.padding(top = 8.dp)) {
                        Text(label, Modifier.weight(0.3f), style = MaterialTheme.typography.bodySmall)
                        Text(value, Modifier.weight(0.7f))
                    }
                }
                val albums = p.albums
                if (albums.isNotEmpty()) {
                    Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Filled.List, null, Modifier.padding(end = 8.dp))
                        Text(albums.joinToString(", "))
                    }
                }
                p.geo?.let { (lat, lon) ->
                    val map = Intent(Intent.ACTION_VIEW, Uri.parse("geo:$lat,$lon?z=16")).setPackage(MAP_APP)
                    if (map.resolveActivity(context.packageManager) != null) {
                        OutlinedButton(onClick = { context.startActivity(map) }, modifier = Modifier.padding(top = 12.dp)) { Text("Show on the map") }
                    }
                }
            }
        }
    }
}

/** retifs' map app (shows a place it's handed as a geo: link). */
const val MAP_APP = "org.retifs.map"

/** A picture that zooms: its preview first, the original once zoomed in. */
@Composable
private fun Zoomable(p: Photo, active: Boolean, tap: () -> Unit, zoomedChanged: (Boolean) -> Unit) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var wantOriginal by remember { mutableStateOf(false) }
    val image by produceState<Bitmap?>(null, p.ns, p.id) { value = Images.best(p, 1800) }
    val sharp by produceState<Bitmap?>(null, p.ns, p.id, wantOriginal) {
        if (wantOriginal && p.original != null && p.original != p.preview) value = Images.load(p.ns, p.original, 4096)
    }
    LaunchedEffect(active) {
        if (!active) {
            scale = 1f
            offset = Offset.Zero
        }
    }
    LaunchedEffect(scale) {
        zoomedChanged(scale > 1.01f)
        if (scale > 1.5f) wantOriginal = true
    }
    Box(
        Modifier.fillMaxSize()
            .pointerInput(p.id) {
                detectTapGestures(
                    onTap = { tap() },
                    onDoubleTap = { at ->
                        if (scale > 1f) {
                            scale = 1f
                            offset = Offset.Zero
                        } else {
                            scale = 2.5f
                            offset = (Offset(size.width / 2f, size.height / 2f) - at) * 1.5f
                        }
                    },
                )
            }
            .pointerInput(p.id) {
                // Two fingers zoom; one pans only when zoomed (else the pager swipes).
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    do {
                        val event = awaitPointerEvent()
                        val fingers = event.changes.count { it.pressed }
                        if (fingers > 1 || scale > 1f) {
                            val z = event.calculateZoom()
                            scale = (scale * z).coerceIn(1f, 6f)
                            val maxX = size.width * (scale - 1) / 2
                            val maxY = size.height * (scale - 1) / 2
                            offset = if (scale <= 1f) Offset.Zero else (offset + event.calculatePan()).let { Offset(it.x.coerceIn(-maxX, maxX), it.y.coerceIn(-maxY, maxY)) }
                            event.changes.forEach { if (it.positionChanged()) it.consume() }
                        }
                    } while (event.changes.any { it.pressed })
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        val shown = sharp ?: image
        if (shown == null) {
            CircularProgressIndicator()
        } else {
            Image(
                shown.asImageBitmap(), p.title, contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxSize().graphicsLayer { scaleX = scale; scaleY = scale; translationX = offset.x; translationY = offset.y },
            )
        }
    }
}
