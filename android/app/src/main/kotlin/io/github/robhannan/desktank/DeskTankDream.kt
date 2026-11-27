package io.github.robhannan.desktank

import android.service.dreams.DreamService
import android.view.SurfaceHolder
import android.view.SurfaceView

/**
 * The screensaver (a Dream): one full-screen [SurfaceView] whose surface and
 * the Dream's start and stop go to Rust over JNI. It holds no tank state.
 *
 * Non-interactive, so any key or touch ends it, as a screensaver should.
 * Surface and dreaming callbacks carry this instance's [token], so Rust can
 * tell a stopping Dream's late callbacks from those of the next host.
 */
class DeskTankDream : DreamService(), SurfaceHolder.Callback {
    private val token = Native.nextToken()

    override fun onCreate() {
        super.onCreate()
        Native.initialize(this)
        Native.applyOled(this, token)
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        isInteractive = false
        isFullscreen = true
        val view = SurfaceView(this)
        view.holder.addCallback(this)
        setContentView(view)
    }

    override fun onDreamingStarted() {
        super.onDreamingStarted()
        val prefs = getSharedPreferences(PREFS, MODE_PRIVATE)
        Native.started(
            token,
            app = false,
            tour = prefs.getBoolean(KEY_TOUR, true),
            tourCard = prefs.getBoolean(KEY_TOUR_CARD, false),
        )
    }

    override fun onDreamingStopped() {
        Native.stopped(token)
        super.onDreamingStopped()
    }

    override fun surfaceCreated(holder: SurfaceHolder) = Native.surfaceCreated(token, holder.surface)

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
        Native.surfaceChanged(token, width, height)

    override fun surfaceDestroyed(holder: SurfaceHolder) = Native.surfaceDestroyed(token)

    private companion object {
        // The camera tour's settings (SPEC §3.6, D29); A3's settings screen
        // edits them. The tour is on by default and its card off.
        const val PREFS = "dream"
        const val KEY_TOUR = "tour"
        const val KEY_TOUR_CARD = "tour_card"
    }
}
