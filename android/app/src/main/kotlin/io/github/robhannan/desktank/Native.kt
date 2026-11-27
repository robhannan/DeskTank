package io.github.robhannan.desktank

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.SystemClock
import android.view.Surface
import java.util.concurrent.atomic.AtomicLong

/**
 * JNI bridge to the shared Rust render thread and tank (SPEC §3.6).
 * Both hosts use the same counter so the newest drawable instance wins.
 */
internal object Native {
    init {
        System.loadLibrary("desktank_android")
    }

    private val tokens = AtomicLong()
    private var initialized = false

    fun nextToken(): Long = tokens.incrementAndGet()

    /** Called from each host's onCreate, on the main thread. */
    @Synchronized
    fun initialize(context: Context) {
        if (!initialized) {
            val television = context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
            init(context.filesDir.absolutePath, television, SystemClock.uptimeMillis())
            initialized = true
        }
    }

    private const val PREFS = "desktank"
    private const val OLED = "oled"

    /**
     * Passes the OLED-mode flag (true black water and air; off by default) to
     * Rust. Until there is a settings screen (#404) it is set over adb:
     * `am start -n io.github.robhannan.desktank/.DeskTankActivity --ez oled true`
     * stores the extra in SharedPreferences, so the Dream reads it too.
     * Pass the Activity's `intent` to take an `oled` extra; a Dream passes none.
     * Only a boolean extra (`--ez`) counts: any other type reads back as both
     * defaults, so it is ignored rather than quietly turning OLED off. The
     * launcher Activity is exported, so another app could set this too; it is
     * cosmetic, and the settings screen (#404) replaces the extra.
     */
    fun applyOled(context: Context, id: Long, intent: Intent? = null) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (intent != null && intent.hasExtra(OLED)) {
            val value = intent.getBooleanExtra(OLED, false)
            if (value == intent.getBooleanExtra(OLED, true)) {
                prefs.edit().putBoolean(OLED, value).apply()
            }
        }
        setOled(id, prefs.getBoolean(OLED, false))
    }

    @JvmStatic private external fun setOled(id: Long, enabled: Boolean)

    @JvmStatic private external fun init(path: String, television: Boolean, uptimeMs: Long)

    @JvmStatic external fun surfaceCreated(id: Long, surface: Surface)

    @JvmStatic external fun surfaceChanged(id: Long, width: Int, height: Int)

    /** Returns once Rust has let go of the surface. */
    @JvmStatic external fun surfaceDestroyed(id: Long)

    /** From onResume for the Activity and onDreamingStarted for the Dream. */
    @JvmStatic external fun started(id: Long, app: Boolean)

    @JvmStatic external fun stopped(id: Long)

    /** One pointer sample in surface pixels and milliseconds since boot (uptime). */
    @JvmStatic external fun touch(
        id: Long,
        action: Int,
        pointerId: Int,
        x: Float,
        y: Float,
        eventTimeMs: Long,
        density: Float,
    )
}
