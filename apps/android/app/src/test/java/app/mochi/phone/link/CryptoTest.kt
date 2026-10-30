package app.mochi.phone.link

import org.junit.Assert.assertEquals
import org.junit.Test

/** Vectors produced by the Rust side (crates/mochi-link); both must agree or pairing fails. */
class CryptoTest {
    @Test
    fun pairingCodeMatchesDesktop() {
        val secret = ByteArray(32) { it.toByte() }
        assertEquals("628869", Crypto.pairingCode(secret))
        assertEquals("764634", Crypto.pairingCode("nonce-1".toByteArray()))
    }

    @Test
    fun authHmacMatchesDesktop() {
        val secret = ByteArray(32) { it.toByte() }
        val nonceB64 = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="
        assertEquals("rfyvxWCEWzSmagrgM60fx7Q2Y1SIVuS+PMngHB5uEAA=", Crypto.authHmac(secret, nonceB64))
    }

    @Test
    fun protocolRoundTrips() {
        assertEquals("""{"type":"utterance","text":"reply to Sara","final":true}""", PhoneMessage.Utterance("reply to Sara", true).toJson())
        val m = DesktopMessage.parse("""{"type":"state","state":"confirm","line":"Send it?"}""") as DesktopMessage.State
        assertEquals(MochiState.CONFIRM, m.state)
        assertEquals("Send it?", m.line)
    }
}
