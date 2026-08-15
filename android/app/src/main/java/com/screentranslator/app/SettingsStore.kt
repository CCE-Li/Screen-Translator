package com.screentranslator.app

import android.content.Context
import com.screentranslator.app.core.RectF

/** Persists app settings in SharedPreferences. */
class SettingsStore(context: Context) {
    private val prefs = context.getSharedPreferences("screen_translator", Context.MODE_PRIVATE)

    fun saveRegion(r: RectF) {
        prefs.edit()
            .putFloat("region_x", r.x)
            .putFloat("region_y", r.y)
            .putFloat("region_w", r.width)
            .putFloat("region_h", r.height)
            .apply()
    }

    fun region(): RectF? {
        if (!prefs.contains("region_w")) return null
        return RectF(
            prefs.getFloat("region_x", 0f),
            prefs.getFloat("region_y", 0f),
            prefs.getFloat("region_w", 0f),
            prefs.getFloat("region_h", 0f),
        )
    }

    /** BCP-47-ish OCR language: "" (auto/latin), "zh", "ja". */
    var ocrLang: String
        get() = prefs.getString("ocr_lang", "")!!
        set(v) = prefs.edit().putString("ocr_lang", v).apply()

    var targetLang: String
        get() = prefs.getString("target_lang", "zh-CN")!!
        set(v) = prefs.edit().putString("target_lang", v).apply()

    /** "local" or "openai". */
    var provider: String
        get() = prefs.getString("provider", "local")!!
        set(v) = prefs.edit().putString("provider", v).apply()

    var baseUrl: String
        get() = prefs.getString("base_url", "https://api.openai.com/v1")!!
        set(v) = prefs.edit().putString("base_url", v).apply()

    var apiKey: String
        get() = prefs.getString("api_key", "")!!
        set(v) = prefs.edit().putString("api_key", v).apply()

    var model: String
        get() = prefs.getString("model", "gpt-4o-mini")!!
        set(v) = prefs.edit().putString("model", v).apply()
}
