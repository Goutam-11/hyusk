use std::time::Instant;

use crate::ui::butterfly::Butterfly;

use eframe::egui;
use tokio::sync::mpsc::Receiver;

use crate::types::{HyuskEvent, HyuskState};

const ORB_SIZE: f32 = 160.0;

pub struct OrbApp {
    state: HyuskState,
    event_rx: Receiver<HyuskEvent>,
    started_at: Instant,
    response: Option<String>,
    positioned: bool,
    was_active: bool,
    steal_focus: bool,
}

impl OrbApp {
    pub fn new(event_rx: Receiver<HyuskEvent>) -> Self {
        let steal_focus = std::env::var("ORB_STEAL_FOCUS")
            .map(|value| {
                !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(false);

        Self {
            state: HyuskState::Hidden,
            event_rx,
            started_at: Instant::now(),
            response: None,
            positioned: false,
            was_active: false,
            steal_focus,
        }
    }

    fn process_events(&mut self) {
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => {
                    self.handle_event(event);
                }

                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    break;
                }

                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }
    }

    fn handle_event(&mut self, event: HyuskEvent) {
        match event {
            HyuskEvent::StateChanged(state) => {
                self.state = state;
            }

            HyuskEvent::Response(response) => {
                self.response = Some(response);
            }

            HyuskEvent::ToolStarted { name } => {
                println!("[UI] Tool started: {}", name);

                self.state = HyuskState::Working;
            }

            HyuskEvent::ToolFinished { name, success } => {
                println!(
                    "[UI] Tool finished: {} ({})",
                    name,
                    if success { "success" } else { "failed" }
                );
            }

            _ => {}
        }
    }

    fn is_active(&self) -> bool {
        self.state != HyuskState::Hidden
    }

    fn ring_color(&self) -> egui::Color32 {
        match self.state {
            HyuskState::Hidden => egui::Color32::from_rgb(36, 38, 48),
            HyuskState::Waking => egui::Color32::from_rgb(130, 210, 255),
            HyuskState::Listening => egui::Color32::from_rgb(120, 230, 170),
            HyuskState::Thinking => egui::Color32::from_rgb(180, 160, 255),
            HyuskState::Working => egui::Color32::from_rgb(255, 170, 110),
            HyuskState::Speaking => egui::Color32::from_rgb(255, 130, 200),
        }
    }

    fn draw_orb(&self, ui: &mut egui::Ui, center: egui::Pos2, time: f32) {
        let painter = ui.painter();

        let radius = (ORB_SIZE * 0.5 - 5.0).max(12.0);

        painter.circle_filled(center, radius, egui::Color32::from_rgb(4, 5, 8));

        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.6_f32, self.ring_color()),
        );

        match self.state {
            HyuskState::Waking => {
                self.draw_waking(painter, center, time);
            }

            HyuskState::Listening => {
                self.draw_butterfly(painter, center, time, 0.9);
            }

            HyuskState::Thinking => {
                self.draw_butterfly(painter, center, time, 1.05);
            }

            HyuskState::Working => {
                self.draw_butterfly(painter, center, time, 1.45);
            }

            HyuskState::Speaking => {
                self.draw_butterfly(painter, center, time, 1.15);
            }

            HyuskState::Hidden => {
                self.draw_butterfly(painter, center, time, 0.32);
            }
        }
    }

    fn draw_waking(&self, painter: &egui::Painter, center: egui::Pos2, time: f32) {
        let progress = (time * 2.5).min(1.0);

        let eased = 1.0 - (1.0 - progress) * (1.0 - progress);

        let position = egui::pos2(center.x, center.y - 26.0 + eased * 44.0);

        let stretch = 1.0 + (1.0 - progress) * 2.0;

        let radius = 6.0 + progress * 22.0;

        let color = self.ring_color();

        painter.circle_filled(position, radius * stretch, color);

        painter.circle_filled(
            egui::pos2(position.x, position.y - radius * 0.7),
            radius * 0.35,
            egui::Color32::from_rgb(240, 180, 255),
        );
    }

    fn draw_butterfly(
        &self,
        painter: &egui::Painter,
        center: egui::Pos2,
        time: f32,
        intensity: f32,
    ) {
        Butterfly::draw(painter, center, time, intensity);
    }

    fn request_top_position(&mut self, ctx: &egui::Context) {
        if self.positioned {
            return;
        }

        let monitor_size = ctx.input(|input| input.viewport().monitor_size);

        if let Some(monitor_size) = monitor_size {
            if monitor_size.x > 1.0 && monitor_size.y > 1.0 {
                let x = (monitor_size.x - ORB_SIZE) * 0.5;

                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, 8.0)));
            }
        }

        self.positioned = true;
    }
}

impl eframe::App for OrbApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_events();

        self.request_top_position(ctx);

        let active = self.is_active();

        if active && !self.was_active {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));

            /*
             * Keyboard input follows the focused window. Stealing focus while
             * the agent is working would send portal key events to the orb
             * instead of the user's application. Focus is therefore opt-in
             * through ORB_STEAL_FOCUS.
             */
            if self.steal_focus {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }

        self.was_active = active;

        let time = self.started_at.elapsed().as_secs_f32();

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();

                let center = rect.center();

                self.draw_orb(ui, center, time);
            });

        ctx.request_repaint();
    }
}

pub fn run(event_rx: Receiver<HyuskEvent>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([ORB_SIZE, ORB_SIZE])
            .with_min_inner_size([ORB_SIZE, ORB_SIZE])
            .with_max_inner_size([ORB_SIZE, ORB_SIZE])
            .with_position([0.0, 0.0])
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_window_level(egui::WindowLevel::AlwaysOnTop),

        ..Default::default()
    };

    eframe::run_native(
        "Hyusk",
        options,
        Box::new(move |_cc| Ok(Box::new(OrbApp::new(event_rx)))),
    )
}
