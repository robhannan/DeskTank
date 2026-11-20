package io.github.robhannan.desktank

import android.graphics.PixelFormat
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
        // A SurfaceView's buffers default to RGB 565, which wgpu's GL backend
        // (EGL) keeps, banding the water; Vulkan sets its own format.
        view.holder.setFormat(PixelFormat.RGBA_8888)
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
