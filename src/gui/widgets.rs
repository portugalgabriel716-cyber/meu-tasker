//! Controles desenhados à mão no estilo do macOS: botões, chave, controle
//! segmentado, campos com anel de foco, caixa de seleção, semáforo da janela...
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, Frame, Id, Margin, Pos2, Rect, Response, Sense,
    Shadow, Shape, Stroke, StrokeKind, Ui, Vec2,
};

use super::theme::{alpha, font, mix, Pal, W};

pub use egui_phosphor::regular as ic;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Normal,
    Primary,
    Danger,
    /// Botão de barra de ferramentas: sem moldura, fundo só no hover.
    Toolbar,
    /// Link azul, sem moldura.
    Link,
}

fn galley(ui: &Ui, text: &str, size: f32, w: W, color: Color32) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(text.to_string(), font(size, w), color)
}

/// Texto solto (com quebra de linha) num tamanho/peso/cor.
pub fn text(ui: &mut Ui, s: impl Into<String>, size: f32, w: W, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(s.into()).font(font(size, w)).color(color)).wrap())
}

/// Texto numa linha só, cortado com "…" se não couber.
pub fn text_trunc(ui: &mut Ui, s: impl Into<String>, size: f32, w: W, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(s.into()).font(font(size, w)).color(color)).truncate())
}

pub fn button(ui: &mut Ui, p: &Pal, label: &str, kind: Btn) -> Response {
    button_icon(ui, p, None, label, kind)
}

pub fn button_icon(ui: &mut Ui, p: &Pal, icon: Option<&str>, label: &str, kind: Btn) -> Response {
    let enabled = ui.is_enabled();
    let h = if kind == Btn::Toolbar { 28.0 } else { 24.0 };
    let fg = match kind {
        Btn::Primary => Color32::WHITE,
        Btn::Danger => p.red,
        Btn::Link => p.accent,
        _ => p.text,
    };
    let weight = if kind == Btn::Primary { W::Medium } else { W::Regular };
    let g = galley(ui, label, 13.0, weight, fg);
    let ig = icon.map(|i| galley(ui, i, if kind == Btn::Toolbar { 16.0 } else { 14.0 }, W::Icon, fg));
    let gap = if label.is_empty() { 0.0 } else { 6.0 };
    let icon_w = ig.as_ref().map(|g| g.size().x + gap).unwrap_or(0.0);
    let pad = match kind {
        Btn::Link => 2.0,
        Btn::Toolbar => 9.0,
        _ => 12.0,
    };
    let w = (g.size().x + icon_w + pad * 2.0).max(if kind == Btn::Link || kind == Btn::Toolbar { 0.0 } else { 64.0 });
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let pressed = resp.is_pointer_button_down_on();
    let hov = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered() && enabled, 0.12);
    let painter = ui.painter();
    let r = CornerRadius::same(6);
    let fade = if enabled { 1.0 } else { 0.45 };
    match kind {
        Btn::Primary => {
            let fill = if pressed { p.accent_press } else { mix(p.accent, p.accent_press, hov * 0.35) };
            painter.add(Shadow { offset: [0, 1], blur: 2, spread: 0, color: alpha(p.accent, 0.25 * fade) }.as_shape(rect, r));
            painter.rect_filled(rect, r, alpha(fill, fade));
            // Brilho de cima, como o gradiente sutil do botão padrão do macOS.
            painter.rect_filled(Rect::from_min_max(rect.min, pos2(rect.max.x, rect.center().y)),
                                CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, alpha(Color32::WHITE, 0.07 * fade));
        }
        Btn::Normal | Btn::Danger => {
            let fill = if pressed { p.control_press } else { p.control };
            if !p.dark {
                painter.add(Shadow { offset: [0, 1], blur: 2, spread: 0, color: alpha(Color32::BLACK, 0.07) }.as_shape(rect, r));
            }
            painter.rect(rect, r, fill, Stroke::new(1.0, if p.dark { Color32::TRANSPARENT } else { p.control_line }), StrokeKind::Inside);
            if hov > 0.0 && !pressed {
                painter.rect_filled(rect, r, alpha(p.text, 0.035 * hov));
            }
        }
        Btn::Toolbar => {
            let a = if pressed { 1.6 } else { hov };
            if a > 0.0 {
                painter.rect_filled(rect, r, alpha(p.text, 0.07 * a));
            }
        }
        Btn::Link => {}
    }
    let color = alpha(if kind == Btn::Link && hov > 0.5 { p.accent_press } else { fg }, fade);
    let total = icon_w + g.size().x;
    let mut x = rect.center().x - total / 2.0;
    if let Some(ig) = ig {
        let pos = pos2(x, rect.center().y - ig.size().y / 2.0);
        painter.galley(pos, ig.clone(), color);
        x += ig.size().x + gap;
    }
    painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    resp
}

/// A chave liga/desliga do macOS.
pub fn toggle(ui: &mut Ui, p: &Pal, on: &mut bool) -> Response {
    let size = vec2(38.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() && ui.is_enabled() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let fade = if ui.is_enabled() { 1.0 } else { 0.45 };
    let painter = ui.painter();
    let track = mix(p.track, p.accent, t);
    painter.rect_filled(rect, CornerRadius::same(11), alpha(track, fade));
    let r = 9.0;
    let cx = egui::lerp((rect.left() + 2.0 + r)..=(rect.right() - 2.0 - r), t);
    let c = pos2(cx, rect.center().y);
    painter.circle_filled(c + vec2(0.0, 0.6), r + 0.6, alpha(Color32::BLACK, 0.16 * fade));
    painter.circle_filled(c, r, alpha(Color32::WHITE, if ui.is_enabled() { 1.0 } else { 0.8 }));
    resp
}

/// Caixa de seleção azul com o "✓" desenhado.
pub fn checkbox(ui: &mut Ui, p: &Pal, on: &mut bool, label: &str) -> Response {
    let g = galley(ui, label, 13.0, W::Regular, p.text);
    let size = vec2(16.0 + 7.0 + g.size().x, 18.0f32.max(g.size().y));
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.1);
    let painter = ui.painter();
    let b = Rect::from_center_size(pos2(rect.left() + 8.0, rect.center().y), vec2(15.0, 15.0));
    let r = CornerRadius::same(4);
    painter.rect(b, r, mix(p.field, p.accent, t), Stroke::new(1.0, mix(p.field_line, p.accent, t)), StrokeKind::Inside);
    if t > 0.01 {
        let s = Stroke::new(1.8, alpha(Color32::WHITE, t));
        let pts = [b.left_center() + vec2(3.6, 0.2), pos2(b.left() + 6.4, b.bottom() - 4.0), pos2(b.right() - 3.4, b.top() + 4.0)];
        painter.add(Shape::line(pts.to_vec(), s));
    }
    painter.galley(pos2(b.right() + 7.0, rect.center().y - g.size().y / 2.0), g, p.text);
    resp
}

/// Controle segmentado (6 h | 24 h | ...). Devolve true se mudou.
pub fn segmented(ui: &mut Ui, p: &Pal, id: Id, items: &[&str], sel: &mut usize) -> bool {
    let gs: Vec<_> = items.iter().map(|s| galley(ui, s, 12.0, W::Medium, p.text)).collect();
    let seg_w = gs.iter().map(|g| g.size().x).fold(0.0, f32::max) + 22.0;
    let h = 24.0;
    let (rect, _) = ui.allocate_exact_size(vec2(seg_w * items.len() as f32 + 4.0, h), Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(7), p.track);
    let x = ui.ctx().animate_value_with_time(id.with("x"), *sel as f32, 0.16);
    let pill = Rect::from_min_size(pos2(rect.left() + 2.0 + x * seg_w, rect.top() + 2.0), vec2(seg_w, h - 4.0));
    painter.add(Shadow { offset: [0, 1], blur: 3, spread: 0, color: alpha(Color32::BLACK, if p.dark { 0.3 } else { 0.12 }) }
        .as_shape(pill, CornerRadius::same(5)));
    painter.rect_filled(pill, CornerRadius::same(5), p.seg_sel);
    let mut changed = false;
    for (i, g) in gs.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 2.0 + i as f32 * seg_w, rect.top()), vec2(seg_w, h));
        let resp = ui.interact(r, id.with(i), Sense::click());
        if resp.clicked() && *sel != i {
            *sel = i;
            changed = true;
        }
        if i > 0 && i != *sel && i != *sel + 1 && x.fract() == 0.0 {
            painter.line_segment([pos2(r.left(), r.top() + 6.0), pos2(r.left(), r.bottom() - 6.0)], Stroke::new(1.0, p.control_line));
        }
        let c = if i == *sel { p.text } else { p.text2 };
        painter.galley(r.center() - g.size() / 2.0, g, c);
    }
    changed
}

pub struct FieldOpts<'a> {
    pub hint: &'a str,
    pub width: f32,
    pub password: bool,
    pub align_right: bool,
    pub icon: Option<&'a str>,
    pub invalid: bool,
    pub mono: bool,
}

impl Default for FieldOpts<'_> {
    fn default() -> Self {
        FieldOpts { hint: "", width: 200.0, password: false, align_right: false, icon: None, invalid: false, mono: false }
    }
}

/// Campo de texto com moldura e anel de foco azul.
pub fn field(ui: &mut Ui, p: &Pal, id: Id, value: &mut String, o: FieldOpts) -> Response {
    let h = 24.0;
    let (rect, _) = ui.allocate_exact_size(vec2(o.width, h), Sense::hover());
    let bg = ui.painter().add(Shape::Noop);
    let pad_l = if o.icon.is_some() { 26.0 } else { 8.0 };
    let inner = Rect::from_min_max(pos2(rect.left() + pad_l, rect.top() + 3.5), pos2(rect.right() - 8.0, rect.bottom() - 3.0));
    let mut te = egui::TextEdit::singleline(value)
        .id(id)
        .frame(Frame::NONE)
        .margin(Margin::ZERO)
        .font(font(13.0, if o.mono { W::Mono } else { W::Regular }))
        .text_color(p.text)
        .password(o.password)
        .desired_width(inner.width())
        .hint_text(egui::RichText::new(o.hint).color(p.text3).font(font(13.0, W::Regular)));
    if o.align_right {
        te = te.horizontal_align(egui::Align::Max);
    }
    let resp = ui.place(inner, te);
    let focus = ui.ctx().animate_bool_with_time(id.with("f"), resp.has_focus(), 0.12);
    let r = CornerRadius::same(6);
    let line = if o.invalid { p.red } else { mix(p.field_line, p.accent, focus) };
    let mut shapes = vec![];
    if focus > 0.0 || o.invalid {
        let ring = if o.invalid { p.red } else { p.accent };
        shapes.push(Shape::Rect(egui::epaint::RectShape::filled(rect.expand(3.0), CornerRadius::same(9),
                                                                 alpha(ring, 0.28 * focus.max(if o.invalid { 0.7 } else { 0.0 })))));
    }
    shapes.push(Shape::Rect(egui::epaint::RectShape::new(rect, r, if ui.is_enabled() { p.field } else { p.tile },
                                                          Stroke::new(1.0, line), StrokeKind::Inside)));
    ui.painter().set(bg, Shape::Vec(shapes));
    if let Some(icon) = o.icon {
        ui.painter().text(pos2(rect.left() + 13.0, rect.center().y), Align2::CENTER_CENTER, icon, font(14.0, W::Icon), p.text2);
    }
    resp
}

/// Cápsula colorida com um ponto (estado).
pub fn pill(ui: &mut Ui, p: &Pal, label: &str, color: Color32) -> Response {
    let g = galley(ui, label, 12.0, W::Semibold, color);
    let size = vec2(g.size().x + 26.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(11), alpha(color, if p.dark { 0.2 } else { 0.13 }));
    painter.circle_filled(pos2(rect.left() + 11.0, rect.center().y), 3.5, color);
    painter.galley(pos2(rect.left() + 19.0, rect.center().y - g.size().y / 2.0), g, color);
    resp
}

/// Cartão: fundo, borda fina e sombra suave.
pub fn card(p: &Pal) -> Frame {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_line))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
        .shadow(if p.dark {
            Shadow::NONE
        } else {
            Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 10) }
        })
}

/// Caixa de lista agrupada (como nos Ajustes do Sistema).
pub fn group(p: &Pal) -> Frame {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_line))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 2))
}

/// Linha fina entre as linhas de uma caixa agrupada.
pub fn row_sep(ui: &mut Ui, p: &Pal) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([rect.left_center(), rect.right_center()], Stroke::new(1.0, p.sep));
}

/// Título de seção acima de um cartão/caixa.
pub fn section_title(ui: &mut Ui, p: &Pal, s: &str) {
    ui.add_space(4.0);
    text(ui, s, 13.0, W::Semibold, p.text);
    ui.add_space(2.0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Warn,
    Bad,
    Good,
}

impl Tone {
    pub fn color(self, p: &Pal) -> Color32 {
        match self {
            Tone::Info => p.accent,
            Tone::Warn => p.orange,
            Tone::Bad => p.red,
            Tone::Good => p.green,
        }
    }
}

/// Faixa de aviso com ícone, título, texto e um botão opcional. true = clicou.
pub fn callout(ui: &mut Ui, p: &Pal, tone: Tone, icon: &str, title: &str, body: &str, action: Option<&str>) -> bool {
    let c = tone.color(p);
    let mut clicked = false;
    Frame::new()
        .fill(alpha(c, if p.dark { 0.16 } else { 0.09 }))
        .stroke(Stroke::new(1.0, alpha(c, 0.25)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, font(20.0, W::IconFill), c);
                let btn_w = action.map(|a| ui.painter().layout_no_wrap(a.to_string(), font(13.0, W::Medium), p.text).size().x + 40.0).unwrap_or(0.0);
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - btn_w).max(100.0));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    text(ui, title, 13.0, W::Semibold, p.text);
                    if !body.is_empty() {
                        text(ui, body, 12.0, W::Regular, p.text2);
                    }
                });
                if let Some(a) = action {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if button(ui, p, a, Btn::Primary).clicked() {
                            clicked = true;
                        }
                    });
                }
            });
        });
    clicked
}

/// Barra de progresso fina.
pub fn progress(ui: &mut Ui, p: &Pal, frac: f32, color: Color32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let f = ui.ctx().animate_value_with_time(resp.id, frac.clamp(0.0, 1.0), 0.6);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), p.track);
    if f > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2((rect.width() * f).max(6.0), rect.height()));
        painter.rect_filled(fill, CornerRadius::same(3), color);
    }
}

/// O indicador giratório do macOS (12 traços que esmaecem).
pub fn spinner(ui: &mut Ui, p: &Pal, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let step = (t * 12.0).floor() as i32;
    let c = rect.center();
    for k in 0..12 {
        let ang = k as f32 / 12.0 * std::f32::consts::TAU;
        let dir = vec2(ang.sin(), -ang.cos());
        let age = ((step - k).rem_euclid(12)) as f32 / 12.0;
        let col = alpha(p.text2, 1.0 - age * 0.85);
        painter_line(ui, c + dir * size * 0.22, c + dir * size * 0.46, size * 0.09, col);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
}

fn painter_line(ui: &Ui, a: Pos2, b: Pos2, w: f32, c: Color32) {
    ui.painter().line_segment([a, b], Stroke::new(w, c));
    ui.painter().circle_filled(a, w / 2.0, c);
    ui.painter().circle_filled(b, w / 2.0, c);
}

/// Os três botões da janela (fechar, minimizar, zoom). Devolve qual foi clicado.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Light {
    Close,
    Minimize,
    Zoom,
}

pub fn traffic_lights(ui: &mut Ui, p: &Pal, origin: Pos2, focused: bool) -> Option<Light> {
    let group = Rect::from_min_size(origin, vec2(52.0, 12.0));
    let hover_group = ui.rect_contains_pointer(group.expand(4.0));
    let colors = [
        (Light::Close, Color32::from_rgb(255, 95, 87), Color32::from_rgb(224, 68, 62)),
        (Light::Minimize, Color32::from_rgb(254, 188, 46), Color32::from_rgb(222, 161, 35)),
        (Light::Zoom, Color32::from_rgb(40, 200, 64), Color32::from_rgb(29, 173, 43)),
    ];
    let mut clicked = None;
    for (i, (which, fill, edge)) in colors.into_iter().enumerate() {
        let c = pos2(origin.x + 6.0 + i as f32 * 20.0, origin.y + 6.0);
        let r = Rect::from_center_size(c, vec2(14.0, 14.0));
        let resp = ui.interact(r, Id::new(("luz", i)), Sense::click());
        let active = focused || hover_group;
        let (fill, edge) = if active {
            (fill, edge)
        } else if p.dark {
            (Color32::from_rgb(78, 78, 82), Color32::from_rgb(66, 66, 70))
        } else {
            (Color32::from_rgb(208, 208, 212), Color32::from_rgb(192, 192, 197))
        };
        let fill = if resp.is_pointer_button_down_on() { mix(fill, Color32::BLACK, 0.18) } else { fill };
        let painter = ui.painter();
        painter.circle_filled(c, 6.0, fill);
        painter.circle_stroke(c, 5.6, Stroke::new(0.8, edge));
        if hover_group {
            let g = Color32::from_rgba_unmultiplied(0, 0, 0, 140);
            let s = Stroke::new(1.15, g);
            match which {
                Light::Close => {
                    painter.line_segment([c + vec2(-2.4, -2.4), c + vec2(2.4, 2.4)], s);
                    painter.line_segment([c + vec2(-2.4, 2.4), c + vec2(2.4, -2.4)], s);
                }
                Light::Minimize => {
                    painter.line_segment([c + vec2(-3.0, 0.0), c + vec2(3.0, 0.0)], Stroke::new(1.3, g));
                }
                Light::Zoom => {
                    // Dois triângulos apontando pra fora, como no macOS.
                    let t1 = vec![c + vec2(-3.0, -3.0), c + vec2(1.4, -3.0), c + vec2(-3.0, 1.4)];
                    let t2 = vec![c + vec2(3.0, 3.0), c + vec2(-1.4, 3.0), c + vec2(3.0, -1.4)];
                    painter.add(Shape::convex_polygon(t1, g, Stroke::NONE));
                    painter.add(Shape::convex_polygon(t2, g, Stroke::NONE));
                }
            }
        }
        if resp.clicked() {
            clicked = Some(which);
        }
    }
    clicked
}

/// Item da barra lateral: ícone azul + rótulo, seleção em cápsula.
pub fn sidebar_item(ui: &mut Ui, p: &Pal, icon: &str, label: &str, selected: bool, focused: bool, badge: Option<(String, Color32)>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let painter = ui.painter();
    let r = CornerRadius::same(7);
    let sel_fill = if focused { p.accent } else { p.select_gray };
    let t = ui.ctx().animate_bool_with_time(resp.id, selected, 0.12);
    if t > 0.0 {
        painter.rect_filled(rect, r, alpha(sel_fill, t * (sel_fill.a() as f32 / 255.0)));
    } else if resp.hovered() {
        painter.rect_filled(rect, r, p.hover);
    }
    let on_accent = selected && focused;
    let icon_c = if on_accent { Color32::WHITE } else { p.accent };
    let text_c = if on_accent { Color32::WHITE } else { p.text };
    painter.text(pos2(rect.left() + 20.0, rect.center().y), Align2::CENTER_CENTER, icon, font(16.0, W::Icon), icon_c);
    painter.text(pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, label, font(13.0, W::Regular), text_c);
    if let Some((b, color)) = badge {
        let g = painter.layout_no_wrap(b, font(11.0, W::Semibold), if on_accent { p.accent } else { Color32::WHITE });
        let br = Rect::from_center_size(pos2(rect.right() - 10.0 - g.size().x / 2.0 - 4.0, rect.center().y),
                                        vec2(g.size().x + 10.0, 17.0));
        painter.rect_filled(br, CornerRadius::same(9), if on_accent { Color32::WHITE } else { color });
        painter.galley(br.center() - g.size() / 2.0, g, Color32::WHITE);
    }
    resp
}

/// Bloco de número com legenda (painel).
pub struct Stat<'a> {
    pub icon: &'a str,
    pub tint: Color32,
    pub caption: &'a str,
    pub value: String,
    pub detail: String,
}

pub fn stat_tile(ui: &mut Ui, p: &Pal, size: Vec2, s: &Stat) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size.max(vec2(60.0, 40.0)), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(10), p.tile);
    let inner = rect.shrink2(vec2(12.0, 9.0));
    let clip = painter.with_clip_rect(rect.shrink(2.0));
    // ícone em quadradinho colorido, como nos Ajustes do Sistema
    let ib = Rect::from_min_size(inner.min, vec2(20.0, 20.0));
    painter.rect_filled(ib, CornerRadius::same(5), s.tint);
    painter.text(ib.center(), Align2::CENTER_CENTER, s.icon, font(13.0, W::IconFill), Color32::WHITE);
    clip.text(pos2(ib.right() + 8.0, ib.center().y), Align2::LEFT_CENTER, s.caption, font(12.0, W::Medium), p.text2);
    // detalhe colado embaixo; o número fica no meio do que sobrar
    let mut bottom = inner.bottom();
    if !s.detail.is_empty() && inner.height() >= 54.0 {
        let dg = painter.layout_no_wrap(s.detail.clone(), font(11.5, W::Regular), p.text2);
        bottom -= dg.size().y;
        clip.galley(pos2(inner.left(), bottom), dg, p.text2);
    }
    let vg = painter.layout_no_wrap(s.value.clone(), font(19.0, W::Display), p.text);
    let y = ((ib.bottom() + bottom) / 2.0 - vg.size().y / 2.0).max(ib.bottom() + 2.0);
    clip.galley(pos2(inner.left(), y), vg, p.text);
    resp
}

/// Caixa de diálogo no estilo de alerta do macOS. Devolve Some(true) =
/// confirmou, Some(false) = cancelou, None = ainda aberta.
pub fn alert(ctx: &egui::Context, p: &Pal, icon: &egui::TextureHandle, title: &str, body: &str,
             ok_label: &str, danger: bool) -> Option<bool> {
    let screen = ctx.content_rect();
    let mut result = None;
    egui::Area::new(Id::new("alerta-fundo"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, CornerRadius::same(10), alpha(Color32::BLACK, if p.dark { 0.35 } else { 0.18 }));
            let _ = resp;
        });
    let w = 280.0;
    egui::Area::new(Id::new("alerta"))
        .order(egui::Order::Tooltip)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -30.0))
        .show(ctx, |ui| {
            Frame::new()
                .fill(p.menu)
                .stroke(Stroke::new(1.0, p.card_line))
                .corner_radius(CornerRadius::same(12))
                .shadow(Shadow { offset: [0, 12], blur: 40, spread: 0, color: alpha(Color32::BLACK, 0.3) })
                .inner_margin(Margin::same(18))
                .show(ui, |ui| {
                    ui.set_width(w - 36.0);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new(icon).fit_to_exact_size(vec2(56.0, 56.0)));
                        ui.add_space(6.0);
                        text(ui, title, 13.0, W::Bold, p.text);
                        ui.add_space(2.0);
                        text(ui, body, 11.5, W::Regular, p.text);
                        ui.add_space(12.0);
                    });
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let bw = (ui.available_width() - 8.0) / 2.0;
                        let cancel = wide_button(ui, p, "Cancelar", None, vec2(bw, 26.0));
                        let ok = wide_button(ui, p, ok_label, Some(if danger { p.red } else { p.accent }), vec2(bw, 26.0));
                        if cancel.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            result = Some(false);
                        }
                        if ok.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            result = Some(true);
                        }
                    });
                });
        });
    result
}

/// Botão de largura fixa (alertas). `fill` = botão cheio colorido.
fn wide_button(ui: &mut Ui, p: &Pal, label: &str, fill: Option<Color32>, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let down = resp.is_pointer_button_down_on();
    let r = CornerRadius::same(6);
    match fill {
        Some(c) => {
            ui.painter().rect_filled(rect, r, if down { mix(c, Color32::BLACK, 0.15) } else { c });
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font(13.0, W::Medium), Color32::WHITE);
        }
        None => {
            let bg = if down { p.control_press } else { p.control };
            ui.painter().rect(rect, r, bg, Stroke::new(1.0, if p.dark { Color32::TRANSPARENT } else { p.control_line }), StrokeKind::Inside);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font(13.0, W::Regular), p.text);
        }
    }
    resp
}

/// Menu suspenso no estilo do macOS: lista de opções (None = separador).
pub fn menu_list(ui: &mut Ui, p: &Pal, items: &[Option<&str>]) -> Option<usize> {
    let mut picked = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    let w = items.iter().flatten().map(|s| ui.painter().layout_no_wrap(s.to_string(), font(13.0, W::Regular), p.text).size().x)
        .fold(0.0, f32::max) + 36.0;
    for (i, it) in items.iter().enumerate() {
        match it {
            None => {
                let (r, _) = ui.allocate_exact_size(vec2(w, 9.0), Sense::hover());
                ui.painter().line_segment([pos2(r.left() + 6.0, r.center().y), pos2(r.right() - 6.0, r.center().y)], Stroke::new(1.0, p.sep));
            }
            Some(label) => {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
                let hov = resp.hovered();
                if hov {
                    ui.painter().rect_filled(r, CornerRadius::same(5), p.accent);
                }
                ui.painter().text(pos2(r.left() + 12.0, r.center().y), Align2::LEFT_CENTER, *label, font(13.0, W::Regular),
                                  if hov { Color32::WHITE } else { p.text });
                if resp.clicked() {
                    picked = Some(i);
                }
            }
        }
    }
    picked
}
