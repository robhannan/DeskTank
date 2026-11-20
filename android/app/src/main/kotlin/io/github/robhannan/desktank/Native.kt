package io.github.robhannan.desktank

import android.view.Surface

/**
 * The JNI calls into libdesktank_android.so (crates/desktank-android). Each
 * forwards one platform callback; Rust holds all the state (SPEC §3.6).
 */
internal object Native {
    init {
        System.loadLibrary("desktank_android")
    }

    @JvmStatic external fun surfaceCreated(surface: Surface)

    @JvmStatic external fun surfaceChanged(width: Int, height: Int)

    @JvmStatic external fun surfaceDestroyed()

    @JvmStatic external fun dreamingStarted()

    @JvmStatic external fun dreamingStopped()
}
