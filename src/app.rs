use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use eframe::egui::{self, Color32, Frame, Margin, RichText};
use crate::events::{ConnectionStatus, LogEvent};
use crate::logfile;
use crate::profile::{Profile, ProfileStore};
use crate::saml;
use crate::secrets;
use crate::vpn;
const BG: Color32 = Color32::from_rgb(0, 0, 0);
const PANEL: Color32 = Color32::from_rgb(0, 0, 0);
const CARD: Color32 = Color32::from_rgb(18, 18, 18);
const CARD_HOVER: Color32 = Color32::from_rgb(28, 28, 28);
const ACCENT_1: Color32 = Color32::from_rgb(230, 230, 230);
const ACCENT_TEXT: Color32 = Color32::from_rgb(12, 12, 12);
const ACCENT_2: Color32 = Color32::from_rgb(150, 150, 150);
const TEXT: Color32 = Color32::from_rgb(240, 240, 240);
const MUTED: Color32 = Color32::from_rgb(142, 142, 147);
const GREEN: Color32 = Color32::from_rgb(104, 199, 140);
const ERROR_RED: Color32 = Color32::from_rgb(224, 100, 100);
const AVATAR_PALETTE: [Color32; 5] = [
    ACCENT_2,
    GREEN,
    ERROR_RED,
    Color32::from_rgb(199, 168, 92),
    Color32::from_rgb(150, 140, 210),
];
pub fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = CARD;
    visuals.extreme_bg_color = CARD_HOVER;
    visuals.faint_bg_color = CARD;
    visuals.selection.bg_fill = ACCENT_1;
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT_1);
    visuals.hyperlink_color = ACCENT_2;
    visuals.window_corner_radius = 14.0.into();
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.corner_radius = 10.0.into();
    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.inactive.weak_bg_fill = CARD;
    visuals.widgets.inactive.corner_radius = 10.0.into();
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(64, 64, 64));
    visuals.widgets.hovered.bg_fill = CARD_HOVER;
    visuals.widgets.hovered.weak_bg_fill = CARD_HOVER;
    visuals.widgets.hovered.corner_radius = 10.0.into();
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT_2);
    visuals.widgets.active.bg_fill = ACCENT_1;
    visuals.widgets.active.weak_bg_fill = ACCENT_1;
    visuals.widgets.active.corner_radius = 10.0.into();
    visuals
}
fn profile_color(id: &str) -> Color32 {
    let sum: u32 = id.bytes().map(|b| b as u32).sum();
    AVATAR_PALETTE[sum as usize % AVATAR_PALETTE.len()]
}
fn draw_profile_icon(painter: &egui::Painter, center: egui::Pos2, r: f32, color: Color32) {
    let points = vec![
        center + egui::vec2(0.0, -r),
        center + egui::vec2(r * 0.85, -r * 0.45),
        center + egui::vec2(r * 0.7, r * 0.35),
        center + egui::vec2(0.0, r),
        center + egui::vec2(-r * 0.7, r * 0.35),
        center + egui::vec2(-r * 0.85, -r * 0.45),
    ];
    painter.add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
    painter.circle_filled(
        center + egui::vec2(0.0, -r * 0.05),
        r * 0.22,
        Color32::from_rgba_unmultiplied(255, 255, 255, 215),
    );
}
fn format_hms(d: Duration) -> String {
    let total = d.as_secs();
    format!("{:02}:{:02}:{:02}", total / 3600, (total % 3600) / 60, total % 60)
}
fn format_speed(bytes_per_sec: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    if bytes_per_sec >= MB {
        format!("{:.1} MB/s", bytes_per_sec / MB)
    } else if bytes_per_sec >= KB {
        format!("{:.0} KB/s", bytes_per_sec / KB)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}
struct EditForm {
    profile: Profile,
    password: String,
    totp_secret: String,
    is_new: bool,
}
impl EditForm {
    fn new_profile() -> Self {
        Self {
            profile: Profile::new_empty(),
            password: String::new(),
            totp_secret: String::new(),
            is_new: true,
        }
    }
    fn from_existing(profile: Profile, password: String, totp_secret: String) -> Self {
        Self {
            profile,
            password,
            totp_secret,
            is_new: false,
        }
    }
}
pub struct DrawbridgeApp {
    store: ProfileStore,
    selected_profile_id: Option<String>,
    editing_profile: Option<EditForm>,
    status: ConnectionStatus,
    log_lines: Vec<String>,
    log_rx: Option<Receiver<LogEvent>>,
    log_tx: Option<Sender<LogEvent>>,
    connected_pid: Option<u32>,
    connected_since: Option<Instant>,
    worker_running: bool,
    log_area_rect: Option<egui::Rect>,
    download_bps: f64,
    upload_bps: f64,
    net_monitor_stop: Option<Arc<AtomicBool>>,
    shutdown_done: bool,
}
impl DrawbridgeApp {
    pub fn new() -> Self {
        let store = ProfileStore::load().unwrap_or_default();
        let selected_profile_id = store.profiles.first().map(|p| p.id.clone());
        let mut log_lines = Vec::new();
        let startup_line = match logfile::path() {
            Some(path) => format!("Logging to {}", path.display()),
            None => "Failed to resolve the log file path".to_string(),
        };
        logfile::append_line(&startup_line);
        log_lines.push(startup_line);
        Self {
            store,
            selected_profile_id,
            editing_profile: None,
            status: ConnectionStatus::Idle,
            log_lines,
            log_rx: None,
            log_tx: None,
            connected_pid: None,
            connected_since: None,
            worker_running: false,
            log_area_rect: None,
            download_bps: 0.0,
            upload_bps: 0.0,
            net_monitor_stop: None,
            shutdown_done: false,
        }
    }
    fn shutdown(&mut self) {
        if self.shutdown_done {
            return;
        }
        self.shutdown_done = true;
        self.stop_speed_monitor();
        saml::cleanup_browsers();
        if self.status == ConnectionStatus::Connected || self.connected_pid.is_some() {
            self.push_log("Quitting: stopping the VPN tunnel");
        } else {
            self.push_log("Quitting");
        }
    }
    fn stop_speed_monitor(&mut self) {
        if let Some(stop) = self.net_monitor_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.download_bps = 0.0;
        self.upload_bps = 0.0;
    }
    fn selected_profile(&self) -> Option<&Profile> {
        self.selected_profile_id
            .as_deref()
            .and_then(|id| self.store.find(id))
    }
    fn push_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        logfile::append_line(&line);
        self.log_lines.push(line);
    }
    fn drain_log_events(&mut self) {
        let Some(rx) = &self.log_rx else {
            return;
        };
        while let Ok(event) = rx.try_recv() {
            match event {
                LogEvent::Status(new_status) => self.status = new_status,
                LogEvent::Log(line) => {
                    logfile::append_line(&line);
                    self.log_lines.push(line);
                }
                LogEvent::Error(message) => {
                    let line = format!("ERROR: {message}");
                    logfile::append_line(&line);
                    self.log_lines.push(line);
                    self.status = ConnectionStatus::Failed;
                    self.worker_running = false;
                    self.connected_since = None;
                    if let Some(stop) = self.net_monitor_stop.take() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    self.download_bps = 0.0;
                    self.upload_bps = 0.0;
                }
                LogEvent::Connected(pid) => {
                    self.connected_pid = Some(pid);
                    self.status = ConnectionStatus::Connected;
                    self.connected_since = Some(Instant::now());
                    if let Some(tx) = &self.log_tx {
                        let stop = Arc::new(AtomicBool::new(false));
                        vpn::spawn_speed_monitor(tx.clone(), stop.clone());
                        self.net_monitor_stop = Some(stop);
                    }
                }
                LogEvent::Disconnected => {
                    self.connected_pid = None;
                    self.status = ConnectionStatus::Idle;
                    self.worker_running = false;
                    self.connected_since = None;
                    if let Some(stop) = self.net_monitor_stop.take() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    self.download_bps = 0.0;
                    self.upload_bps = 0.0;
                }
                LogEvent::NetSpeed { down_bps, up_bps } => {
                    self.download_bps = down_bps;
                    self.upload_bps = up_bps;
                }
            }
        }
    }
    fn start_connect(&mut self) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.push_log("ERROR: No profile selected");
            return;
        };
        let (password, totp_secret) = match secrets::load_credentials(&profile.id) {
            Ok(credentials) => credentials,
            Err(e) => {
                self.push_log(format!("ERROR: {e:#}"));
                self.status = ConnectionStatus::Failed;
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        self.log_rx = Some(rx);
        self.log_tx = Some(tx.clone());
        self.log_lines.clear();
        self.status = ConnectionStatus::LoggingIn;
        self.worker_running = true;
        self.connected_since = None;
        self.stop_speed_monitor();
        thread::spawn(move || {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = tx.send(LogEvent::Error(format!(
                        "Failed to start the async runtime: {e}"
                    )));
                    return;
                }
            };
            let cookie_result = runtime.block_on(saml::login_and_get_cookie(
                &profile,
                &password,
                &totp_secret,
                tx.clone(),
            ));
            let cookie = match cookie_result {
                Ok(cookie) => cookie,
                Err(e) => {
                    let _ = tx.send(LogEvent::Error(format!("{e:#}")));
                    return;
                }
            };
            let _ = tx.send(LogEvent::Log("Starting openfortivpn".to_string()));
            match vpn::connect(&profile, &cookie, tx.clone()) {
                Ok(pid) => {
                    let _ = tx.send(LogEvent::Connected(pid));
                }
                Err(e) => {
                    let _ = tx.send(LogEvent::Error(format!("{e:#}")));
                }
            }
        });
    }
    fn start_disconnect(&mut self) {
        let Some(tx) = self.log_tx.clone() else {
            self.push_log("ERROR: No active session to disconnect");
            return;
        };
        self.worker_running = true;
        thread::spawn(move || match vpn::disconnect() {
            Ok(()) => {
                let _ = tx.send(LogEvent::Log("Disconnect command sent".to_string()));
                let _ = tx.send(LogEvent::Disconnected);
            }
            Err(e) => {
                let _ = tx.send(LogEvent::Error(format!("{e:#}")));
            }
        });
    }
    fn save_edit_form(&mut self) {
        let Some(form) = self.editing_profile.take() else {
            return;
        };
        if form.profile.name.trim().is_empty()
            || form.profile.vpn_host.trim().is_empty()
            || form.profile.vpn_port.trim().is_empty()
            || form.profile.email.trim().is_empty()
        {
            self.push_log("ERROR: Name, host, port and email are required");
            self.editing_profile = Some(form);
            return;
        }
        if let Err(e) = secrets::save_credentials(&form.profile.id, &form.password, &form.totp_secret)
        {
            self.push_log(format!("ERROR: {e:#}"));
            self.editing_profile = Some(form);
            return;
        }
        if form.is_new {
            self.store.add(form.profile.clone());
        } else if let Err(e) = self.store.update(form.profile.clone()) {
            self.push_log(format!("ERROR: {e:#}"));
            return;
        }
        if let Err(e) = self.store.save() {
            self.push_log(format!("ERROR: {e:#}"));
            return;
        }
        self.selected_profile_id = Some(form.profile.id);
    }
    fn delete_profile(&mut self, id: &str) {
        if let Err(e) = secrets::delete_secrets(id) {
            self.push_log(format!("ERROR: {e:#}"));
            return;
        }
        self.store.delete(id);
        if let Err(e) = self.store.save() {
            self.push_log(format!("ERROR: {e:#}"));
            return;
        }
        if self.selected_profile_id.as_deref() == Some(id) {
            self.selected_profile_id = self.store.profiles.first().map(|p| p.id.clone());
        }
    }
    fn edit_profile(&mut self, id: &str) {
        let (password, totp_secret) = secrets::load_credentials(id).unwrap_or_default();
        if let Some(profile) = self.store.find(id).cloned() {
            self.editing_profile = Some(EditForm::from_existing(profile, password, totp_secret));
        }
    }
    fn draw_header(&mut self, ui: &mut egui::Ui) {
        const LOGO_SIZE: f32 = 72.0;
        const SPACING: f32 = 12.0;
        let title = "DrawbridgeVPN";
        let font_id = egui::FontId::new(26.0, egui::FontFamily::Proportional);
        let galley = ui.painter().layout_no_wrap(title.to_string(), font_id, TEXT);
        let total_width = LOGO_SIZE + SPACING + galley.size().x;
        let left_pad = ((ui.available_width() - total_width) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(left_pad);
            ui.add(
                egui::Image::new(egui::include_image!("../assets/logo_header.png"))
                    .fit_to_exact_size(egui::vec2(LOGO_SIZE, LOGO_SIZE)),
            );
            ui.add_space(SPACING);
            let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
            let painter = ui.painter();
            for dx in [0.0_f32, 0.3, 0.6, 0.9, 1.2, 1.5] {
                for dy in [0.0_f32, 0.3] {
                    painter.galley(rect.min + egui::vec2(dx, dy), galley.clone(), TEXT);
                }
            }
        });
    }
    fn draw_hero_card(&mut self, ui: &mut egui::Ui) {
        let selected = self.selected_profile().cloned();
        let is_connected = self.status == ConnectionStatus::Connected;
        let is_busy = self.worker_running;
        let has_selection = selected.is_some();
        Frame::new()
            .fill(CARD)
            .corner_radius(18.0)
            .inner_margin(Margin::same(22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    let caption = if is_connected {
                        "CONNECTED FOR"
                    } else {
                        "CONNECTION TIME"
                    };
                    ui.label(RichText::new(caption).size(11.0).color(MUTED));
                    ui.add_space(4.0);
                    let timer_str = match self.connected_since {
                        Some(t) if is_connected => format_hms(t.elapsed()),
                        _ => "00:00:00".to_string(),
                    };
                    ui.label(
                        RichText::new(timer_str)
                            .size(38.0)
                            .strong()
                            .monospace()
                            .color(Color32::WHITE),
                    );
                    ui.add_space(14.0);
                    match &selected {
                        Some(p) => {
                            ui.label(RichText::new(&p.name).size(16.0).strong().color(TEXT));
                            ui.label(
                                RichText::new(format!("{}:{}", p.vpn_host, p.vpn_port))
                                    .size(12.0)
                                    .color(MUTED),
                            );
                        }
                        None => {
                            ui.label(RichText::new("No profile selected").size(14.0).color(MUTED));
                        }
                    }
                    if is_connected {
                        ui.add_space(10.0);
                        let down_text = format!("↓ {}", format_speed(self.download_bps));
                        let up_text = format!("↑ {}", format_speed(self.upload_bps));
                        let font_id = egui::FontId::monospace(12.0);
                        let painter = ui.painter();
                        let down_width = painter
                            .layout_no_wrap(down_text.clone(), font_id.clone(), MUTED)
                            .size()
                            .x;
                        let up_width = painter
                            .layout_no_wrap(up_text.clone(), font_id, MUTED)
                            .size()
                            .x;
                        const SPEED_SPACING: f32 = 16.0;
                        let total_width = down_width + SPEED_SPACING + up_width;
                        let left_pad = ((ui.available_width() - total_width) / 2.0).max(0.0);
                        ui.horizontal(|ui| {
                            ui.add_space(left_pad);
                            ui.label(RichText::new(down_text).size(12.0).monospace().color(MUTED));
                            ui.add_space(SPEED_SPACING);
                            ui.label(RichText::new(up_text).size(12.0).monospace().color(MUTED));
                        });
                    }
                    ui.add_space(16.0);
                    let (label, fill, text_color, enabled) = if is_connected {
                        ("Disconnect".to_string(), ERROR_RED, Color32::WHITE, true)
                    } else if is_busy {
                        (self.status.to_string(), CARD_HOVER, MUTED, false)
                    } else {
                        ("Connect".to_string(), ACCENT_1, ACCENT_TEXT, has_selection)
                    };
                    let button = egui::Button::new(RichText::new(label).size(15.0).strong().color(text_color))
                        .fill(fill)
                        .corner_radius(22.0)
                        .min_size(egui::vec2(180.0, 44.0));
                    if ui.add_enabled(enabled, button).clicked() {
                        if is_connected {
                            self.start_disconnect();
                        } else {
                            self.start_connect();
                        }
                    }
                });
            });
    }
    fn draw_profile_row(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(profile) = self.store.find(id) else {
            return;
        };
        let name = profile.name.clone();
        let host = profile.vpn_host.clone();
        let port = profile.vpn_port.clone();
        let color = profile_color(id);
        let is_selected = self.selected_profile_id.as_deref() == Some(id);
        let is_this_connected = is_selected && self.status == ConnectionStatus::Connected;
        let busy = self.worker_running;
        Frame::new()
            .fill(if is_selected { CARD_HOVER } else { CARD })
            .corner_radius(14.0)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
                    draw_profile_icon(ui.painter(), rect.center(), 15.0, color);
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        let name_color = if is_selected { ACCENT_TEXT } else { TEXT };
                        if ui
                            .selectable_label(
                                is_selected,
                                RichText::new(&name).size(14.0).strong().color(name_color),
                            )
                            .clicked()
                        {
                            self.selected_profile_id = Some(id.to_string());
                        }
                        ui.label(
                            RichText::new(format!("{host}:{port}"))
                                .size(11.0)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Delete").clicked() {
                            self.delete_profile(id);
                        }
                        if ui.small_button("Edit").clicked() {
                            self.edit_profile(id);
                        }
                        if is_this_connected {
                            ui.add_enabled(
                                false,
                                egui::Button::new(RichText::new("Connected").color(GREEN)),
                            );
                        } else if ui.add_enabled(!busy, egui::Button::new("Connect")).clicked() {
                            self.selected_profile_id = Some(id.to_string());
                            self.start_connect();
                        }
                    });
                });
            });
    }
    fn draw_profiles_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Profiles").size(15.0).strong().color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("+ Add Profile").strong()).clicked() {
                    self.editing_profile = Some(EditForm::new_profile());
                }
            });
        });
        ui.add_space(8.0);
        let profile_ids: Vec<String> = self.store.profiles.iter().map(|p| p.id.clone()).collect();
        if profile_ids.is_empty() {
            ui.label(RichText::new("No profiles yet. Add one to get started.").color(MUTED));
            return;
        }
        for id in &profile_ids {
            self.draw_profile_row(ui, id);
            ui.add_space(8.0);
        }
    }
    fn draw_log_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        egui::CollapsingHeader::new(RichText::new("Log").color(MUTED))
            .default_open(false)
            .show(ui, |ui| {
                let user_interacting = self.log_area_rect.is_some_and(|rect| {
                    ui.input(|i| {
                        i.pointer.any_down()
                            && i.pointer
                                .interact_pos()
                                .is_some_and(|pos| rect.contains(pos))
                    })
                });
                let output = egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .stick_to_bottom(!user_interacting)
                    .show(ui, |ui| {
                        for line in &self.log_lines {
                            ui.add(
                                egui::Label::new(RichText::new(line).size(11.0).monospace())
                                    .selectable(true),
                            );
                        }
                        if user_interacting {
                            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                                let clip = ui.clip_rect();
                                const EDGE: f32 = 16.0;
                                const SPEED: f32 = 8.0;
                                if pos.y > clip.bottom() - EDGE {
                                    ui.scroll_with_delta(egui::vec2(0.0, -SPEED));
                                    ui.ctx().request_repaint();
                                } else if pos.y < clip.top() + EDGE {
                                    ui.scroll_with_delta(egui::vec2(0.0, SPEED));
                                    ui.ctx().request_repaint();
                                }
                            }
                        }
                    });
                self.log_area_rect = Some(output.inner_rect);
            });
    }
    fn draw_edit_window(&mut self, ctx: &egui::Context) {
        if self.editing_profile.is_none() {
            return;
        }
        let mut should_close = false;
        let mut should_save = false;
        if let Some(form) = &mut self.editing_profile {
            let title = if form.is_new { "Add Profile" } else { "Edit Profile" };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    egui::Grid::new("edit_form_grid")
                        .num_columns(2)
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Name");
                            ui.text_edit_singleline(&mut form.profile.name);
                            ui.end_row();
                            ui.label("VPN Host");
                            ui.text_edit_singleline(&mut form.profile.vpn_host);
                            ui.end_row();
                            ui.label("VPN Port");
                            ui.text_edit_singleline(&mut form.profile.vpn_port);
                            ui.end_row();
                            ui.label("Email");
                            ui.text_edit_singleline(&mut form.profile.email);
                            ui.end_row();
                            ui.label("Password");
                            ui.add(egui::TextEdit::singleline(&mut form.password).password(true));
                            ui.end_row();
                            ui.label("TOTP Secret");
                            ui.add(egui::TextEdit::singleline(&mut form.totp_secret).password(true));
                            ui.end_row();
                            ui.label("Certificate Digest");
                            ui.text_edit_singleline(&mut form.profile.cert_digest);
                            ui.end_row();
                            ui.label("Login Browser");
                            ui.checkbox(
                                &mut form.profile.use_chrome,
                                "Use Google Chrome for login",
                            );
                            ui.end_row();
                        });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            should_save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            should_close = true;
                        }
                    });
                });
        }
        if should_save {
            self.save_edit_form();
        } else if should_close {
            self.editing_profile = None;
        }
    }
}
impl eframe::App for DrawbridgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_log_events();
        if self.worker_running || self.status == ConnectionStatus::Connected {
            ui.ctx().request_repaint();
        }
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::same(18)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.draw_header(ui);
                    ui.add_space(16.0);
                    self.draw_hero_card(ui);
                    self.draw_profiles_section(ui);
                    self.draw_log_section(ui);
                });
            });
        self.draw_edit_window(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) {
            self.shutdown();
        }
    }
    fn on_exit(&mut self) {
        self.shutdown();
    }
}
