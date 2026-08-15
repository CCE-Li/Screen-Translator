package com.screentranslator.app.translation

import com.screentranslator.app.core.Translation

/** Offline echo provider: verifies the pipeline without network. */
class LocalTranslationProvider : TranslationProvider {
    override fun name(): String = "local"

    override fun translate(request: TranslateRequest): List<Translation> =
        request.texts.map {
            Translation(
                translatedText = it.trim(),
                detectedSourceLang = request.sourceLang,
                targetLang = request.targetLang,
            )
        }
}
