package app.mochi.phone.link

import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.X509TrustManager

/**
 * The phone's trust model for the desktop's self-signed certificate (mirrors crates/mochi-link/src/tls.rs):
 *
 *  - **Pairing** (nothing pinned yet): accept whatever certificate the desktop presents, but
 *    remember its fingerprint — it is mixed into the 6-digit code, so a man-in-the-middle
 *    shows a different code on the two screens and the user denies.
 *  - **Paired**: only the pinned fingerprint is accepted; anything else fails the handshake.
 *
 * Fingerprint = SHA-256 of the DER certificate, lowercase hex, same as the desktop computes.
 */
class PinnedTrust(private val pinned: () -> String?) : X509TrustManager {
    /** Fingerprint of the certificate seen on the most recent handshake. */
    @Volatile var lastSeen: String? = null
        private set

    override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
        val leaf = chain.firstOrNull() ?: throw CertificateException("no certificate")
        val fp = fingerprint(leaf.encoded)
        lastSeen = fp
        val want = pinned()
        if (want != null && !want.equals(fp, ignoreCase = true)) {
            throw CertificateException("certificate changed: expected ${want.take(16)}…, got ${fp.take(16)}…")
        }
    }

    override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) = throw CertificateException("not a server")
    override fun getAcceptedIssuers(): Array<X509Certificate> = arrayOf()

    companion object {
        fun fingerprint(der: ByteArray): String =
            MessageDigest.getInstance("SHA-256").digest(der).joinToString("") { "%02x".format(it) }
    }
}
