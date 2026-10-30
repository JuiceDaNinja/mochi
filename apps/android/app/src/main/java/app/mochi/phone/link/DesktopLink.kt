package app.mochi.phone.link

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit

/** Everything the UI needs to draw, in one value. */
data class LinkUi(
    val phase: Phase = Phase.Disconnected,
    val desktopName: String = "",
    val pairCode: String? = null,
    val mochi: MochiState = MochiState.IDLE,
    val caption: String? = null,
    val confirmText: String? = null,
    val error: String? = null,
    /** "heart" or "thumbs_up" while a success gesture should play; null otherwise. */
    val gesture: String? = null,
    /** Last few link events, newest last; shown on the pairing screen for troubleshooting. */
    val trail: List<String> = emptyList(),
) {
    enum class Phase { Disconnected, Connecting, Pairing, WaitingApproval, Connected }
}

/**
 * The phone's side of the link. Pairs once, then reconnects with HMAC auth whenever the
 * network comes back; the user never re-approves on the same phone + desktop pair.
 * Both apps have to be running: there is no wake-on-LAN and no background service.
 */
class DesktopLink(private val store: PairStore, private val scope: CoroutineScope, private val deviceName: String) {
    private val client = OkHttpClient.Builder().pingInterval(0, TimeUnit.SECONDS).readTimeout(0, TimeUnit.SECONDS).build()
    private val _ui = MutableStateFlow(LinkUi())
    val ui: StateFlow<LinkUi> = _ui

    private var socket: WebSocket? = null
    private var reconnectJob: Job? = null
    private var pairing: PendingPair? = null
    private var target: Desktop? = null
    private var wantConnected = false
    private var successUntil = 0L

    private class PendingPair(val privateKey: ByteArray, var secret: ByteArray? = null, var phoneApproved: Boolean = false)

    /** Connect to a known desktop (auth) or start pairing with a new one. */
    fun connect(desktop: Desktop) {
        val phase = _ui.value.phase
        if (phase != LinkUi.Phase.Disconnected && target?.host == desktop.host && target?.port == desktop.port) {
            note("already connecting to ${desktop.name}; ignoring second tap")
            return
        }
        target = desktop
        wantConnected = true
        open()
    }

    fun disconnect() {
        wantConnected = false
        reconnectJob?.cancel()
        socket?.close(1000, "bye")
        socket = null
        _ui.update { it.copy(phase = LinkUi.Phase.Disconnected) }
    }

    /** The user tapped Approve on the phone during pairing. */
    fun approvePairing() {
        pairing?.phoneApproved = true
        val sent = socket?.send(PhoneMessage.PairApprove.toJson()) ?: false
        note(if (sent) "→ PairApprove" else "→ PairApprove NOT sent (socket closed)")
        _ui.update { it.copy(phase = LinkUi.Phase.WaitingApproval) }
    }

    fun send(m: PhoneMessage) { socket?.send(m.toJson()) }
    fun wake() = send(PhoneMessage.Wake)
    fun utterance(text: String, final: Boolean) = send(PhoneMessage.Utterance(text, final))
    fun userSpeaking() = send(PhoneMessage.UserSpeaking)
    fun answer(yes: Boolean) { send(PhoneMessage.Answer(yes)); _ui.update { it.copy(confirmText = null) } }
    fun stop() = send(PhoneMessage.Stop)

    private fun open() {
        val d = target ?: return
        // Never two sockets at once: a second connection would split the pairing approvals.
        socket?.let { old -> socket = null; old.cancel(); note("replaced an older socket") }
        pairing = null
        _ui.update { it.copy(phase = LinkUi.Phase.Connecting, desktopName = d.name, error = null) }
        val req = Request.Builder().url("ws://${d.host}:${d.port}/").build()
        socket = client.newWebSocket(req, listener)
    }

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) { note("socket open") }
        override fun onMessage(webSocket: WebSocket, text: String) {
            if (webSocket !== socket || text.length > MAX_FRAME_BYTES) return
            val m = try { DesktopMessage.parse(text) } catch (e: Exception) { note("bad frame: ${e.message}"); return }
            note("← ${m::class.simpleName}")
            try { handle(m) } catch (e: Exception) { note("handler failed: ${e::class.simpleName}: ${e.message}"); _ui.update { it.copy(error = "${e::class.simpleName}: ${e.message}") } }
        }
        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            if (webSocket !== socket) return // an old, replaced socket going away
            note("failure: ${t.message}")
            _ui.update { it.copy(phase = LinkUi.Phase.Disconnected, error = t.message) }
            scheduleReconnect()
        }
        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            if (webSocket !== socket) return
            note("closed $code $reason")
            _ui.update { it.copy(phase = LinkUi.Phase.Disconnected) }
            scheduleReconnect()
        }
    }

    private fun note(line: String) {
        _ui.update { it.copy(trail = (it.trail + line).takeLast(8)) }
    }

    private fun handle(m: DesktopMessage) {
        val saved = store.load()
        val d = target
        when (m) {
            is DesktopMessage.Hello -> {
                // Known desktop: authenticate. New desktop: pair.
                if (saved != null && d != null && saved.desktopName == m.desktopName) {
                    store.updateAddress(d.host, d.port)
                    send(PhoneMessage.Auth(saved.deviceId, Crypto.authHmac(saved.secret, m.nonceB64)))
                } else {
                    val p = PendingPair(Crypto.newPrivateKey())
                    pairing = p
                    send(PhoneMessage.PairRequest(deviceName, Crypto.publicKeyB64(p.privateKey)))
                }
            }
            is DesktopMessage.PairChallenge -> {
                val p = pairing ?: return
                p.secret = Crypto.sharedSecret(p.privateKey, m.desktopPublicKeyB64)
                val code = Crypto.pairingCode(p.secret!!)
                // Show OUR code; if it differs from the desktop's, someone is in the middle.
                _ui.update { it.copy(phase = LinkUi.Phase.Pairing, desktopName = m.desktopName, pairCode = code) }
            }
            is DesktopMessage.Paired -> {
                // Move on first so the screen never sticks; then persist.
                val p = pairing
                val secret = p?.secret
                pairing = null
                _ui.update { it.copy(phase = LinkUi.Phase.Connected, pairCode = null, mochi = MochiState.IDLE, caption = "Paired with ${it.desktopName}", error = null) }
                if (secret == null || d == null) { note("paired but no secret/target to save"); return }
                try {
                    store.save(Pairing(_ui.value.desktopName, d.host, d.port, m.deviceId, secret))
                } catch (e: Exception) {
                    note("save failed: ${e.message}")
                    _ui.update { it.copy(error = "Paired, but could not save the pairing: ${e.message}") }
                }
            }
            DesktopMessage.PairDenied -> { pairing = null; _ui.update { it.copy(phase = LinkUi.Phase.Disconnected, pairCode = null, error = "Pairing was not approved on both devices.") }; wantConnected = false }
            DesktopMessage.AuthOk -> {
                d?.let { store.updateAddress(it.host, it.port) }
                _ui.update { it.copy(phase = LinkUi.Phase.Connected, mochi = MochiState.IDLE, error = null) }
            }
            DesktopMessage.AuthFailed -> { store.clear(); _ui.update { it.copy(phase = LinkUi.Phase.Disconnected, error = "This computer no longer knows this phone. Pair again.") }; wantConnected = false }
            is DesktopMessage.State -> {
                if (m.state == MochiState.IDLE && System.currentTimeMillis() < successUntil) {
                    scope.launch { delay(successUntil - System.currentTimeMillis()); _ui.update { it.copy(mochi = MochiState.IDLE) } }
                } else {
                    _ui.update { it.copy(mochi = m.state, caption = m.line ?: it.caption, confirmText = if (m.state == MochiState.CONFIRM) it.confirmText else null) }
                }
            }
            is DesktopMessage.Narrate -> _ui.update { it.copy(caption = m.text) }
            is DesktopMessage.ConfirmRequest -> _ui.update { it.copy(mochi = MochiState.CONFIRM, caption = m.text, confirmText = m.text) }
            is DesktopMessage.Done -> {
                _ui.update { it.copy(mochi = MochiState.SUCCESS, caption = m.summary, confirmText = null, gesture = m.kind) }
                successUntil = System.currentTimeMillis() + 1_800
                scope.launch { delay(1_500); _ui.update { it.copy(gesture = null) } }
            }
            DesktopMessage.Sleepy -> _ui.update { it.copy(mochi = MochiState.IDLE, caption = "Mochi's asleep on the computer…") }
            DesktopMessage.Ping -> send(PhoneMessage.Pong)
            is DesktopMessage.Unknown -> {}
        }
    }

    /** 1 s, 2 s, 5 s, then every 10 s until the desktop is back. Every other attempt
     *  tries Mochi's default port on the same host, in case the saved port is stale. */
    private fun scheduleReconnect() {
        if (!wantConnected || reconnectJob?.isActive == true) return
        reconnectJob = scope.launch {
            var attempt = 0
            val original = target
            while (wantConnected && _ui.value.phase == LinkUi.Phase.Disconnected) {
                delay(RECONNECT_BACKOFF_MS[minOf(attempt, RECONNECT_BACKOFF_MS.size - 1)])
                attempt++
                val o = original ?: break
                target = if (attempt % 2 == 0 && o.port != DEFAULT_PORT) o.copy(port = DEFAULT_PORT) else o
                open()
                delay(1_500) // give the attempt a moment before judging it
            }
        }
    }
}
