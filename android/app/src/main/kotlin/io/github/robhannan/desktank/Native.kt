package io.github.robhannan.desktank

import android.view.Surface

/**
 * The JNI calls into libdesktank_android.so (crates/desktank-android). Each
 * forwards one platform callback, tagged with the calling Dream's token;
 * Rust holds all the state and draws on its own render thread (SPEC §3.6).
 */
internal object Native {
    init {
        System.loadLibrary("desktank_android")
    }

    @JvmStatic external fun surfaceCreated(dream: Long, surface: Surface)

    @JvmStatic external fun surfaceChanged(dream: Long, width: Int, height: Int)

    /** Returns once Rust has let go of the surface. */
    @JvmStatic external fun surfaceDestroyed(dream: Long)

    @JvmStatic external fun dreamingStarted(dream: Long)

    @JvmStatic external fun dreamingStopped(dream: Long)
}
