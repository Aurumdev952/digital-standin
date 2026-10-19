use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, anyhow};
use brain::Settings;
use eframe::egui::{self, Color32, ColorImage, RichText, Stroke, TextureHandle, TextureOptions, pos2, vec2};

use crate::live::{BrainIn, Live, Options};
use crate::telemetry::{EventKind, HISTORY_SECS, Series, Telemetry};
use crate::theme::{self, AMBER, CORAL, CYAN, ICE, LIME, MAGENTA, PALETTE};

pub fn run(opts: Options, settings: Settings, config_path: PathBuf) -> Result<()> {
    let live = Live::start(opts)?;
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Digital Standin").with_inner_size([1700.0, 1010.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Digital Standin",
        native,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            let aliases = settings.brain.aliases.join(", ");
            Ok(Box::new(Panel {
                live,
                saved: settings.clone(),
                settings,
                config_path,
                preview: None,
                seq: 0,
                paused: false,
                show_decisions: false,
                status: String::new(),
                aliases,
            }))
        }),
    )
    .map_err(|e| anyhow!("GUI failed: {e}"))
}

struct Panel {
    live: Live,
    settings: Settings,
    saved: Settings,
    config_path: PathBuf,
    preview: Option<TextureHandle>,
    seq: u64,
    paused: bool,
    show_decisions: bool,
    status: String,
    aliases: String,
}

impl Panel {
    fn push_settings(&self) {
        let _ = self.live.tx.send(BrainIn::Config(self.settings.brain.clone()));
        let _ = self.live.tx.send(BrainIn::BlendFrames(self.settings.video.blend_frames));
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        let p = self.live.engine.preview.lock();
        if p.seq == self.seq || p.rgba.is_empty() {
            return;
        }
        self.seq = p.seq;
        let img = ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
        match &mut self.preview {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.preview = Some(ctx.load_texture("preview", img, TextureOptions::LINEAR)),
        }
    }
}

impl eframe::App for Panel {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.live.shutdown();
        // Model threads are mid-inference; tearing the runtime down under them only logs errors.
        std::process::exit(0);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(66));
        theme::backdrop(ui.ctx());
        if let Some(msg) = self.live.sidecar_exited() {
            let mut t = self.live.telemetry.lock();
            let now = t.now;
            t.decider = msg.clone();
            t.event(now, EventKind::Info, msg);
        }
        self.refresh_preview(ui.ctx());
        let stats = *self.live.engine.stats.lock();
        let lib = self.live.lib.clone();
        let t = self.live.telemetry.lock();
        let time = ui.input(|i| i.time);

        egui::Panel::top("status").frame(theme::panel_frame()).show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::title(ui, "DIGITAL::STANDIN");
                ui.add_space(12.0);
                let state = stats.status.map(|s| lib.states[s.state].name.as_str()).unwrap_or("-");
                let next = stats.status.and_then(|s| s.pending).map(|p| format!(" > {}", lib.states[p].name)).unwrap_or_default();
                theme::lcd(ui, &format!("NOW {}{next}", state.to_uppercase()));
                theme::lcd(ui, &format!("CAM {} FR  {} LATE", stats.frames, stats.late));
                let share = t.asr_share().map(|s| format!("  ON {:.0}%", s * 100.0)).unwrap_or_default();
                theme::lcd(ui, &format!("ASR {:.0}MS{share}", t.asr_ms));
                let decider = if t.decider.is_empty() { "STARTING".to_string() } else { t.decider.to_uppercase() };
                theme::lcd(ui, &format!("AI {decider}"));
                if self.paused {
                    ui.label(RichText::new("■ HOLDING IDLE").monospace().strong().color(MAGENTA));
                }
            });
        });

        egui::Panel::left("camera").exact_size(690.0).frame(theme::panel_frame()).show(ui, |ui| {
            theme::bevel().show(ui, |ui| {
                theme::heading(ui, "camera out");
                if let Some(tex) = &self.preview {
                    let r = ui.image((tex.id(), vec2(640.0, 360.0))).rect;
                    theme::crt(ui, r, time);
                }
                theme::spacer(ui, 4.0);
                ui.horizontal_wrapped(|ui| {
                    let (label, color) = if self.paused { ("> RESUME", LIME.gamma_multiply(0.75)) } else { ("|| HOLD IDLE", MAGENTA.gamma_multiply(0.8)) };
                    if theme::aqua_button(ui, label, color, 130.0).clicked() {
                        self.paused = !self.paused;
                        let _ = self.live.tx.send(BrainIn::Pause(self.paused));
                    }
                    for s in &lib.states {
                        if theme::aqua_button(ui, &s.name, Color32::from_rgb(30, 90, 220), 0.0).on_hover_text(&s.description).clicked() {
                            let _ = self.live.tx.send(BrainIn::Manual(s.name.clone()));
                        }
                    }
                });
            });
            theme::bevel().show(ui, |ui| {
                theme::heading(ui, "live graphs");
                egui::ScrollArea::vertical().id_salt("graphs").show(ui, |ui| graphs(ui, &t));
            });
        });

        let mut changed = false;
        egui::Panel::right("settings").exact_size(420.0).frame(theme::panel_frame()).show(ui, |ui| {
            theme::bevel().show(ui, |ui| {
                theme::heading(ui, "settings");
                egui::ScrollArea::vertical().show(ui, |ui| {
                    changed |= settings_ui(ui, &mut self.settings, &mut self.aliases, &lib);
                    ui.separator();
                    ui.horizontal(|ui| {
                        let dirty = self.settings != self.saved;
                        let save = if dirty { CYAN.gamma_multiply(0.55) } else { Color32::from_rgb(60, 60, 90) };
                        if theme::aqua_button(ui, "SAVE", save, 90.0).clicked() && dirty {
                            self.status = match self.settings.save(&self.config_path) {
                                Ok(()) => {
                                    self.saved = self.settings.clone();
                                    format!("saved to {}", self.config_path.display())
                                }
                                Err(e) => format!("save failed: {e:#}"),
                            };
                        }
                        if theme::aqua_button(ui, "DEFAULTS", Color32::from_rgb(90, 60, 170), 90.0).clicked() {
                            let aliases = self.settings.brain.aliases.clone();
                            self.settings.brain = brain::BrainConfig { aliases, ..Default::default() };
                            changed = true;
                        }
                    });
                    ui.small(&self.status);
                });
            });
        });

        egui::CentralPanel::default().frame(theme::panel_frame()).show(ui, |ui| {
            let h = ui.available_height();
            theme::bevel().show(ui, |ui| {
                theme::heading(ui, "live transcript");
                egui::ScrollArea::vertical().id_salt("transcript").max_height(h * 0.36).stick_to_bottom(true).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    for (_, line, closed) in &t.transcript {
                        let text = RichText::new(line).size(13.5);
                        ui.label(if *closed { text } else { text.italics().color(ICE) });
                    }
                });
            });
            theme::bevel().show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::heading(ui, "what the stand-in did");
                    ui.checkbox(&mut self.show_decisions, RichText::new("every decision").small());
                });
                egui::ScrollArea::vertical().id_salt("events").max_height(h * 0.2).stick_to_bottom(true).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    for e in t.events.iter().filter(|e| self.show_decisions || e.kind != EventKind::Decision) {
                        let color = match e.kind {
                            EventKind::Reaction => LIME,
                            EventKind::Alert => MAGENTA,
                            EventKind::Decision => Color32::from_rgb(140, 140, 190),
                            EventKind::Info => CYAN,
                        };
                        let text: String = e.text.chars().take(140).collect();
                        ui.label(RichText::new(format!("{:7.1}s  {text}", e.t)).color(color).monospace());
                    }
                });
            });
            theme::bevel().show(ui, |ui| now_ui(ui, &t));
        });
        drop(t);
        if changed {
            self.push_settings();
        }
    }
}

/// Current values: room meters and the last decision.
fn now_ui(ui: &mut egui::Ui, t: &Telemetry) {
    theme::heading(ui, "right now");
    let c = t.last_cue;
    theme::led_meter(ui, "audio", t.peak, CYAN);
    theme::led_meter(ui, "laughter", c.laughter, CORAL);
    theme::led_meter(ui, "applause", c.applause, AMBER);
    theme::led_meter(ui, "engagement", t.engagement, LIME);
    let (dot, text) = if c.speaking { (LIME, "SOMEONE IS SPEAKING") } else { (Color32::from_rgb(90, 90, 130), "QUIET") };
    ui.label(RichText::new(format!("● {text}")).monospace().color(dot));
    theme::spacer(ui, 4.0);
    if let Some(d) = &t.decision {
        ui.label(
            RichText::new(format!("LAST DECISION  addressed {:.2}  question {:.2}  {:.0} ms", d.addressed, d.question, d.ms))
                .monospace()
                .size(11.0)
                .color(ICE),
        );
        for (i, (state, p)) in d.reaction.iter().enumerate() {
            theme::led_meter(ui, state, *p, PALETTE[i % PALETTE.len()]);
        }
    } else {
        ui.label(RichText::new("NO DECISION YET").monospace().color(ICE));
    }
}

fn graphs(ui: &mut egui::Ui, t: &Telemetry) {
    let h = &t.history;
    let marks: Vec<(f64, Color32)> = t
        .events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::Reaction => Some((e.t, LIME)),
            EventKind::Alert => Some((e.t, MAGENTA)),
            _ => None,
        })
        .collect();
    let now = t.now;
    let room = [("laughter", CORAL, &h.laughter), ("applause", AMBER, &h.applause), ("level", CYAN, &h.level)];
    Chart { title: "room · shaded = speech · ticks = reactions / alerts", y_max: Some(1.0), unit: "", band: Some(&h.speech), marks: &marks }
        .show(ui, now, &room);
    let probs: Vec<(&str, Color32, &Series)> =
        h.probs.iter().enumerate().map(|(i, (k, s))| (k.as_str(), PALETTE[i % PALETTE.len()], s)).collect();
    Chart { title: "decision model · p(reaction)", y_max: Some(1.0), unit: "", band: None, marks: &marks }.show(ui, now, &probs);
    Chart::plain("latency", None, "ms").show(ui, now, &[("speech", CYAN, &h.asr_ms), ("decider", MAGENTA, &h.decider_ms)]);
    Chart::plain("camera frame lateness", None, "ms").show(ui, now, &[("jitter", LIME, &h.jitter_ms)]);
    Chart::plain("cpu · 100% = one core", None, "%").show(ui, now, &[("cpu", CORAL, &h.cpu_pct)]);
    Chart::plain("memory", None, "MB").show(ui, now, &[("ram", CYAN, &h.rss_mb), ("gpu", MAGENTA, &h.gpu_mb)]);
    Chart::plain("gpu load", Some(100.0), "%").show(ui, now, &[("gpu", MAGENTA, &h.gpu_util)]);
}

/// A neon line chart of the last [`HISTORY_SECS`] seconds, with an optional shaded on/off band and
/// vertical event ticks.
struct Chart<'a> {
    title: &'a str,
    y_max: Option<f32>,
    unit: &'a str,
    band: Option<&'a Series>,
    marks: &'a [(f64, Color32)],
}

impl<'a> Chart<'a> {
    fn plain(title: &'a str, y_max: Option<f32>, unit: &'a str) -> Self {
        Self { title, y_max, unit, band: None, marks: &[] }
    }

    fn show(&self, ui: &mut egui::Ui, now: f64, series: &[(&str, Color32, &Series)]) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(self.title.to_uppercase()).monospace().size(10.5).color(theme::CHROME));
            for (name, color, s) in series {
                let v = s.last().map(|v| format!("{v:.1}{}", self.unit)).unwrap_or_else(|| "-".into());
                ui.label(RichText::new(format!("▬ {name} {v}")).monospace().size(10.5).color(*color));
            }
        });
        let (rect, _) = ui.allocate_exact_size(vec2(640.0, 82.0), egui::Sense::hover());
        let p = ui.painter_at(rect);
        theme::plot_area(&p, rect, HISTORY_SECS);
        let top = self.y_max.unwrap_or_else(|| {
            let m = series.iter().flat_map(|(_, _, s)| s.0.iter().map(|x| x.1)).fold(0.0f32, f32::max);
            (m * 1.15).max(1.0)
        });
        let x = |t: f64| rect.left() + ((HISTORY_SECS - (now - t)) / HISTORY_SECS) as f32 * rect.width();
        let y = |v: f32| rect.bottom() - 2.0 - (v / top).clamp(0.0, 1.0) * (rect.height() - 6.0);
        if let Some(band) = self.band {
            let pts: Vec<_> = band.0.iter().collect();
            for w in pts.windows(2).filter(|w| w[0].1 > 0.5) {
                let r = egui::Rect::from_min_max(pos2(x(w[0].0), rect.top()), pos2(x(w[1].0), rect.bottom()));
                p.rect_filled(r, 0.0, Color32::from_rgba_premultiplied(30, 60, 120, 70));
            }
        }
        for (t, color) in self.marks.iter().filter(|(t, _)| now - t <= HISTORY_SECS) {
            p.line_segment([pos2(x(*t), rect.top()), pos2(x(*t), rect.top() + 14.0)], Stroke::new(2.0, *color));
        }
        for (_, color, s) in series {
            theme::neon_line(&p, s.0.iter().map(|(t, v)| pos2(x(*t), y(*v))).collect(), *color);
        }
        let small = egui::FontId::monospace(9.5);
        p.text(rect.right_top() + vec2(-4.0, 2.0), egui::Align2::RIGHT_TOP, format!("{top:.0}{}", self.unit), small.clone(), theme::ICE);
        p.text(rect.left_bottom() + vec2(4.0, -2.0), egui::Align2::LEFT_BOTTOM, "-2:00", small, theme::ICE);
        ui.add_space(3.0);
    }
}

/// Every adjustable setting. Returns whether anything changed.
fn settings_ui(ui: &mut egui::Ui, s: &mut Settings, aliases: &mut String, lib: &video::Library) -> bool {
    let mut changed = false;
    let b = &mut s.brain;
    ui.label(RichText::new("YOUR NAMES (comma separated)").monospace().color(CYAN));
    if ui.text_edit_singleline(aliases).changed() {
        b.aliases = aliases.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        changed = true;
    }
    let base = &lib.states[lib.base].name;
    let section = |name: &str| RichText::new(name.to_uppercase()).monospace().strong().color(CYAN);
    egui::CollapsingHeader::new(section("reactions")).default_open(true).show(ui, |ui| {
        ui.small("Untick to stop the stand-in choosing a reaction. The slider is the confidence it needs.");
        for st in lib.states.iter().filter(|st| &st.name != base) {
            ui.horizontal(|ui| {
                let mut on = !b.disabled.contains(&st.name);
                if ui.checkbox(&mut on, "").on_hover_text(&st.description).changed() {
                    if on {
                        b.disabled.retain(|d| d != &st.name);
                    } else {
                        b.disabled.push(st.name.clone());
                    }
                    changed = true;
                }
                let mut th = b.threshold_for(&st.name);
                if ui.add_enabled(on, egui::Slider::new(&mut th, 0.1..=0.99).text(&st.name)).changed() {
                    b.state_thresholds.insert(st.name.clone(), th);
                    changed = true;
                }
            });
        }
    });
    egui::CollapsingHeader::new(section("laughter")).default_open(true).show(ui, |ui| {
        changed |= slider(ui, &mut b.laugh_threshold, 0.05..=1.0, "smile at laughter ≥");
        changed |= slider(ui, &mut b.big_laugh, 0.05..=1.0, "laugh at laughter ≥");
        changed |= slider(ui, &mut b.applause_threshold, 0.05..=1.0, "smile at applause ≥");
    });
    egui::CollapsingHeader::new(section("decisions & alerts")).default_open(true).show(ui, |ui| {
        changed |= slider(ui, &mut b.reaction_threshold, 0.1..=0.95, "default confidence");
        changed |= slider(ui, &mut b.alert_threshold, 0.1..=0.99, "alert when addressed p ≥");
        changed |= slider64(ui, &mut b.end_of_utterance_secs, 0.2..=2.0, "pause ending an utterance (s)");
        changed |= slider64(ui, &mut b.decide_every_secs, 0.5..=10.0, "decide during speech every (s)");
        changed |= slider64(ui, &mut b.context_secs, 10.0..=180.0, "transcript context (s)");
        changed |= slider64(ui, &mut b.alert_gap_secs, 1.0..=120.0, "gap between alerts (s)");
    });
    egui::CollapsingHeader::new(section("nodding")).default_open(true).show(ui, |ui| {
        changed |= slider64(ui, &mut b.nod_min_secs, 2.0..=30.0, "nod every at least (s)");
        changed |= slider64(ui, &mut b.nod_max_secs, 2.0..=60.0, "nod every at most (s)");
    });
    egui::CollapsingHeader::new(section("timing")).default_open(true).show(ui, |ui| {
        changed |= slider64(ui, &mut b.delay_min_secs, 0.0..=2.0, "reaction delay min (s)");
        changed |= slider64(ui, &mut b.delay_max_secs, 0.0..=3.0, "reaction delay max (s)");
        changed |= slider64(ui, &mut b.min_gap_secs, 0.0..=10.0, "gap between reactions (s)");
        changed |= slider64(ui, &mut b.same_state_gap_secs, 0.0..=30.0, "gap before repeating (s)");
    });
    egui::CollapsingHeader::new(section("video")).default_open(true).show(ui, |ui| {
        let mut blend = s.video.blend_frames as f32;
        if ui.add(egui::Slider::new(&mut blend, 0.0..=30.0).step_by(1.0).text("crossfade frames")).changed() {
            s.video.blend_frames = blend as usize;
            changed = true;
        }
        ui.small(format!("clips: {} (takes effect on restart)", s.video.manifest.display()));
    });
    changed
}

fn slider(ui: &mut egui::Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, text: &str) -> bool {
    ui.add(egui::Slider::new(v, range).text(text)).changed()
}

fn slider64(ui: &mut egui::Ui, v: &mut f64, range: std::ops::RangeInclusive<f64>, text: &str) -> bool {
    ui.add(egui::Slider::new(v, range).text(text)).changed()
}
