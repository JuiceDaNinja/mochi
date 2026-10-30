package app.mochi.phone.link

import android.content.Context
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/** The one paired desktop: where it is, the shared secret, and the pinned TLS certificate, in encrypted preferences. */
data class Pairing(val desktopName: String, val host: String, val port: Int, val deviceId: String, val secret: ByteArray, val certFingerprint: String)

class PairStore(context: Context) {
    private val prefs = EncryptedSharedPreferences.create(
        context,
        "mochi-pairing",
        MasterKey.Builder(context).setKeyScheme(MasterKey.KeyScheme.AES256_GCM).build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
    )

    fun load(): Pairing? {
        val id = prefs.getString("device_id", null) ?: return null
        val secret = prefs.getString("secret", null) ?: return null
        // Pairings from before the link was encrypted have no pin; they must pair again.
        val fp = prefs.getString("cert_fp", null) ?: run { clear(); return null }
        return Pairing(
            desktopName = prefs.getString("desktop_name", "") ?: "",
            host = prefs.getString("host", "") ?: "",
            port = prefs.getInt("port", 0),
            deviceId = id,
            secret = Crypto.unb64(secret),
            certFingerprint = fp,
        )
    }

    fun save(p: Pairing) {
        prefs.edit()
            .putString("device_id", p.deviceId)
            .putString("secret", Crypto.b64(p.secret))
            .putString("cert_fp", p.certFingerprint)
            .putString("desktop_name", p.desktopName)
            .putString("host", p.host)
            .putInt("port", p.port)
            .apply()
    }

    /** The desktop may move to a new address; keep the key, update where it lives. */
    fun updateAddress(host: String, port: Int) {
        prefs.edit().putString("host", host).putInt("port", port).apply()
    }

    fun clear() = prefs.edit().clear().apply()
}
