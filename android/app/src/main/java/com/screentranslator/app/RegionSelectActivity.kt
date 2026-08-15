package com.screentranslator.app

import android.app.Activity
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.os.Bundle
import android.view.MotionEvent
import android.view.View
import android.widget.FrameLayout
import android.widget.TextView
import com.screentranslator.app.core.RectF
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

/**
 * Translucent full-screen activity where the user drags a rectangle to select
 * the translation region. The selection is persisted to [SettingsStore].
 */
class RegionSelectActivity : Activity() {

    private lateinit var settings: SettingsStore
    private var selection: RectF? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        settings = SettingsStore(this)
        val overlay = SelectView(this) { rect ->
            settings.saveRegion(rect)
            finish()
        }
        val hint = TextView(this).apply {
            text = "按住拖动选择翻译区域，松开确认"
            setTextColor(Color.WHITE)
            textSize = 16f
        }
        val root = FrameLayout(this)
        root.setBackgroundColor(Color.TRANSPARENT)
        root.addView(overlay, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT))
        root.addView(hint, FrameLayout.LayoutParams(FrameLayout.LayoutParams.WRAP_CONTENT, FrameLayout.LayoutParams.WRAP_CONTENT).apply {
            gravity = android.view.Gravity.TOP or android.view.Gravity.CENTER_HORIZONTAL
            topMargin = (40 * resources.displayMetrics.density).toInt()
        })
        setContentView(root)

        // Dim the background slightly so the selected rectangle is visible.
        val dm = resources.displayMetrics
        root.setBackgroundColor(0x66000000)
    }

    /** Custom view: dims screen, draws the drag rectangle. */
    private inner class SelectView(
        context: Context,
        private val onConfirm: (RectF) -> Unit,
    ) : View(context) {

        private val fillPaint = Paint().apply { color = 0x40FFFFFF.toInt() }
        private val borderPaint = Paint().apply {
            color = Color.YELLOW
            style = Paint.Style.STROKE
            strokeWidth = 2f * resources.displayMetrics.density
        }
        private var start: android.graphics.Point? = null
        private var current: android.graphics.Point? = null

        override fun onTouchEvent(event: MotionEvent): Boolean {
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    start = android.graphics.Point(event.x.toInt(), event.y.toInt())
                    current = start
                }
                MotionEvent.ACTION_MOVE -> {
                    current = android.graphics.Point(event.x.toInt(), event.y.toInt())
                    invalidate()
                }
                MotionEvent.ACTION_UP -> {
                    val s = start
                    val c = current
                    if (s != null && c != null) {
                        val rect = com.screentranslator.app.core.RectF(
                            min(s.x, c.x).toFloat(),
                            min(s.y, c.y).toFloat(),
                            abs(c.x - s.x).toFloat(),
                            abs(c.y - s.y).toFloat(),
                        )
                        if (rect.width >= 20 && rect.height >= 12) {
                            onConfirm(rect)
                        } else {
                            start = null
                            current = null
                            invalidate()
                        }
                    } else {
                        start = null
                        current = null
                    }
                }
            }
            return true
        }

        override fun onDraw(canvas: Canvas) {
            super.onDraw(canvas)
            val s = start
            val c = current
            if (s == null || c == null) return
            val r = android.graphics.RectF(
                min(s.x, c.x).toFloat(),
                min(s.y, c.y).toFloat(),
                max(s.x, c.x).toFloat(),
                max(s.y, c.y).toFloat(),
            )
            canvas.drawRect(r, fillPaint)
            canvas.drawRect(r, borderPaint)
        }
    }
}
