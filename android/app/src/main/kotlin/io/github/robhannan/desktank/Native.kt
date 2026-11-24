package io.github.robhannan.desktank

import android.content.Context
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
    fun initialize(context: Context) {
        if (!initialized) {
            val television = context.packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK)
            init(context.filesDir.absolutePath, television, SystemClock.uptimeMillis())
            initialized = true
        }
    }

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
