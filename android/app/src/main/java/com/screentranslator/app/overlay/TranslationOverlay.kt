package com.screentranslator.app.overlay

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.view.View
import android.view.WindowManager
import com.screentranslator.app.core.OverlayText

/**
 * Draws the translation text on top of the original content. The window is
 * FLAG_NOT_TOUCHABLE so touches pass through to the app underneath.
 */
class TranslationView(context: Context) : View(context) {
    private var overlays: List<OverlayText> = emptyList()
    private val bgPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xB0000000.toInt()
    }
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFFFFFFFF.toInt()
    }
    private val shadowPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0x80000000.toInt()
    }

    fun setOverlays(list: List<OverlayText>) {
        overlays = list
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        for (o in overlays) {
            val pad = 4f
            val bg = RectF(o.box.x - pad, o.box.y - pad, o.box.x + o.box.width + pad, o.box.y + o.box.height + pad)
            canvas.drawRoundRect(bg, 6f, 6f, bgPaint)

            textPaint.textSize = o.fontSize
            // Center horizontally within the box; clip to keep inside the box.
            canvas.save()
            canvas.clipRect(bg)
            val fm = textPaint.fontMetrics
            val baseline = bg.centerY() - (fm.ascent + fm.descent) / 2f
            if (o.text.isNotEmpty()) {
                canvas.drawText(o.text, bg.centerX(), baseline + 1.5f, shadowPaint)
                canvas.drawText(o.text, bg.centerX(), baseline, textPaint)
            }
            canvas.restore()
        }
    }
}

/**
 * Owns the full-screen overlay window. `show` (re)creates the window on first
 * call, then updates content. All coordinates are screen coordinates.
 */
class TranslationOverlay(private val context: Context) {
    private val windowManager = context.getSystemService(Context.WINDOW_SERVICE) as WindowManager
    private var view: TranslationView? = null

    fun show(overlays: List<OverlayText>) {
        if (view == null) {
            val type = if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
            } else {
                @Suppress("DEPRECATION")
                WindowManager.LayoutParams.TYPE_PHONE
            }
            val params = WindowManager.LayoutParams(
                WindowManager.LayoutParams.MATCH_PARENT,
                WindowManager.LayoutParams.MATCH_PARENT,
                type,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                    WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                    WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or
                    WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                    WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
                android.graphics.PixelFormat.TRANSLUCENT,
            )
            val v = TranslationView(context).apply { setOverlays(overlays) }
            view = v
            try {
                windowManager.addView(v, params)
            } catch (e: Exception) {
                android.util.Log.e("TranslationOverlay", "addView failed: ${e.message}")
                view = null
            }
        } else {
            view?.setOverlays(overlays)
        }
    }

    fun dismiss() {
        view?.let { v ->
            try {
                windowManager.removeView(v)
            } catch (_: Exception) {
            }
        }
        view = null
    }
}
