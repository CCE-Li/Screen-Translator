//! Developer debug panel (egui). A separate window on its own thread, fed by
//! shared state updated from the pipeline (stats, logs) and sending commands
//! (start/stop pipeline, apply config) back to the UI thread.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;

use screen_translator_core::config::{AppConfig, RegionConfig, TranslationConfig};
use screen_translator_core::types::Rect;

use crate::overlay::StatsSnapshot;

/// Commands from the debug panel, drained by the main UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCommand {
    StartPipeline,
    StopPipeline,
    ToggleMode,
    /// Push `shared.config` into the app and save to disk.
    ApplyConfig,
    /// Reload the app config from disk into `shared.config`.
    ReloadConfig,
}

/// State shared between the debug panel thread and the app.
pub struct DebugShared {
    pub stats: Option<StatsSnapshot>,
    pub log_tail: VecDeque<String>,
    pub config: AppConfig,
    pub config_path: String,
    pub pipeline_running: bool,
    pub mode_work: bool,
    pub commands: Vec<DebugCommand>,
}

impl DebugShared {
    pub fn new(config: AppConfig, config_path: PathBuf) -> Self {
        Self {
            stats: None,
            log_tail: VecDeque::new(),
            config,
            config_path: config_path.to_string_lossy().into_owned(),
            pipeline_running: false,
            mode_work: false,
            commands: Vec::new(),
        }
    }

    pub fn push_log(&mut self, line: String) {
        self.log_tail.push_back(line);
        while self.log_tail.len() > 600 {
            self.log_tail.pop_front();
        }
    }

    pub fn push_command(&mut self, cmd: DebugCommand) {
        self.commands.push(cmd);
    }
}

/// A `log` logger that mirrors messages to stderr and into the shared log so
/// the debug panel shows the same output as the console.
pub struct SharedLog {
    shared: Arc<Mutex<DebugShared>>,
}

impl SharedLog {
    pub fn new(shared: Arc<Mutex<DebugShared>>) -> Self {
        Self { shared }
    }
}

impl log::Log for SharedLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{}: {}", record.level(), record.args());
        eprintln!("{line}");
        if let Ok(mut s) = self.shared.lock() {
            s.push_log(line);
        }
    }

    fn flush(&self) {}
}

/// eframe application driving the debug panel.
pub struct DebugApp {
    shared: Arc<Mutex<DebugShared>>,
}

impl DebugApp {
    pub fn new(shared: Arc<Mutex<DebugShared>>) -> Self {
        Self { shared }
    }
}

impl eframe::App for DebugApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut shared = self.shared.lock().unwrap();
        let mut actions: Vec<DebugCommand> = Vec::new();
        let mut save = false;

        egui::Panel::top("status").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mode = if shared.mode_work { "工作模式" } else { "编辑模式" };
                let running = if shared.pipeline_running { "pipeline 运行中" } else { "pipeline 已停止" };
                ui.heading("Screen Translator");
                ui.separator();
                ui.label(format!("{mode} · {running}"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("退出").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("保存并应用").clicked() {
                        actions.push(DebugCommand::ApplyConfig);
                        save = true;
                    }
                    if ui.button("重新载入").clicked() {
                        actions.push(DebugCommand::ReloadConfig);
                    }
                    if ui.button("工作⇄编辑").clicked() {
                        actions.push(DebugCommand::ToggleMode);
                    }
                    if ui.button("停止").clicked() {
                        actions.push(DebugCommand::StopPipeline);
                    }
                    if ui.button("开始").clicked() {
                        actions.push(DebugCommand::StartPipeline);
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("log")
            .resizable(true)
            .show(ui, |ui| {
                ui.set_min_height(120.0);
                ui.label("日志");
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in &shared.log_tail {
                            ui.monospace(line);
                        }
                    });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.heading("实时统计");
                match &shared.stats {
                    Some(s) => {
                        egui::Grid::new("stats").num_columns(4).spacing([24.0, 6.0]).show(ui, |ui| {
                            ui.label(format!("fps {:.1} / {:.0}", s.fps, s.target_fps));
                            ui.label(format!("capture {:.0}ms", s.capture_ms));
                            ui.label(format!("ocr {:.0}ms", s.ocr_ms));
                            ui.label(format!("translate {:.0}ms", s.translate_ms));
                            ui.end_row();
                            ui.label(format!("ocr_runs {}", s.ocr_runs));
                            ui.label(format!("pending {}", s.pending));
                            ui.label(format!("tc_hit {:.0}%", s.cache_hit * 100.0));
                            ui.label(format!("cpu {:.0}% · mem {:.0}MB", s.cpu, s.mem_mb));
                            ui.end_row();
                        });
                    }
                    None => {
                        ui.label("（等待流水线运行）");
                    }
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("翻译区域");

                let mut remove: Option<usize> = None;
                let mut regions = shared.config.regions.clone();
                for (i, r) in regions.iter_mut().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut r.enabled, &r.id);
                            ui.label("x");
                            ui.add(egui::DragValue::new(&mut r.rect.x).speed(1.0));
                            ui.label("y");
                            ui.add(egui::DragValue::new(&mut r.rect.y).speed(1.0));
                            ui.label("w");
                            ui.add(egui::DragValue::new(&mut r.rect.width).speed(1.0));
                            ui.label("h");
                            ui.add(egui::DragValue::new(&mut r.rect.height).speed(1.0));
                            if ui.button("删除").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("源语言");
                            let src = r.source_lang.get_or_insert_with(String::new);
                            ui.add(egui::TextEdit::singleline(src).desired_width(60.0));
                            ui.label("目标语言");
                            ui.add(egui::TextEdit::singleline(&mut r.target_lang).desired_width(70.0));
                        });
                    });
                }
                if let Some(i) = remove {
                    regions.remove(i);
                }
                shared.config.regions = regions;
                if ui.button("+ 添加区域").clicked() {
                    let default_target = shared
                        .config
                        .regions
                        .first()
                        .map(|r| r.target_lang.clone())
                        .unwrap_or_else(|| "zh-CN".into());
                    let next_id = format!("r{}", shared.config.regions.len() + 1);
                    shared.config.regions.push(RegionConfig {
                        id: next_id,
                        rect: Rect::new(100.0, 100.0, 400.0, 100.0),
                        source_lang: None,
                        target_lang: default_target,
                        enabled: true,
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("翻译");
                let mut provider = shared.config.translation.clone();
                ui.horizontal(|ui| {
                    ui.label("provider");
                    let selected = match &provider {
                        TranslationConfig::Local => "local (echo)",
                        TranslationConfig::OpenAi(_) => "openai",
                    };
                    egui::ComboBox::from_id_salt("provider")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut provider, TranslationConfig::Local, "local (echo)");
                            let openai = match &shared.config.translation {
                                TranslationConfig::OpenAi(c) => TranslationConfig::OpenAi(c.clone()),
                                TranslationConfig::Local => TranslationConfig::OpenAi(Default::default()),
                            };
                            ui.selectable_value(&mut provider, openai, "openai");
                        });
                });
                if let TranslationConfig::OpenAi(c) = &mut provider {
                    ui.horizontal(|ui| {
                        ui.label("base_url");
                        ui.add(egui::TextEdit::singleline(&mut c.base_url).desired_width(280.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("api_key");
                        ui.add(egui::TextEdit::singleline(&mut c.api_key).desired_width(240.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("model");
                        ui.add(egui::TextEdit::singleline(&mut c.model).desired_width(200.0));
                        ui.label("batch");
                        ui.add(egui::DragValue::new(&mut c.batch_size).range(1..=16));
                    });
                }
                shared.config.translation = provider;

                ui.add_space(8.0);
                ui.separator();
                ui.heading("捕获 / OCR / 外观");
                egui::Grid::new("settings").num_columns(4).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("capture.fps");
                    ui.add(egui::DragValue::new(&mut shared.config.capture.fps).speed(0.2).range(0.5..=30.0));
                    ui.label("ocr_cooldown_ms");
                    ui.add(egui::DragValue::new(&mut shared.config.capture.ocr_cooldown_ms).speed(50.0));
                    ui.end_row();
                    ui.label("ocr_language");
                    ui.add(egui::TextEdit::singleline(&mut shared.config.ocr_language).desired_width(70.0));
                    ui.label("diff_threshold");
                    ui.add(egui::DragValue::new(&mut shared.config.capture.diff_threshold).speed(0.001).range(0.0..=0.5));
                    ui.end_row();
                    ui.label("overlay.opacity");
                    ui.add(egui::DragValue::new(&mut shared.config.overlay_style.opacity).speed(0.01).range(0.0..=1.0));
                    ui.label("bg_opacity");
                    ui.add(egui::DragValue::new(&mut shared.config.overlay_style.background_opacity).speed(0.01).range(0.0..=1.0));
                    ui.end_row();
                    ui.label("bg_color");
                    ui.add(egui::TextEdit::singleline(&mut shared.config.overlay_style.background_color).desired_width(90.0));
                    ui.label("text_color");
                    ui.add(egui::TextEdit::singleline(&mut shared.config.overlay_style.text_color).desired_width(90.0));
                    ui.end_row();
                });
                ui.checkbox(&mut shared.config.show_stats, "show_stats (性能 HUD)");
                ui.checkbox(&mut shared.config.overlay_style.shadow, "text shadow");

                ui.add_space(8.0);
                ui.label(egui::RichText::new(format!("config: {}", shared.config_path)).small().weak());
            });
        });

        if save {
            if let Err(e) = shared.config.save(&std::path::PathBuf::from(&shared.config_path)) {
                shared.push_log(format!("保存失败: {e}"));
            }
        }
        for a in actions {
            shared.push_command(a);
        }
        drop(shared);

        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Spawn the debug panel on a dedicated thread.
pub fn spawn_debug_panel(shared: Arc<Mutex<DebugShared>>) {
    std::thread::Builder::new()
        .name("debug-gui".into())
        .spawn(move || {
            let options = eframe::NativeOptions {
                viewport: egui::ViewportBuilder::default()
                    .with_inner_size([600.0, 760.0])
                    .with_title("Screen Translator — Debug"),
                // The panel runs on a worker thread; winit requires opting in.
                event_loop_builder: Some(Box::new(|builder| {
                    use winit::platform::windows::EventLoopBuilderExtWindows as _;
                    builder.with_any_thread(true);
                })),
                ..Default::default()
            };
            let result = eframe::run_native(
                "screen-translator-debug",
                options,
                Box::new(move |_cc| -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>> {
                    Ok(Box::new(DebugApp::new(shared)))
                }),
            );
            if let Err(e) = result {
                eprintln!("debug panel failed to start: {e}");
            }
        })
        .expect("spawn debug-gui thread");
}
