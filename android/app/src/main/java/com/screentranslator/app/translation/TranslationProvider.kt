package com.screentranslator.app.translation

import com.screentranslator.app.core.Translation

data class TranslateRequest(
    val texts: List<String>,
    val sourceLang: String?,
    val targetLang: String,
)

/** Translates one or more texts; preserves input order. */
interface TranslationProvider {
    fun name(): String

    fun translate(request: TranslateRequest): List<Translation>
}
