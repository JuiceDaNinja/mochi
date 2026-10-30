package app.mochi.phone.link

import com.google.crypto.tink.subtle.X25519
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Byte-for-byte the same scheme as crates/mochi-link/src/crypto.rs:
 *  - pairing: X25519; secret = X25519(myPrivate, theirPublic); code = pairingCode(secret, certFingerprint)
 *  - auth:    base64(HMAC-SHA256(secret, rawNonce))
 */
object Crypto {
    fun newPrivateKey(): ByteArray = X25519.generatePrivateKey()
    fun publicKeyB64(privateKey: ByteArray): String = b64(X25519.publicFromPrivate(privateKey))

    fun sharedSecret(privateKey: ByteArray, desktopPublicB64: String): ByteArray =
        X25519.computeSharedSecret(privateKey, unb64(desktopPublicB64))

    fun authHmac(secret: ByteArray, nonceB64: String): String {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(secret, "HmacSHA256"))
        return b64(mac.doFinal(unb64(nonceB64)))
    }

    /** Six digits from the shared secret followed by the desktop certificate's fingerprint
     *  (hex text), as in crates/mochi-link/src/tls.rs; FNV-1a 64 then mod 1e6. */
    fun pairingCode(secret: ByteArray, certFingerprint: String): String =
        pairingCode(secret + certFingerprint.toByteArray(Charsets.US_ASCII))

    /** FNV-1a 64 then mod 1e6, identical to mochi-core `pairing_code`. */
    fun pairingCode(input: ByteArray): String {
        var h = 0xcbf29ce484222325uL
        for (b in input) {
            h = h xor (b.toUByte().toULong())
            h *= 0x100000001b3uL
        }
        return (h % 1_000_000uL).toString().padStart(6, '0')
    }

    // java.util.Base64 (API 26+) rather than android.util so this file runs in JVM unit tests.
    fun b64(bytes: ByteArray): String = java.util.Base64.getEncoder().encodeToString(bytes)
    fun unb64(s: String): ByteArray = java.util.Base64.getDecoder().decode(s.trim())
}
