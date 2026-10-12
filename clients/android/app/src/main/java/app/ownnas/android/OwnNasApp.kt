package app.ownnas.android

import android.app.Application
import app.ownnas.android.data.ServerStore
import app.ownnas.android.data.Servers

class OwnNasApp : Application() {
    val store: ServerStore by lazy { ServerStore(this) }
    val servers: Servers by lazy { Servers(this, store) }
}
