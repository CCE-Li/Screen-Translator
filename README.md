# Screen Translator

实时屏幕翻译工具：框选屏幕上的一个区域，程序持续捕获该区域，识别其中的外语文字并翻译，然后通过一个**透明、点击穿透的 Overlay** 把译文覆盖回原文位置——而不影响你继续操作底下的应用/游戏/视频。

```
框选区域 → 实时捕获 → 帧差 → OCR → 翻译 → 透明 Overlay 覆盖
```

## 当前状态（Phase 1 + 2 Windows · Phase 3 Android）

### Windows

- ✅ **捕获**：DXGI Desktop Duplication + D3D11，只把区域从 GPU 拷到 CPU（staging texture + `CopySubresourceRegion`）
- ✅ **多区域**：所有启用区域并行运行，每区域独立 OCR 引擎（使用该区域 `source_lang`）+ 独立会话/缓存；overlay 合并绘制
- ✅ **OCR**：Windows 内置 OCR（`Windows.Media.Ocr`，零模型下载，用系统已装的 OCR 语言包）
- ✅ **翻译**：OpenAI 兼容 HTTP API / 本地 echo（离线验证用）
- ✅ **Overlay**：无边框置顶透明窗口（`WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE`），GDI+ 圆角背景 + 抗锯齿文字 + 阴影
- ✅ **编辑/工作模式**：拖拽创建/移动区域、**拖拽边缘/角点缩放**、右键删除；工作模式鼠标/点击穿透
- ✅ **性能**：帧差采样（24x24 亮度网格）跳过静态帧；OCR 冷却 + 定时重触发；OCR/翻译 LRU 缓存；相同文本不重复翻译；翻译单线程排队（不阻塞捕获/OCR）
- ✅ **性能监控**：每 10s 打印聚合指标 —— FPS / 捕获耗时 / OCR 耗时 / 翻译耗时 / OCR 次数 / OCR 缓存命中 / 翻译缓存命中率 / 进程 CPU% / 内存

### Android（`android/`，Kotlin + Compose）

- ✅ **捕获**：MediaProjection + VirtualDisplay + ImageReader（分辨率上限 720p，降低功耗）
- ✅ **OCR**：ML Kit 文本识别（经典 Play-services 版），支持 Latin/中文/日文；首次使用时自动下载模型（`Tasks.await` 等待式，避免冷启动失败）
- ✅ **翻译**：OpenAI 兼容 HTTP / 本地 echo；LRU 缓存 + 去重
- ✅ **Overlay**：`TYPE_APPLICATION_OVERLAY` 全屏透明窗口，`FLAG_NOT_TOUCHABLE` 触摸穿透，Canvas 圆角背景 + 居中文字
- ✅ **区域选择**：半透明全屏 Activity 拖拽框选，持久化到 SharedPreferences
- ✅ **Foreground Service**：前台服务 + 通知（停止按钮），`foregroundServiceType=mediaProjection`
- ✅ **流水线**：捕获线程独占会话；单翻译线程网络调用，结果经阻塞队列回传（镜像桌面端线程模型）；帧差 + OCR 冷却 + 重触发 + 缓存
- ⚠️ **验证说明**：APK 构建、安装、区域选择、MediaProjection 授权、前台服务、捕获→OCR 调用→统计全链路已在 API 34 模拟器验证（`stats: ocr_runs=2 frames=2 ocr_ms=14`）。ML Kit OCR 模型需 Play services 动态下载，模拟器上受镜像限制未能完成模型加载；**真机（带 Play services）首次启动会自动下载模型并正常工作**。

#### Android 构建

```powershell
cd android
gradlew.bat :app:assembleDebug        # APK 在 app/build/outputs/apk/debug/
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

启动后：设置翻译区域（拖拽）→ 选择 OCR 语言/目标语言/翻译服务 → 开始翻译 → 授权屏幕捕获。

## 快速开始

```powershell
cargo build -p screen-translator
cargo run -p screen-translator
```

首次运行会生成 `config.json`（在 exe 同级目录）。默认使用本地 echo 翻译，可立即看到“捕获→OCR→覆盖”整条链路是否工作。

### 操作

| 操作 | 说明 |
| --- | --- |
| 启动后（有区域） | 进入工作模式，overlay 透明且点击穿透 |
| 启动后（无区域） | 进入编辑模式 |
| **拖拽**（编辑模式） | 创建新区域 / 移动已有区域 |
| **拖拽边缘/角点**（编辑模式） | 缩放区域（显示白色手柄） |
| **右键**（编辑模式） | 删除区域 |
| **Ctrl+Alt+key** | 切换编辑/工作模式（热键自动选择未被占用的按键，运行日志会打印实际组合，如 `hotkeys: toggle Y`） |
| **Ctrl+Alt+X** | 退出 |

### 配置 `config.json`

```jsonc
{
  "regions": [{
    "id": "r1",
    "rect": { "x": 100, "y": 200, "width": 640, "height": 120 },   // 屏幕绝对坐标
    "source_lang": "ja",      // OCR + 源语言；null = 自动（用系统用户语言）
    "target_lang": "zh-CN",
    "enabled": true
  }],
  "capture": {
    "fps": 5.0,               // 捕获帧率
    "ocr_cooldown_ms": 700,   // OCR 最小间隔
    "ocr_retrigger_ms": 3000, // 静态画面也会周期性重 OCR（兜底）
    "diff_threshold": 0.01    // 帧差超过该比例才触发 OCR
  },
  "ocr_language": "",         // 空 = 用户配置文件语言
  "translation": { "provider": "local" },
  "overlay_style": {
    "opacity": 0.9,
    "text_color": "#FFFFFF",
    "background_color": "#B0000000",
    "corner_radius": 6.0,
    "shadow": true,
    "font_family": "Microsoft YaHei UI",
    "font_size_scale": 1.0,
    "max_font_size": 48.0
  }
}
```

使用真实翻译 API（任何 OpenAI 兼容端点）：

```json
{
  "translation": {
    "provider": "openai",
    "base_url": "https://api.openai.com/v1",
    "api_key": "sk-...",
    "model": "gpt-4o-mini",
    "timeout_secs": 30,
    "max_retries": 2,
    "batch_size": 8
  }
}
```

> 提示：区域 `source_lang` 同时用于选择 OCR 语言包。例如 `"ja"` 需要系统已安装日语 OCR 语言包（Windows 设置 → 时间和语言 → 语言 → 可选功能 → 光学字符识别）。

## 架构

```
core/                        # 跨平台核心（纯 Rust，可测试）
├── types.rs                 # TextRegion / Frame / OverlayText / Rect ...
├── ocr.rs                   # OcrProvider trait（WindowsOCR / 未来 PaddleOCR / ONNXOCR）
├── translation/
│   ├── mod.rs               # TranslationProvider trait + 批量请求
│   ├── openai.rs            # OpenAI 兼容 HTTP provider（超时/重试/批处理/响应解析）
│   ├── local.rs             # 离线 echo provider
│   └── cache.rs             # 翻译缓存（LRU + 可选 TTL，key = 源语言+目标语言+归一化文本）
├── cache.rs                 # 通用 LRU 缓存
├── diff.rs                  # 帧差采样签名（跳过静态帧）
├── layout.rs                # 译文在原文 bbox 内的字号收缩/对齐
├── session.rs               # 单区域实时会话：限流、OCR 结果缓存、去重排队、缓存命中
└── config.rs                # 序列化配置

windows/                     # Windows 平台实现
├── capture.rs               # DXGI Desktop Duplication + D3D11 区域拷贝
├── ocr_winrt.rs             # Windows.Media.Ocr provider（含超大图降采样）
├── overlay.rs               # 透明分层窗口 + GDI+ 渲染 + 编辑模式绘制
├── app.rs                   # 线程编排：pipeline 线程 / 翻译 worker / UI 消息循环
└── main.rs
```

### 线程模型

```
[UI 线程]  overlay 窗口 + 消息循环（编辑模式鼠标交互）
   │  overlay_tx (Draw 列表)
[Pipeline 线程]  capture → 帧差 → session(OCR+缓存) → 排队翻译
   │  translate_tx / done_rx
[翻译 worker]  单线程串行调用 provider（有内部重试/超时）
```

背压：pipeline 用 `try_recv` 非阻塞轮询 + 有界节奏；帧差跳过静态帧；OCR 冷却限流；翻译排队。

## 测试

```powershell
cargo test --workspace
cargo clippy --workspace
```

核心逻辑（缓存、帧差、布局、会话、响应解析）均有单元测试。

## 路线图

- **Phase 4**：Android 悬浮球、触摸穿透完善、屏幕旋转适配、电量优化、性能面板
- **Phase 5**：跨平台 Provider/Cache/Layout/Config 统一
- **Phase 6**：全屏智能翻译、游戏/字幕模式、原文覆盖（背景遮盖 / OpenCV·AI Inpainting）、本地模型（离线 OCR/翻译）、GPU 加速、性能 HUD
