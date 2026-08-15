package com.screentranslator.app

import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver

class MainActivity : ComponentActivity() {

    private val serviceLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        if (result.resultCode == RESULT_OK && result.data != null) {
            startCaptureService(result.resultCode, result.data!!)
        }
    }

    private fun requestCapture() {
        val mpm = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
        serviceLauncher.launch(mpm.createScreenCaptureIntent())
    }

    private fun startCaptureService(resultCode: Int, data: Intent) {
        val intent = Intent(this, com.screentranslator.app.service.ScreenCaptureService::class.java)
            .putExtra(com.screentranslator.app.service.ScreenCaptureService.EXTRA_RESULT_CODE, resultCode)
            .putExtra(com.screentranslator.app.service.ScreenCaptureService.EXTRA_RESULT_DATA, data)
        startForegroundService(intent)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    SettingsScreen(
                        onSelectRegion = {
                            startActivity(Intent(this@MainActivity, RegionSelectActivity::class.java))
                        },
                        onStart = { requestCapture() },
                        onStop = {
                            stopService(Intent(this@MainActivity, com.screentranslator.app.service.ScreenCaptureService::class.java))
                        },
                    )
                }
            }
        }
    }
}

@Composable
private fun SettingsScreen(
    onSelectRegion: () -> Unit,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    val context = LocalContext.current
    val settings = remember { SettingsStore(context.applicationContext) }

    var targetLang by remember { mutableStateOf(settings.targetLang) }
    var ocrLang by remember { mutableStateOf(settings.ocrLang) }
    var provider by remember { mutableStateOf(settings.provider) }
    var baseUrl by remember { mutableStateOf(settings.baseUrl) }
    var apiKey by remember { mutableStateOf(settings.apiKey) }
    var model by remember { mutableStateOf(settings.model) }
    var regionText by remember {
        mutableStateOf(settings.region()?.let { "(${it.x.toInt()}, ${it.y.toInt()}) ${it.width.toInt()}×${it.height.toInt()}" }
            ?: "未设置（默认全屏）")
    }

    val notificationPermissionLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { onStart() }

    // Refresh region label when returning from RegionSelectActivity.
    val lifecycleOwner = androidx.compose.ui.platform.LocalLifecycleOwner.current
    androidx.compose.runtime.DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                regionText = settings.region()?.let {
                    "(${it.x.toInt()}, ${it.y.toInt()}) ${it.width.toInt()}×${it.height.toInt()}"
                } ?: "未设置（默认全屏）"
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Screen Translator", style = MaterialTheme.typography.headlineSmall)

        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("翻译区域：", modifier = Modifier.weight(1f))
            Text(regionText)
        }
        Button(onClick = onSelectRegion, modifier = Modifier.fillMaxWidth()) {
            Text("选择翻译区域")
        }

        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("目标语言", modifier = Modifier.weight(1f))
            OutlinedTextField(
                value = targetLang,
                onValueChange = {
                    targetLang = it
                    settings.targetLang = it
                },
                modifier = Modifier.weight(2f),
                singleLine = true,
            )
        }

        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("OCR 语言", modifier = Modifier.weight(1f))
            val ocrOptions = listOf("auto" to "自动/Latin", "zh" to "中文", "ja" to "日本語")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for ((value, label) in ocrOptions) {
                    androidx.compose.material3.FilterChip(
                        selected = ocrLang == value,
                        onClick = {
                            ocrLang = value
                            settings.ocrLang = value
                        },
                        label = { Text(label) },
                    )
                }
            }
        }

        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("使用在线翻译", modifier = Modifier.weight(1f))
            Switch(
                checked = provider == "openai",
                onCheckedChange = {
                    provider = if (it) "openai" else "local"
                    settings.provider = provider
                },
            )
        }

        if (provider == "openai") {
            OutlinedTextField(
                value = baseUrl,
                onValueChange = { baseUrl = it; settings.baseUrl = it },
                label = { Text("Base URL") },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            OutlinedTextField(
                value = apiKey,
                onValueChange = { apiKey = it; settings.apiKey = it },
                label = { Text("API Key") },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            OutlinedTextField(
                value = model,
                onValueChange = { model = it; settings.model = it },
                label = { Text("Model") },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
        }

        Spacer(Modifier.height(8.dp))

        Button(onClick = {
            if (!Settings.canDrawOverlays(context)) {
                val intent = Intent(
                    Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                    Uri.parse("package:${context.packageName}")
                )
                context.startActivity(intent)
                return@Button
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                context.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
                android.content.pm.PackageManager.PERMISSION_GRANTED
            ) {
                notificationPermissionLauncher.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                return@Button
            }
            onStart()
        }, modifier = Modifier.fillMaxWidth()) {
            Text("开始翻译")
        }
        Button(onClick = onStop, modifier = Modifier.fillMaxWidth()) {
            Text("停止")
        }
    }
}
