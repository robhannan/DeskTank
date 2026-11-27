package io.github.robhannan.desktank

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController

/** The launcher host for the shared tank. Rust handles all touch gestures. */
class DeskTankActivity : Activity(), SurfaceHolder.Callback {
    private val token = Native.nextToken()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Native.initialize(this)
        Native.applyOled(this, token, intent)
        val view = SurfaceView(this)
        view.holder.addCallback(this)
        // Give the callback coordinates relative to the surface view.
        view.setOnTouchListener { _, event -> handleTouch(event) }
        setContentView(view)
        enterImmersiveMode()
    }

    /** `singleTask`: a later `am start --ez oled ...` arrives here, not in onCreate. */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        Native.applyOled(this, token, intent)
    }

    override fun onResume() {
        super.onResume()
        Native.started(token, app = true)
    }

    override fun onPause() {
        Native.stopped(token)
        super.onPause()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) enterImmersiveMode()
    }

    @Suppress("DEPRECATION") // systemUiVisibility on API 26–29
    private fun enterImmersiveMode() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
            window.decorView.windowInsetsController?.let {
                it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                it.hide(WindowInsets.Type.systemBars())
            }
        } else {
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
        }
    }

    override fun surfaceCreated(holder: SurfaceHolder) = Native.surfaceCreated(token, holder.surface)

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
        Native.surfaceChanged(token, width, height)

    override fun surfaceDestroyed(holder: SurfaceHolder) = Native.surfaceDestroyed(token)

    /** Forwards the view's touches; others go unhandled, so none reach Rust twice. */
    private fun handleTouch(event: MotionEvent): Boolean {
        val density = resources.displayMetrics.density
        fun forward(action: Int, index: Int) {
            Native.touch(
                token, action, event.getPointerId(index), event.getX(index), event.getY(index),
                event.eventTime, density,
            )
        }
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN ->
                forward(MotionEvent.ACTION_DOWN, event.actionIndex)
            MotionEvent.ACTION_MOVE ->
                for (index in 0 until event.pointerCount) forward(MotionEvent.ACTION_MOVE, index)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP ->
                forward(MotionEvent.ACTION_UP, event.actionIndex)
            MotionEvent.ACTION_CANCEL ->
                Native.touch(token, MotionEvent.ACTION_CANCEL, -1, 0f, 0f, event.eventTime, density)
            else -> return false
        }
        return true
    }
}
