package app.ownnas.android

import android.app.Application
import app.ownnas.android.data.ServerStore

class OwnNasApp : Application() {
    val store: ServerStore by lazy { ServerStore(this) }
}
