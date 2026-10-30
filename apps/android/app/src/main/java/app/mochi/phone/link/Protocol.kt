package app.mochi.phone.link

import org.json.JSONObject

// Mirrors crates/mochi-core/src/protocol.rs. One JSON object per WebSocket text frame.

const val SERVICE_TYPE = "_mochi._tcp."
const val MAX_FRAME_BYTES = 64 * 1024
val RECONNECT_BACKOFF_MS = longArrayOf(1_000, 2_000, 5_000, 10_000)

/** Phone -> desktop. */
sealed class PhoneMessage {
    data class PairRequest(val deviceName: String, val publicKeyB64: String) : PhoneMessage()
    object PairApprove : PhoneMessage()
    data class Auth(val deviceId: String, val hmacB64: String) : PhoneMessage()
    object Wake : PhoneMessage()
    data class Utterance(val text: String, val isFinal: Boolean) : PhoneMessage()
    object UserSpeaking : PhoneMessage()
    data class Answer(val yes: Boolean) : PhoneMessage()
    object Stop : PhoneMessage()
    object Pong : PhoneMessage()

    fun toJson(): String {
        val o = JSONObject()
        when (this) {
            is PairRequest -> { o.put("type", "pair_request"); o.put("device_name", deviceName); o.put("public_key_b64", publicKeyB64) }
            PairApprove -> o.put("type", "pair_approve")
            is Auth -> { o.put("type", "auth"); o.put("device_id", deviceId); o.put("hmac_b64", hmacB64) }
            Wake -> o.put("type", "wake")
            is Utterance -> { o.put("type", "utterance"); o.put("text", text); o.put("final", isFinal) }
            UserSpeaking -> o.put("type", "user_speaking")
            is Answer -> { o.put("type", "answer"); o.put("yes", yes) }
            Stop -> o.put("type", "stop")
            Pong -> o.put("type", "pong")
        }
        return o.toString()
    }
}

/** Mochi's states, same names as the desktop. */
enum class MochiState { IDLE, LISTENING, THINKING, WORKING, SUCCESS, LOST, CONFIRM, LEARNING;
    companion object {
        fun parse(s: String?) = entries.firstOrNull { it.name.equals(s ?: "", ignoreCase = true) } ?: IDLE
    }
}

/** Desktop -> phone. */
sealed class DesktopMessage {
    data class Hello(val desktopName: String, val nonceB64: String) : DesktopMessage()
    data class PairChallenge(val desktopName: String, val code: String, val desktopPublicKeyB64: String) : DesktopMessage()
    data class Paired(val deviceId: String) : DesktopMessage()
    object PairDenied : DesktopMessage()
    object AuthOk : DesktopMessage()
    object AuthFailed : DesktopMessage()
    data class State(val state: MochiState, val line: String?) : DesktopMessage()
    data class Narrate(val text: String, val audioUrl: String?) : DesktopMessage()
    data class ConfirmRequest(val text: String) : DesktopMessage()
    data class Done(val summary: String) : DesktopMessage()
    object Sleepy : DesktopMessage()
    object Ping : DesktopMessage()
    data class Unknown(val type: String) : DesktopMessage()

    companion object {
        fun parse(json: String): DesktopMessage {
            val o = JSONObject(json)
            return when (val t = o.optString("type")) {
                "hello" -> Hello(o.optString("desktop_name"), o.optString("nonce_b64"))
                "pair_challenge" -> PairChallenge(o.optString("desktop_name"), o.optString("code"), o.optString("desktop_public_key_b64"))
                "paired" -> Paired(o.optString("device_id"))
                "pair_denied" -> PairDenied
                "auth_ok" -> AuthOk
                "auth_failed" -> AuthFailed
                "state" -> State(MochiState.parse(o.optString("state")), if (o.has("line")) o.optString("line") else null)
                "narrate" -> Narrate(o.optString("text"), if (o.has("audio_url")) o.optString("audio_url") else null)
                "confirm_request" -> ConfirmRequest(o.optString("text"))
                "done" -> Done(o.optString("summary"))
                "sleepy" -> Sleepy
                "ping" -> Ping
                else -> Unknown(t)
            }
        }
    }
}
