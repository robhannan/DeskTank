package io.github.robhannan.desktank

import android.service.dreams.DreamService
import android.view.SurfaceHolder
import android.view.SurfaceView
import java.util.concurrent.atomic.AtomicLong

/**
 * The screensaver (a Dream): one full-screen [SurfaceView] whose surface and
 * the Dream's start and stop go to Rust over JNI. It holds no tank state.
 *
 * Non-interactive, so any key or touch ends it, as a screensaver should.
 * Surface and dreaming callbacks carry this instance's [token], so Rust can
 * tell a stopping Dream's late callbacks from the next Dream's.
 */
class DeskTankDream : DreamService(), SurfaceHolder.Callback {
    private val token = nextToken.incrementAndGet()

    override fun onCreate() {
        super.onCreate()
        Native.setFilesDir(filesDir.absolutePath)
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
        Native.dreamingStarted(token)
    }

    override fun onDreamingStopped() {
        Native.dreamingStopped(token)
        super.onDreamingStopped()
    }

    override fun surfaceCreated(holder: SurfaceHolder) = Native.surfaceCreated(token, holder.surface)

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
        Native.surfaceChanged(token, width, height)

    override fun surfaceDestroyed(holder: SurfaceHolder) = Native.surfaceDestroyed(token)

    private companion object {
        /** Increasing, so a later Dream always has a larger token. */
        val nextToken = AtomicLong()
    }
}
