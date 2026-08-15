package com.screentranslator.app.translation

import com.screentranslator.app.core.Translation
import com.screentranslator.app.core.TranslationCache
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * OpenAI-compatible chat-completions translation provider. Batch texts are
 * sent with [[index]] markers so a single HTTP call returns all translations.
 */
class OpenAiTranslationProvider(
    baseUrl: String,
    apiKey: String,
    private val model: String,
    timeoutSecs: Long = 30,
    private val batchSize: Int = 8,
) : TranslationProvider {

    private val url = baseUrl.trimEnd('/') + "/chat/completions"
    private val client = OkHttpClient.Builder()
        .connectTimeout(timeoutSecs, TimeUnit.SECONDS)
        .readTimeout(timeoutSecs, TimeUnit.SECONDS)
        .build()
    private val authHeader: String? = apiKey.takeIf { it.isNotEmpty() }?.let { "Bearer $it" }

    override fun name(): String = "openai-compatible"

    override fun translate(request: TranslateRequest): List<Translation> {
        if (request.texts.isEmpty()) return emptyList()
        require(request.texts.size <= batchSize) { "batch too large: ${request.texts.size} > $batchSize" }

        val src = request.sourceLang ?: "auto-detect"
        val system =
            "You are a translation engine. Translate the user's text from $src to ${request.targetLang}. " +
                "Return ONLY the translation(s), no explanations. If the user sent multiple items " +
                "marked as [[index]]text, reply with each translation on its own line prefixed " +
                "with the same [[index]] marker, preserving order."
        val user = request.texts.mapIndexed { i, t -> "[[$i]]$t" }.joinToString("\n")

        val body = JSONObject().apply {
            put("model", model)
            put("temperature", 0.1)
            put("stream", false)
            put(
                "messages",
                JSONArray().apply {
                    put(JSONObject().put("role", "system").put("content", system))
                    put(JSONObject().put("role", "user").put("content", user))
                },
            )
        }

        val rb = Request.Builder().url(url).post(
            body.toString().toRequestBody("application/json; charset=utf-8".toMediaType())
        )
        authHeader?.let { rb.addHeader("Authorization", it) }

        client.newCall(rb.build()).execute().use { resp ->
            if (!resp.isSuccessful) throw RuntimeException("HTTP ${resp.code}")
            val content = try {
                val json = JSONObject(resp.body?.string() ?: "")
                json.getJSONArray("choices").getJSONObject(0)
                    .getJSONObject("message").getString("content")
            } catch (e: Exception) {
                throw RuntimeException("unparseable response: ${e.message}", e)
            }
            return parseResponse(content, request)
        }
    }

    private fun parseResponse(content: String, request: TranslateRequest): List<Translation> {
        val n = request.texts.size
        val out = arrayOfNulls<String>(n)
        val trimmed = content.trim()

        // 1) plain JSON array of strings
        try {
            val arr = JSONArray(trimmed)
            if (arr.length() == n) {
                var allStrings = true
                for (i in 0 until n) if (arr.optString(i).isEmpty()) allStrings = false
                if (allStrings) {
                    for (i in 0 until n) out[i] = arr.getString(i)
                    return finish(out, request)
                }
            }
        } catch (_: Exception) {
            // fall through to marker parse
        }

        // 2) [[index]] markers
        for (i in request.texts.indices) {
            val marker = "[[$i]]"
            val idx = trimmed.indexOf(marker)
            if (idx >= 0) {
                val rest = trimmed.substring(idx + marker.length)
                val end = rest.indexOf("[[")
                val seg = (if (end >= 0) rest.substring(0, end) else rest).trim()
                out[i] = if (seg.isNotEmpty()) seg else request.texts[i]
            }
        }

        // 3) one plain line per item
        if (out.all { it == null }) {
            val lines = trimmed.lines().map { it.trim() }.filter { it.isNotEmpty() }
            if (lines.size == n) for (i in 0 until n) out[i] = lines[i]
        }

        return finish(out, request)
    }

    private fun finish(out: Array<String?>, request: TranslateRequest): List<Translation> =
        request.texts.mapIndexed { i, src ->
            val t = out[i]
            Translation(
                translatedText = if (t.isNullOrEmpty()) src else t,
                detectedSourceLang = request.sourceLang,
                targetLang = request.targetLang,
            )
        }

    companion object {
        /** Normalize text exactly like the core cache so keys line up. */
        fun normalize(text: String): String = TranslationCache.normalize(text)
    }
}
