package app.mochi.phone.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.offset
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.ui.draw.alpha
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.mochi.phone.link.Desktop
import app.mochi.phone.link.LinkUi
import app.mochi.phone.link.MochiState

val Cream = Color(0xFFF6F1EB)
val Pink = Color(0xFFFFB3BF)
val PinkInk = Color(0xFF5A1F2B)
val Muted = Color(0xFF8A7A6F)

/** Pick a computer from the list, or type its address. */
@Composable
fun ConnectScreen(desktops: List<Desktop>, remembered: Desktop?, error: String?, onConnect: (Desktop) -> Unit) {
    var host by remember { mutableStateOf(remembered?.host ?: "") }
    var port by remember { mutableStateOf(remembered?.port?.toString() ?: "") }
    Column(Modifier.fillMaxSize().background(Cream).padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Spacer(Modifier.height(24.dp))
        MochiFace(MochiState.IDLE, Modifier.size(140.dp))
        Spacer(Modifier.height(8.dp))
        Text("Where's my computer?", fontSize = 22.sp, fontWeight = FontWeight.Bold)
        Text("Open Mochi on the computer first. Same Wi-Fi.", color = Muted, textAlign = TextAlign.Center)
        Spacer(Modifier.height(16.dp))
        if (remembered != null) {
            DesktopRow(remembered, "paired", onConnect)
        }
        desktops.filter { it.name != remembered?.name }.forEach { DesktopRow(it, "nearby", onConnect) }
        if (desktops.isEmpty() && remembered == null) {
            Row(verticalAlignment = Alignment.CenterVertically) { CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp); Spacer(Modifier.width(8.dp)); Text("Looking…", color = Muted) }
        }
        Spacer(Modifier.height(20.dp))
        Text("Or type the address from the computer's settings", color = Muted, fontSize = 13.sp)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(host, { host = it }, Modifier.weight(2f), label = { Text("IP address") }, singleLine = true)
            OutlinedTextField(port, { port = it.filter(Char::isDigit) }, Modifier.weight(1f), label = { Text("Port") }, singleLine = true)
        }
        Spacer(Modifier.height(8.dp))
        Button(onClick = { port.toIntOrNull()?.let { onConnect(Desktop(host, host, it)) } }, enabled = host.isNotBlank() && port.isNotBlank(), colors = ButtonDefaults.buttonColors(containerColor = Pink, contentColor = PinkInk)) { Text("Connect") }
        if (error != null) { Spacer(Modifier.height(12.dp)); Text(error, color = Color(0xFFA04050), textAlign = TextAlign.Center) }
    }
}

@Composable
private fun DesktopRow(d: Desktop, tag: String, onConnect: (Desktop) -> Unit) {
    Card(Modifier.fillMaxWidth().padding(vertical = 4.dp), colors = CardDefaults.cardColors(containerColor = Color.White)) {
        Row(Modifier.padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) { Text(d.name, fontWeight = FontWeight.SemiBold); Text("${d.host}:${d.port} · $tag", color = Muted, fontSize = 12.sp) }
            Button(onClick = { onConnect(d) }, colors = ButtonDefaults.buttonColors(containerColor = Pink, contentColor = PinkInk)) { Text("Connect") }
        }
    }
}

/** Both devices show the same six digits; the user approves on both. */
@Composable
fun PairScreen(ui: LinkUi, onApprove: () -> Unit, onCancel: () -> Unit) {
    Column(Modifier.fillMaxSize().background(Cream).padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        MochiFace(MochiState.CONFIRM, Modifier.size(140.dp))
        Spacer(Modifier.height(16.dp))
        Text("Connecting to ${ui.desktopName}", fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("Approve only if the computer shows this code:", color = Muted, textAlign = TextAlign.Center)
        Spacer(Modifier.height(12.dp))
        Text(ui.pairCode?.let { it.take(3) + " " + it.drop(3) } ?: "…", fontSize = 44.sp, fontWeight = FontWeight.ExtraBold, color = PinkInk, letterSpacing = 4.sp)
        Spacer(Modifier.height(20.dp))
        if (ui.phase == LinkUi.Phase.WaitingApproval) {
            Row(verticalAlignment = Alignment.CenterVertically) { CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp); Spacer(Modifier.width(8.dp)); Text("Waiting for the computer to approve…", color = Muted) }
        } else {
            Button(onClick = onApprove, colors = ButtonDefaults.buttonColors(containerColor = Pink, contentColor = PinkInk)) { Text("Approve") }
        }
        TextButton(onClick = onCancel) { Text("Cancel") }
        if (ui.error != null) Text(ui.error, color = Color(0xFFA04050), textAlign = TextAlign.Center, fontSize = 13.sp)
        Spacer(Modifier.height(16.dp))
        ui.trail.forEach { Text(it, color = Muted, fontSize = 11.sp) }
    }
}

/** Mochi's face mirrors the desktop; tap to talk; Yes/No when Mochi asks. */
@Composable
fun HomeScreen(ui: LinkUi, listening: Boolean, onTalk: () -> Unit, onAnswer: (Boolean) -> Unit, onStop: () -> Unit, onForget: () -> Unit) {
    Column(Modifier.fillMaxSize().background(Cream).padding(20.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(ui.desktopName, color = Muted, modifier = Modifier.weight(1f))
            TextButton(onClick = onForget) { Text("Forget", color = Muted) }
        }
        Spacer(Modifier.weight(1f))
        Card(Modifier.fillMaxWidth(0.9f), shape = RoundedCornerShape(18.dp), colors = CardDefaults.cardColors(containerColor = Color.White)) {
            Text(ui.caption ?: hintFor(ui.mochi), Modifier.padding(16.dp).fillMaxWidth(), textAlign = TextAlign.Center, fontSize = 18.sp)
        }
        Spacer(Modifier.height(12.dp))
        Box(Modifier.fillMaxWidth(0.7f).aspectRatio(120f / 112f)) {
            MochiFace(if (listening) MochiState.LISTENING else ui.mochi, Modifier.fillMaxSize())
            if (ui.gesture != null) GestureBurst(ui.gesture, Modifier.align(Alignment.TopEnd))
        }
        Spacer(Modifier.weight(1f))
        if (ui.confirmText != null) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Button(onClick = { onAnswer(true) }, Modifier.weight(1f).height(60.dp), colors = ButtonDefaults.buttonColors(containerColor = Pink, contentColor = PinkInk)) { Text("Yes", fontSize = 20.sp, fontWeight = FontWeight.Bold) }
                OutlinedButton(onClick = { onAnswer(false) }, Modifier.weight(1f).height(60.dp)) { Text("No", fontSize = 20.sp, fontWeight = FontWeight.Bold) }
            }
        } else {
            Box(Modifier.size(96.dp).background(if (listening) PinkInk else Pink, CircleShape), contentAlignment = Alignment.Center) {
                TextButton(onClick = onTalk, Modifier.fillMaxSize()) { Text(if (listening) "…" else "Talk", color = if (listening) Color.White else PinkInk, fontWeight = FontWeight.Bold, fontSize = 18.sp) }
            }
            Spacer(Modifier.height(8.dp))
            Text(if (listening) "Listening…" else "Tap and say what Mochi should do", color = Muted, fontSize = 13.sp)
        }
        Spacer(Modifier.height(12.dp))
        if (ui.mochi != MochiState.IDLE) TextButton(onClick = onStop) { Text("Stop", color = Color(0xFFA04050)) }
        Spacer(Modifier.height(8.dp))
    }
}

/** The desktop's heart / thumbs-up, floating up beside the face for a moment. */
@Composable
private fun GestureBurst(kind: String, modifier: Modifier = Modifier) {
    val t = rememberInfiniteTransition(label = "burst")
    val k by t.animateFloat(0f, 1f, infiniteRepeatable(tween(1400, easing = LinearEasing), RepeatMode.Restart), label = "k")
    val rise = (k * 40).dp
    val alpha = if (k < 0.2f) k / 0.2f else if (k > 0.75f) (1f - k) / 0.25f else 1f
    Text(
        if (kind == "heart") "\uD83D\uDC97" else "\uD83D\uDC4D",
        modifier.offset(x = (-8).dp, y = -rise).alpha(alpha),
        fontSize = 40.sp,
    )
}

private fun hintFor(s: MochiState) = when (s) {
    MochiState.IDLE -> "Mm? I'm here!"
    MochiState.LISTENING -> "Listening…"
    MochiState.THINKING -> "thinking…"
    MochiState.WORKING -> "On it!"
    MochiState.SUCCESS -> "Done!"
    MochiState.LOST -> "Mochi… can't find it… show me on the computer?"
    MochiState.CONFIRM -> "Are you sure?"
    MochiState.LEARNING -> "Oh! There it is! Thank you!"
}
