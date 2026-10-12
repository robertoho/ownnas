package app.ownnas.android

import android.app.Application
import app.ownnas.android.data.ServerStore
import app.ownnas.android.data.Servers
import app.ownnas.android.data.Syncs

class OwnNasApp : Application() {
    val store: ServerStore by lazy { ServerStore(this) }
    val servers: Servers by lazy { Servers(this, store) }
    val syncs: Syncs by lazy { Syncs(this, servers) }
}
