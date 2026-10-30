package app.mochi.phone.link

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow

data class Desktop(val name: String, val host: String, val port: Int)

/** Finds `_mochi._tcp` desktops on the local network. Emits the growing list. */
fun discoverDesktops(context: Context): Flow<List<Desktop>> = callbackFlow {
    val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
    val found = linkedMapOf<String, Desktop>()
    val listener = object : NsdManager.DiscoveryListener {
        override fun onServiceFound(info: NsdServiceInfo) {
            nsd.resolveService(info, object : NsdManager.ResolveListener {
                override fun onServiceResolved(r: NsdServiceInfo) {
                    val host = r.host?.hostAddress ?: return
                    val label = r.attributes["name"]?.let { String(it) } ?: r.serviceName
                    found[r.serviceName] = Desktop(label, host, r.port)
                    trySend(found.values.toList())
                }
                override fun onResolveFailed(info: NsdServiceInfo, code: Int) {}
            })
        }
        override fun onServiceLost(info: NsdServiceInfo) {
            found.remove(info.serviceName)
            trySend(found.values.toList())
        }
        override fun onDiscoveryStarted(type: String) {}
        override fun onDiscoveryStopped(type: String) {}
        override fun onStartDiscoveryFailed(type: String, code: Int) { close() }
        override fun onStopDiscoveryFailed(type: String, code: Int) {}
    }
    nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
    awaitClose { runCatching { nsd.stopServiceDiscovery(listener) } }
}
