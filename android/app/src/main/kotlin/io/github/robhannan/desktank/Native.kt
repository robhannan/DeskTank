package io.github.robhannan.desktank

import android.view.Surface

/**
 * JNI calls into libdesktank_android.so (crates/desktank-android). The shim
 * passes the app files directory and forwards callbacks with the Dream's
 * token. Rust holds the tank state and draws on its render thread (SPEC §3.6).
 */
internal object Native {
    init {
        System.loadLibrary("desktank_android")
    }

    @JvmStatic external fun setFilesDir(path: String)

    @JvmStatic external fun surfaceCreated(dream: Long, surface: Surface)

    @JvmStatic external fun surfaceChanged(dream: Long, width: Int, height: Int)

    /** Returns once Rust has let go of the surface. */
    @JvmStatic external fun surfaceDestroyed(dream: Long)

    @JvmStatic external fun dreamingStarted(dream: Long)

    @JvmStatic external fun dreamingStopped(dream: Long)
}
