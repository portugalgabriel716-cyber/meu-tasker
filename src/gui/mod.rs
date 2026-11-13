//! A janela do Tasker: nativa (egui/OpenGL), com cara de app de macOS.
//!
//! A janela é um processo à parte do serviço: fechar não para o monitor. Ela
//! desenha a própria moldura (cantos arredondados, semáforo, barra de
//! ferramentas unificada); com `TASKER_DECORACAO=sistema` (ou a opção em
//! Ajustes → Aparência) usa a barra de título do sistema.
mod charts;
mod data;
mod pages;
mod theme;
mod widgets;

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Id, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder};
use serde_json::{json, Value};

use self::data::{Cmd, Shared};
use self::theme::{alpha, font, Pal, W};
use self::widgets::{ic, Btn};
use crate::paths::Paths;
use crate::store::{read_json, write_json};
use crate::types::AppState;
use crate::util::{env_get, hhmm, now, Env};

pub const SIDEBAR_W: f32 = 216.0;
pub const TOOLBAR_H: f32 = 52.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Painel,
    Historico,
    Log,
    Ajustes,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::Painel, Page::Historico, Page::Log, Page::Ajustes];

    fn title(self) -> &'static str {
        match self {
            Page::Painel => "Painel",
            Page::Historico => "Histórico",
            Page::Log => "Log",
            Page::Ajustes => "Ajustes",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Painel => ic::GAUGE,
            Page::Historico => ic::CHART_LINE,
            Page::Log => ic::LIST_BULLETS,
            Page::Ajustes => ic::GEAR_SIX,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Page::Painel => "painel",
            Page::Historico => "historico",
            Page::Log => "log",
            Page::Ajustes => "ajustes",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemePref {
    Auto,
    Light,
    Dark,
}

/// Preferências só da janela (dados/janela.json).
#[derive(Clone, Debug)]
pub struct Prefs {
    pub theme: ThemePref,
    pub system_frame: bool,
    pub size: [f32; 2],
    pub page: Page,
}

impl Prefs {
    fn load(paths: &Paths, env: &Env) -> Prefs {
        let m = read_json(&paths.window_file).unwrap_or_default();
        let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let theme = match s("tema").as_str() {
            "claro" => ThemePref::Light,
            "escuro" => ThemePref::Dark,
            _ => ThemePref::Auto,
        };
        let size = m.get("tamanho").and_then(Value::as_array)
            .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(1120.0) as f32, a.get(1).and_then(Value::as_f64).unwrap_or(780.0) as f32])
            .unwrap_or([1120.0, 780.0]);
        let page = Page::ALL.into_iter().find(|p| p.key() == s("aba")).unwrap_or(Page::Painel);
        let system_frame = match env_get(env, "TASKER_DECORACAO") {
            Some(v) => v == "sistema",
            None => s("moldura") == "sistema",
        };
        Prefs { theme, system_frame, size: [size[0].clamp(760.0, 4000.0), size[1].clamp(560.0, 3000.0)], page }
    }

    fn save(&self, paths: &Paths) {
        let tema = match self.theme {
            ThemePref::Auto => "auto",
            ThemePref::Light => "claro",
            ThemePref::Dark => "escuro",
        };
        let _ = write_json(&paths.window_file, &json!({
            "tema": tema,
            "moldura": if self.system_frame { "sistema" } else { "propria" },
            "tamanho": [self.size[0].round(), self.size[1].round()],
            "aba": self.page.key(),
        }), false);
    }
}

/// Pergunta de confirmação pendente.
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub ok: String,
    pub danger: bool,
    pub then: Then,
}

pub enum Then {
    Act(Value),
    Defaults,
}

/// Captura automática de telas (desenvolvimento): TASKER_FOTOS=pasta.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Extra {
    None,
    Menu,
    Alert,
}

struct Snap {
    dir: std::path::PathBuf,
    queue: Vec<(Page, bool, Extra)>,
    started: f64,
    sent: bool,
}

pub struct TaskerApp {
    pub shared: Arc<Mutex<Shared>>,
    pub tx: Sender<Cmd>,
    pub paths: Arc<Paths>,
    pub prefs: Prefs,
    pub page: Page,
    pub pal: Pal,
    applied: Option<Pal>,
    system_dark: Option<bool>,
    pub icon: egui::TextureHandle,
    // painel
    pub login_num: String,
    pub login_pw: String,
    pub login_remember: bool,
    pub force_login: bool,
    // log
    pub log_filter: String,
    pub log_follow: bool,
    // histórico
    pub hist_sel: usize,
    // ajustes
    pub form: BTreeMap<String, String>,
    pub form_bool: BTreeMap<String, bool>,
    pub form_base: (BTreeMap<String, String>, BTreeMap<String, bool>),
    pub form_gen: u64,
    pub form_saving: bool,
    pub confirm: Option<Confirm>,
    pub pause_open: bool,
    last_focus_seq: Option<u64>,
    snap: Option<Snap>,
    closing_at: Option<f64>,
    focus_search: bool,
    opened_at: f64,
}

pub fn run(paths: Arc<Paths>, env: Env) -> i32 {
    let prefs = Prefs::load(&paths, &env);
    let icon = image::load_from_memory(crate::system::ICON_PNG).map(|i| i.to_rgba8()).ok();
    let mut vp = egui::ViewportBuilder::default()
        .with_title("Tasker")
        .with_app_id("tasker")
        .with_inner_size(prefs.size)
        .with_min_inner_size([760.0, 560.0])
        .with_decorations(prefs.system_frame)
        .with_transparent(!prefs.system_frame);
    if let Some(i) = &icon {
        vp = vp.with_icon(egui::IconData { rgba: i.as_raw().clone(), width: i.width(), height: i.height() });
    }
    let options = eframe::NativeOptions { viewport: vp, centered: true, ..Default::default() };
    let snap_dir = env_get(&env, "TASKER_FOTOS").map(std::path::PathBuf::from);
    let result = eframe::run_native(
        "Tasker",
        options,
        Box::new(move |cc| Ok(Box::new(TaskerApp::new(cc, paths, prefs, snap_dir)))),
    );
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Não consegui abrir a janela: {e}");
            1
        }
    }
}

/// Tema escuro do sistema, quando o winit não sabe dizer (Linux/X11).
fn detect_dark() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        use std::time::Duration;
        let q = |schema: &str, key: &str| {
            crate::store::run_quiet("gsettings", &["get", schema, key], Duration::from_secs(2), &[])
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
        };
        if let Some(s) = q("org.gnome.desktop.interface", "color-scheme") {
            if s.contains("dark") {
                return Some(true);
            }
        }
        for schema in ["org.cinnamon.desktop.interface", "org.gnome.desktop.interface"] {
            if let Some(s) = q(schema, "gtk-theme") {
                return Some(s.contains("dark"));
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    None
}

impl TaskerApp {
    fn new(cc: &eframe::CreationContext, paths: Arc<Paths>, prefs: Prefs, snap_dir: Option<std::path::PathBuf>) -> TaskerApp {
        theme::install_fonts(&cc.egui_ctx);
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        data::spawn(paths.clone(), shared.clone(), rx, cc.egui_ctx.clone());
        let img = image::load_from_memory(crate::system::ICON_PNG).map(|i| i.to_rgba8()).unwrap_or_default();
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
        let icon = cc.egui_ctx.load_texture("icone", color, egui::TextureOptions::LINEAR);
        let snap = snap_dir.map(|dir| {
            let _ = std::fs::create_dir_all(&dir);
            let mut queue = vec![];
            for dark in [false, true] {
                for p in Page::ALL {
                    queue.push((p, dark, Extra::None));
                }
            }
            queue.push((Page::Painel, false, Extra::Menu));
            queue.push((Page::Painel, true, Extra::Alert));
            Snap { dir, queue, started: now(), sent: false }
        });
        let page = prefs.page;
        TaskerApp {
            shared,
            tx,
            paths,
            page,
            prefs,
            pal: theme::light(),
            applied: None,
            system_dark: detect_dark(),
            icon,
            login_num: String::new(),
            login_pw: String::new(),
            login_remember: true,
            force_login: false,
            log_filter: String::new(),
            log_follow: true,
            hist_sel: 1,
            form: BTreeMap::new(),
            form_bool: BTreeMap::new(),
            form_base: Default::default(),
            form_gen: 0,
            form_saving: false,
            confirm: None,
            pause_open: false,
            last_focus_seq: None,
            snap,
            closing_at: None,
            focus_search: false,
            opened_at: now(),
        }
    }

    pub fn act(&self, body: Value) {
        let _ = self.tx.send(Cmd::Action(body));
    }

    pub fn ask(&mut self, title: &str, body: &str, ok: &str, danger: bool, then: Then) {
        self.confirm = Some(Confirm { title: title.into(), body: body.into(), ok: ok.into(), danger, then });
    }

    pub fn go(&mut self, page: Page) {
        if self.page != page {
            self.page = page;
            self.pause_open = false;
            match page {
                Page::Ajustes => {
                    let _ = self.tx.send(Cmd::LoadSettings);
                }
                Page::Historico => {
                    let _ = self.tx.send(Cmd::LoadHistory(pages::HOURS[self.hist_sel]));
                }
                _ => {}
            }
        }
    }

    fn pick_palette(&mut self, ctx: &egui::Context) {
        let dark = match self.prefs.theme {
            ThemePref::Light => false,
            ThemePref::Dark => true,
            ThemePref::Auto => match ctx.system_theme() {
                Some(t) => t == egui::Theme::Dark,
                None => self.system_dark.unwrap_or(false),
            },
        };
        let dark = match &self.snap {
            Some(s) => s.queue.first().map(|q| q.1).unwrap_or(dark),
            None => dark,
        };
        self.pal = if dark { theme::dark() } else { theme::light() };
        if self.applied != Some(self.pal) {
            theme::apply_style(ctx, &self.pal);
            self.applied = Some(self.pal);
        }
    }

    fn custom_frame(&self) -> bool {
        !self.prefs.system_frame
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (cmd, keys) = ctx.input(|i| {
            let k = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::R,
                     egui::Key::W, egui::Key::Comma, egui::Key::F];
            (i.modifiers.command, k.map(|key| i.key_pressed(key)))
        });
        if !cmd || self.confirm.is_some() {
            return;
        }
        for (n, p) in Page::ALL.into_iter().enumerate() {
            if keys[n] {
                self.go(p);
            }
        }
        if keys[4] {
            self.act(json!({"acao": "checar"}));
        }
        if keys[5] {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if keys[6] {
            self.go(Page::Ajustes);
        }
        if keys[7] {
            self.go(Page::Log);
            self.focus_search = true;
        }
    }

    /// Bordas da janela sem moldura: o cursor muda e arrastar redimensiona.
    fn resize_edges(&self, ctx: &egui::Context, full: Rect) {
        use egui::viewport::ResizeDirection as D;
        use egui::CursorIcon as C;
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized {
            return;
        }
        let t = 5.0;
        let c = 12.0;
        let edges = [
            (Rect::from_min_max(full.min, pos2(full.left() + c, full.top() + c)), D::NorthWest, C::ResizeNorthWest),
            (Rect::from_min_max(pos2(full.right() - c, full.top()), pos2(full.right(), full.top() + c)), D::NorthEast, C::ResizeNorthEast),
            (Rect::from_min_max(pos2(full.left(), full.bottom() - c), pos2(full.left() + c, full.bottom())), D::SouthWest, C::ResizeSouthWest),
            (Rect::from_min_max(pos2(full.right() - c, full.bottom() - c), full.max), D::SouthEast, C::ResizeSouthEast),
            (Rect::from_min_max(full.min, pos2(full.right(), full.top() + t)), D::North, C::ResizeVertical),
            (Rect::from_min_max(pos2(full.left(), full.bottom() - t), full.max), D::South, C::ResizeVertical),
            (Rect::from_min_max(full.min, pos2(full.left() + t, full.bottom())), D::West, C::ResizeHorizontal),
            (Rect::from_min_max(pos2(full.right() - t, full.top()), full.max), D::East, C::ResizeHorizontal),
        ];
        egui::Area::new(Id::new("bordas"))
            .order(egui::Order::Foreground)
            .fixed_pos(full.min)
            .show(ctx, |ui| {
                for (i, (r, dir, cursor)) in edges.into_iter().enumerate() {
                    let resp = ui.interact(r, Id::new(("borda", i)), Sense::drag());
                    if resp.hovered() || resp.dragged() {
                        ctx.set_cursor_icon(cursor);
                    }
                    if resp.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                    }
                }
            });
    }

    fn sidebar(&mut self, ui: &mut Ui, rect: Rect, st: Option<&AppState>, focused: bool) {
        let p = self.pal;
        let painter = ui.painter();
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        let rad = if self.custom_frame() && !maximized { 10 } else { 0 };
        painter.rect_filled(rect, CornerRadius { nw: rad, sw: rad, ne: 0, se: 0 }, p.sidebar);
        painter.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, p.sidebar_line));

        // faixa de cima arrasta a janela
        let drag_rect = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + TOOLBAR_H));
        self.drag_area(ui, drag_rect, "arrasta-lateral");
        if self.custom_frame() {
            if let Some(l) = widgets::traffic_lights(ui, &p, pos2(rect.left() + 20.0, rect.top() + 20.0), focused) {
                let cmd = match l {
                    widgets::Light::Close => egui::ViewportCommand::Close,
                    widgets::Light::Minimize => egui::ViewportCommand::Minimized(true),
                    widgets::Light::Zoom => egui::ViewportCommand::Maximized(!maximized),
                };
                ui.ctx().send_viewport_cmd(cmd);
            }
        }

        let list = Rect::from_min_max(pos2(rect.left() + 10.0, rect.top() + TOOLBAR_H + 4.0), pos2(rect.right() - 10.0, rect.bottom() - 90.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(list));
        child.spacing_mut().item_spacing.y = 2.0;
        widgets::text(&mut child, "Tasker", 11.0, W::Semibold, p.text3);
        child.add_space(2.0);
        let log_badge = st.and_then(|s| (s.monitor.falhas_seguidas > 0).then(|| (s.monitor.falhas_seguidas.to_string(), p.red)));
        for page in Page::ALL {
            let badge = if page == Page::Log { log_badge.clone() } else { None };
            if widgets::sidebar_item(&mut child, &p, page.icon(), page.title(), self.page == page, focused, badge).clicked() {
                self.go(page);
            }
        }

        // cartão de estado embaixo
        let card = Rect::from_min_max(pos2(rect.left() + 10.0, rect.bottom() - 80.0), pos2(rect.right() - 10.0, rect.bottom() - 12.0));
        let painter = ui.painter();
        painter.rect_filled(card, CornerRadius::same(10), if p.dark { alpha(Color32::WHITE, 0.05) } else { alpha(Color32::WHITE, 0.6) });
        painter.rect_stroke(card, CornerRadius::same(10), Stroke::new(1.0, p.card_line), StrokeKind::Inside);
        let ir = Rect::from_min_size(card.min + vec2(12.0, 12.0), vec2(30.0, 30.0));
        painter.image(self.icon.id(), ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        painter.text(pos2(ir.right() + 10.0, ir.top() + 7.0), Align2::LEFT_CENTER, "Tasker", font(13.0, W::Semibold), p.text);
        let (label, color) = status_line(&p, st, self.shared.lock().unwrap().connected);
        painter.circle_filled(pos2(ir.right() + 14.0, ir.top() + 24.0), 3.5, color);
        let clip = painter.with_clip_rect(card.shrink(4.0));
        clip.text(pos2(ir.right() + 22.0, ir.top() + 24.0), Align2::LEFT_CENTER, label, font(11.5, W::Regular), p.text2);
        let ver = st.map(|s| format!("versão {}", s.sistema.versao)).unwrap_or_default();
        clip.text(pos2(card.left() + 12.0, card.bottom() - 13.0), Align2::LEFT_CENTER, ver, font(10.5, W::Regular), p.text3);
    }

    fn drag_area(&self, ui: &mut Ui, rect: Rect, id: &str) {
        if !self.custom_frame() {
            return;
        }
        let resp = ui.interact(rect, Id::new(id), Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, rect: Rect, st: Option<&AppState>) {
        let p = self.pal;
        self.drag_area(ui, rect, "arrasta-barra");
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, p.toolbar_line));
        let title_pos = pos2(rect.left() + 22.0, rect.center().y);
        ui.painter().text(title_pos, Align2::LEFT_CENTER, self.page.title(), font(16.0, W::Bold), p.text);

        let inner = Rect::from_min_max(pos2(rect.left() + 160.0, rect.top()), pos2(rect.right() - 14.0, rect.bottom()));
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 4.0;
        let Some(st) = st else { return };
        let m = &st.monitor;
        let enabled = self.shared.lock().unwrap().connected;
        child.add_enabled_ui(enabled, |ui| {
            if m.pausado {
                if widgets::button_icon(ui, &p, Some(ic::PLAY), "Retomar", Btn::Toolbar).clicked() {
                    self.act(json!({"acao": "retomar"}));
                }
            } else {
                let resp = widgets::button_icon(ui, &p, Some(ic::PAUSE), "Pausar", Btn::Toolbar);
                if resp.clicked() {
                    self.pause_open = !self.pause_open;
                }
                if self.pause_open {
                    let picked = egui::Area::new(Id::new("menu-pausa"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(pos2(resp.rect.right() - 168.0, resp.rect.bottom() + 6.0))
                        .show(ui.ctx(), |ui| {
                            Frame::new()
                                .fill(p.menu)
                                .stroke(Stroke::new(1.0, p.card_line))
                                .corner_radius(CornerRadius::same(8))
                                .shadow(egui::Shadow { offset: [0, 6], blur: 22, spread: 0, color: p.shadow })
                                .inner_margin(egui::Margin::same(5))
                                .show(ui, |ui| {
                                    ui.set_min_width(158.0);
                                    widgets::menu_list(ui, &p, &[Some("Por 30 minutos"), Some("Por 1 hora"), Some("Por 2 horas"), None,
                                                               Some("Até eu voltar…")])
                                })
                                .inner
                        });
                    let outside = ui.input(|i| i.pointer.any_click())
                        && !picked.response.rect.contains(ui.input(|i| i.pointer.interact_pos().unwrap_or_default()))
                        && !resp.clicked();
                    if let Some(i) = picked.inner {
                        self.pause_open = false;
                        match i {
                            0 => self.act(json!({"acao": "pausar", "minutos": 30})),
                            1 => self.act(json!({"acao": "pausar", "minutos": 60})),
                            2 => self.act(json!({"acao": "pausar", "minutos": 120})),
                            _ => self.ask("Pausar sem prazo?", "O Tasker só volta a vigiar o volume quando você clicar em Retomar.",
                                          "Pausar", false, Then::Act(json!({"acao": "pausar", "minutos": 0}))),
                        }
                    } else if outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.pause_open = false;
                    }
                }
            }
            ui.add_enabled_ui(m.ligado && !m.pausado, |ui| {
                if widgets::button_icon(ui, &p, Some(ic::ARROWS_CLOCKWISE), "Checar agora", Btn::Toolbar)
                    .on_hover_text("Ctrl+R")
                    .clicked()
                {
                    self.act(json!({"acao": "checar"}));
                }
            });
        });
        child.add_space(8.0);
        let (label, color) = pill_state(&p, m, enabled);
        widgets::pill(&mut child, &p, label, color);
        if self.shared.lock().unwrap().busy > 0 {
            child.add_space(4.0);
            widgets::spinner(&mut child, &p, 16.0);
        }
    }

    fn toasts(&self, ctx: &egui::Context) {
        let p = self.pal;
        let mut s = self.shared.lock().unwrap();
        let t = now();
        s.toasts.retain(|(_, at)| t - at < 5.0);
        let Some((msg, at)) = s.toasts.back().cloned() else { return };
        drop(s);
        let age = (t - at) as f32;
        let fade = (age / 0.18).min(1.0).min(((5.0 - age) / 0.35).max(0.0));
        let screen = ctx.content_rect();
        let max_w = (screen.width() - SIDEBAR_W - 80.0).clamp(240.0, 560.0);
        egui::Area::new(Id::new("hud"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(SIDEBAR_W / 2.0, -26.0 + (1.0 - fade) * 12.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(fade);
                Frame::new()
                    .fill(p.hud)
                    .corner_radius(CornerRadius::same(12))
                    .shadow(egui::Shadow { offset: [0, 8], blur: 26, spread: 0, color: alpha(Color32::BLACK, 0.28) })
                    .inner_margin(egui::Margin::symmetric(16, 11))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(ic::INFO).font(font(15.0, W::IconFill)).color(alpha(p.hud_text, 0.8)));
                            widgets::text(ui, msg, 13.0, W::Medium, p.hud_text);
                        });
                    });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
    }

    fn run_confirm(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.confirm else { return };
        let p = self.pal;
        if let Some(ok) = widgets::alert(ctx, &p, &self.icon, &c.title, &c.body, &c.ok, c.danger) {
            let c = self.confirm.take().unwrap();
            if ok {
                match c.then {
                    Then::Act(v) => self.act(v),
                    Then::Defaults => {
                        self.act(json!({"acao": "padroes"}));
                        self.shared.lock().unwrap().save_errors.clear();
                        self.form_gen = 0;
                        let _ = self.tx.send(Cmd::LoadSettings);
                    }
                }
            }
        }
    }

    fn snapshots(&mut self, ctx: &egui::Context) {
        let Some(snap) = &mut self.snap else { return };
        // Fotos que chegaram
        let images: Vec<(String, Arc<egui::ColorImage>)> = ctx.input(|i| {
            i.raw.events.iter().filter_map(|e| match e {
                egui::Event::Screenshot { user_data, image, .. } => user_data.data.as_ref()
                    .and_then(|d| d.downcast_ref::<String>()).map(|n| (n.clone(), image.clone())),
                _ => None,
            }).collect()
        });
        for (name, img) in images {
            let path = snap.dir.join(format!("{name}.png"));
            let buf = image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32,
                                                 img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect());
            if let Some(b) = buf {
                let _ = b.save(&path);
            }
            snap.queue.remove(0);
            snap.sent = false;
            snap.started = now();
        }
        let Some(&(page, dark, extra)) = snap.queue.first() else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        if self.page != page {
            self.page = page;
            let _ = self.tx.send(Cmd::LoadSettings);
            let _ = self.tx.send(Cmd::LoadHistory(24));
        }
        let first = !snap.sent && now() - snap.started < 0.05;
        match extra {
            Extra::Menu if first => {
                self.pause_open = true;
                self.shared.lock().unwrap().toast("Checando agora…");
            }
            Extra::Alert if first => {
                self.pause_open = false;
                self.confirm = Some(Confirm { title: "Encerrar o Tasker?".into(), body: "Ninguém vai vigiar o volume até você abrir de novo.".into(),
                                              ok: "Encerrar".into(), danger: true, then: Then::Act(json!({})) });
            }
            _ => {}
        }
        let snap = self.snap.as_mut().unwrap();
        let wait = if snap.queue.len() == 10 { 3.5 } else { 1.6 };
        if !snap.sent && now() - snap.started > wait {
            let tag = match extra {
                Extra::None => "",
                Extra::Menu => "-menu",
                Extra::Alert => "-alerta",
            };
            let name = format!("{}-{}{tag}", page.key(), if dark { "escuro" } else { "claro" });
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(name)));
            snap.sent = true;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Texto + cor do estado (barra lateral).
fn status_line(p: &Pal, st: Option<&AppState>, connected: bool) -> (String, Color32) {
    let Some(st) = st.filter(|_| connected) else { return ("sem contato com o serviço".into(), p.red) };
    let m = &st.monitor;
    let vol = m.volume.as_ref().map(|v| format!(" · {} GB", crate::util::comma(format!("{:.2}", v.left)))).unwrap_or_default();
    if !m.ligado {
        ("monitor desligado".into(), p.red)
    } else if m.precisa_login.is_some() {
        ("sem sessão".into(), p.red)
    } else if m.pausado {
        (m.pausa_ate.map(|t| format!("pausado até {}", hhmm(t))).unwrap_or_else(|| "pausado".into()), p.orange)
    } else if m.erro.is_some() {
        (format!("instável{vol}"), p.orange)
    } else {
        (format!("vigiando{vol}"), p.green)
    }
}

fn pill_state(p: &Pal, m: &crate::types::MonitorSnap, connected: bool) -> (&'static str, Color32) {
    if !connected {
        ("sem contato", p.red)
    } else if !m.ligado {
        ("desligado", p.red)
    } else if m.precisa_login.is_some() {
        ("sem sessão", p.red)
    } else if m.pausado {
        ("pausado", p.orange)
    } else if m.fase == "confirmando refill" {
        ("confirmando refill", p.orange)
    } else if m.erro.is_some() {
        ("instável", p.orange)
    } else {
        ("vigiando", p.green)
    }
}

impl eframe::App for TaskerApp {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        if self.custom_frame() { [0.0; 4] } else { self.pal.window.to_normalized_gamma_f32() }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pick_palette(&ctx);
        self.snapshots(&ctx);
        self.shortcuts(&ctx);
        let p = self.pal;
        let (st, connected, ever, starting, quit, focus) = {
            let s = self.shared.lock().unwrap();
            (s.state.clone(), s.connected, s.ever_connected, s.starting, s.quit, s.state.as_ref().map(|x| x.sistema.foco))
        };
        // clicaram no atalho com a janela já aberta: vem pra frente
        if let Some(f) = focus {
            if self.last_focus_seq.is_some_and(|old| old != f) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            self.last_focus_seq = Some(f);
        }
        if quit && self.closing_at.is_none() {
            self.closing_at = Some(now());
        }
        if let Some(t) = self.closing_at {
            if now() - t > 0.6 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if let Some(r) = ctx.input(|i| i.viewport().inner_rect) {
                if !ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
                    self.prefs.size = [r.width(), r.height()];
                }
            }
            self.prefs.page = self.page;
            self.prefs.save(&self.paths);
        }

        let full = ui.max_rect();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let rad = if self.custom_frame() && !maximized { 10 } else { 0 };
        ui.painter().rect_filled(full, CornerRadius::same(rad), p.window);

        let side = Rect::from_min_max(full.min, pos2(full.left() + SIDEBAR_W, full.bottom()));
        self.sidebar(ui, side, st.as_ref(), focused);
        let main = Rect::from_min_max(pos2(side.right() + 1.0, full.top()), full.max);
        let bar = Rect::from_min_max(main.min, pos2(main.right(), main.top() + TOOLBAR_H));
        self.toolbar(ui, bar, st.as_ref());

        let footer_h = if self.page == Page::Ajustes && st.is_some() { 56.0 } else { 0.0 };
        let body = Rect::from_min_max(pos2(main.left(), bar.bottom() + 1.0), pos2(main.right(), main.bottom() - footer_h));
        let mut child = ui.new_child(UiBuilder::new().max_rect(body));
        child.set_clip_rect(body);
        match &st {
            None => self.empty_state(&mut child, starting, ever),
            Some(st) => {
                if self.page == Page::Log {
                    // o log cuida da própria rolagem
                    let inner = body.shrink2(vec2(22.0, 18.0));
                    let mut c2 = child.new_child(UiBuilder::new().max_rect(inner));
                    if !connected {
                        self.lost_contact(&mut c2);
                    }
                    let focus_search = std::mem::take(&mut self.focus_search);
                    self.page_log(&mut c2, focus_search);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("rolagem", self.page.key()))
                        .auto_shrink([false, false])
                        .show(&mut child, |ui| {
                            Frame::new().inner_margin(egui::Margin { left: 22, right: 22, top: 18, bottom: 24 }).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 14.0;
                                if !connected {
                                    self.lost_contact(ui);
                                }
                                match self.page {
                                    Page::Painel => self.page_painel(ui, st),
                                    Page::Historico => self.page_historico(ui, st),
                                    Page::Ajustes => self.page_ajustes(ui, st),
                                    Page::Log => {}
                                }
                            });
                        });
                }
            }
        }
        if footer_h > 0.0 {
            let foot = Rect::from_min_max(pos2(main.left(), main.bottom() - footer_h), main.max);
            self.settings_footer(ui, foot);
        }
        if self.custom_frame() && !maximized {
            ui.painter().rect_stroke(full, CornerRadius::same(rad), Stroke::new(1.0, p.window_border), StrokeKind::Inside);
            self.resize_edges(&ctx, full);
        }
        self.run_confirm(&ctx);
        self.toasts(&ctx);
        // relógio da próxima checagem anda sozinho
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

impl TaskerApp {
    fn empty_state(&mut self, ui: &mut Ui, starting: bool, ever: bool) {
        let p = self.pal;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 120.0).max(40.0));
            ui.add(egui::Image::new(&self.icon).fit_to_exact_size(vec2(72.0, 72.0)));
            ui.add_space(10.0);
            if starting || !ever && now() - self.opened_at < 3.0 {
                widgets::text(ui, "Abrindo o Tasker…", 17.0, W::Semibold, p.text);
                ui.add_space(8.0);
                widgets::spinner(ui, &p, 20.0);
            } else {
                widgets::text(ui, "O Tasker não está rodando", 17.0, W::Semibold, p.text);
                ui.add_space(2.0);
                widgets::text(ui, "O serviço que vigia o volume não respondeu. Abra de novo pra ele voltar.", 13.0, W::Regular, p.text2);
                ui.add_space(14.0);
                if widgets::button(ui, &p, "Abrir o Tasker", Btn::Primary).clicked() {
                    let _ = self.tx.send(Cmd::StartService);
                }
            }
        });
    }

    fn lost_contact(&mut self, ui: &mut Ui) {
        let p = self.pal;
        if widgets::callout(ui, &p, widgets::Tone::Bad, ic::WARNING_CIRCLE, "Sem contato com o serviço do Tasker",
                            "Ele pode ter sido encerrado. Os números abaixo são da última leitura.", Some("Abrir o Tasker")) {
            let _ = self.tx.send(Cmd::StartService);
        }
    }

    fn settings_footer(&mut self, ui: &mut Ui, rect: Rect) {
        let p = self.pal;
        let painter = ui.painter();
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        let rad = if self.custom_frame() && !maximized { 10 } else { 0 };
        painter.rect_filled(rect, CornerRadius { se: rad, sw: 0, ne: 0, nw: 0 }, alpha(p.window, 0.96));
        painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, p.toolbar_line));
        let inner = rect.shrink2(vec2(22.0, 0.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
        child.spacing_mut().item_spacing.x = 8.0;
        let dirty = self.form_dirty();
        let has_errors = !self.shared.lock().unwrap().save_errors.is_empty();
        if child.add_enabled_ui(dirty, |ui| widgets::button(ui, &p, "Salvar", Btn::Primary)).inner.clicked() {
            self.save_form();
        }
        if widgets::button(&mut child, &p, "Restaurar padrões", Btn::Normal).clicked() {
            self.ask("Restaurar os ajustes padrão?", "Todos os ajustes voltam ao valor de fábrica. Número, senha e sessão não mudam.",
                     "Restaurar", true, Then::Defaults);
        }
        let msg = if has_errors {
            ("Tem campo fora do limite — nada foi gravado.", p.red)
        } else if dirty {
            ("Alterações não salvas", p.text2)
        } else {
            ("Os ajustes valem na hora — não precisa reiniciar nada.", p.text3)
        };
        child.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            widgets::text(ui, msg.0, 12.0, W::Regular, msg.1);
        });
        if child.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) && dirty {
            self.save_form();
        }
    }
}
