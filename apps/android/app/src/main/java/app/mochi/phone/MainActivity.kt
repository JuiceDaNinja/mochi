package app.mochi.phone

import android.Manifest
import android.os.Build
import android.os.Bundle
import android.speech.tts.TextToSpeech
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import app.mochi.phone.link.Desktop
import app.mochi.phone.link.DesktopLink
import app.mochi.phone.link.LinkUi
import app.mochi.phone.link.PairStore
import app.mochi.phone.link.discoverDesktops
import app.mochi.phone.ui.ConnectScreen
import app.mochi.phone.ui.HomeScreen
import app.mochi.phone.ui.PairScreen
import app.mochi.phone.voice.Listener
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.flow.onEach
import java.util.Locale

class LinkViewModel(app: android.app.Application) : androidx.lifecycle.AndroidViewModel(app) {
    val store = PairStore(app)
    val link = DesktopLink(store, viewModelScope, deviceName = Build.MODEL ?: "Android phone")
    val desktops = MutableStateFlow<List<Desktop>>(emptyList())

    init {
        discoverDesktops(app).onEach { found ->
            desktops.value = found
            // The paired computer may have moved (new IP after a reboot, or a different port):
            // follow it to wherever discovery sees it now, and reconnect without asking.
            val saved = store.load() ?: return@onEach
            val seen = found.firstOrNull { it.name == saved.desktopName } ?: return@onEach
            if (seen.host != saved.host || seen.port != saved.port) store.updateAddress(seen.host, seen.port)
            if (link.ui.value.phase == LinkUi.Phase.Disconnected) link.connect(seen)
        }.launchIn(viewModelScope)
        // A paired desktop at its last known address: try it right away.
        store.load()?.let { if (it.host.isNotBlank()) link.connect(Desktop(it.desktopName, it.host, it.port)) }
    }
}

class MainActivity : ComponentActivity() {
    private var tts: TextToSpeech? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}.launch(Manifest.permission.RECORD_AUDIO)
        // Mochi's lines are spoken on the desktop; the phone repeats confirm questions so the
        // user hears them from the couch. Pitched up to match the character.
        tts = TextToSpeech(this) { if (it == TextToSpeech.SUCCESS) { tts?.language = Locale.US; tts?.setPitch(1.5f); tts?.setSpeechRate(1.05f) } }
        setContent { App { text -> tts?.speak(text, TextToSpeech.QUEUE_FLUSH, null, "mochi") } }
    }

    override fun onDestroy() { tts?.shutdown(); super.onDestroy() }
}

@Composable
fun App(speak: (String) -> Unit) {
    val vm: LinkViewModel = viewModel()
    val ui by vm.link.ui.collectAsState()
    val desktops by vm.desktops.collectAsState()
    val context = LocalContext.current
    var listening by remember { mutableStateOf(false) }
    var lastConfirm by remember { mutableStateOf<String?>(null) }
    val listener = remember {
        Listener(
            context,
            onPartial = { vm.link.userSpeaking(); vm.link.utterance(it, final = false) },
            onFinal = { vm.link.utterance(it, final = true) },
            onState = { listening = it },
        )
    }
    if (ui.confirmText != null && ui.confirmText != lastConfirm) { lastConfirm = ui.confirmText; speak(ui.confirmText!!) }
    if (ui.confirmText == null) lastConfirm = null

    val remembered = vm.store.load()?.let { if (it.host.isNotBlank()) Desktop(it.desktopName, it.host, it.port) else null }
    when (ui.phase) {
        LinkUi.Phase.Disconnected, LinkUi.Phase.Connecting ->
            ConnectScreen(desktops, remembered, ui.error ?: if (ui.phase == LinkUi.Phase.Connecting) "Connecting to ${ui.desktopName}…" else null) { vm.link.connect(it) }
        LinkUi.Phase.Pairing, LinkUi.Phase.WaitingApproval ->
            PairScreen(ui, onApprove = { vm.link.approvePairing() }, onCancel = { vm.link.disconnect() })
        LinkUi.Phase.Connected ->
            HomeScreen(
                ui,
                listening,
                onTalk = { vm.link.wake(); listener.start() },
                onAnswer = { vm.link.answer(it) },
                onStop = { vm.link.stop() },
                onForget = { vm.link.disconnect(); vm.store.clear() },
            )
    }
}
