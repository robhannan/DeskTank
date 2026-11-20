package io.github.robhannan.desktank

import android.service.dreams.DreamService
import android.view.SurfaceHolder
import android.view.SurfaceView

/**
 * The screensaver (a Dream): one full-screen [SurfaceView] whose surface and
 * the Dream's start and stop go to Rust over JNI. It holds no tank state.
 *
 * Non-interactive, so any key or touch ends it, as a screensaver should.
 */
class DeskTankDream : DreamService(), SurfaceHolder.Callback {
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
        Native.dreamingStarted()
    }

    override fun onDreamingStopped() {
        Native.dreamingStopped()
        super.onDreamingStopped()
    }

    override fun surfaceCreated(holder: SurfaceHolder) = Native.surfaceCreated(holder.surface)

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
        Native.surfaceChanged(width, height)

    override fun surfaceDestroyed(holder: SurfaceHolder) = Native.surfaceDestroyed()
}
