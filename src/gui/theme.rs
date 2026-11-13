//! Aparência: paleta clara/escura no estilo do macOS, fontes e o estilo base do egui.
//!
//! As cores seguem as "system colors" da Apple (azul #007AFF, verde #34C759...).
//! A fonte é a Inter (licença OFL), a mais próxima da SF Pro que pode ser
//! distribuída; a InterDisplay entra nos números grandes, como a SF Display.
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Shadow, Stroke};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pal {
    pub dark: bool,
    /// Fundo da área de conteúdo.
    pub window: Color32,
    pub sidebar: Color32,
    pub sidebar_line: Color32,
    pub toolbar_line: Color32,
    pub card: Color32,
    pub card_line: Color32,
    pub tile: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub text3: Color32,
    pub sep: Color32,
    pub accent: Color32,
    pub accent_press: Color32,
    pub accent_soft: Color32,
    pub green: Color32,
    pub orange: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub purple: Color32,
    pub teal: Color32,
    pub indigo: Color32,
    pub control: Color32,
    pub control_line: Color32,
    pub control_press: Color32,
    pub field: Color32,
    pub field_line: Color32,
    pub track: Color32,
    pub seg_sel: Color32,
    pub hover: Color32,
    pub select_gray: Color32,
    pub menu: Color32,
    pub shadow: Color32,
    pub console: Color32,
    pub window_border: Color32,
    pub hud: Color32,
    pub hud_text: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Mistura linear (no espaço sRGB, como o CSS faz).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let [r1, g1, b1, a1] = a.to_srgba_unmultiplied();
    let [r2, g2, b2, a2] = b.to_srgba_unmultiplied();
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(r1, r2), l(g1, g2), l(b1, b2), l(a1, a2))
}

pub fn light() -> Pal {
    Pal {
        dark: false,
        window: rgb(245, 245, 247),
        sidebar: rgb(234, 234, 237),
        sidebar_line: rgb(214, 214, 219),
        toolbar_line: rgb(225, 225, 229),
        card: rgb(255, 255, 255),
        card_line: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
        tile: rgb(246, 246, 248),
        text: rgb(29, 29, 31),
        text2: rgb(110, 110, 115),
        text3: rgb(162, 162, 168),
        sep: rgb(229, 229, 234),
        accent: rgb(0, 122, 255),
        accent_press: rgb(0, 98, 214),
        accent_soft: Color32::from_rgba_unmultiplied(0, 122, 255, 28),
        green: rgb(40, 180, 80),
        orange: rgb(255, 149, 0),
        red: rgb(255, 59, 48),
        yellow: rgb(255, 204, 0),
        purple: rgb(175, 82, 222),
        teal: rgb(48, 176, 199),
        indigo: rgb(88, 86, 214),
        control: rgb(255, 255, 255),
        control_line: rgb(206, 206, 212),
        control_press: rgb(232, 232, 236),
        field: rgb(255, 255, 255),
        field_line: rgb(204, 204, 210),
        track: rgb(229, 229, 234),
        seg_sel: rgb(255, 255, 255),
        hover: Color32::from_rgba_unmultiplied(0, 0, 0, 12),
        select_gray: Color32::from_rgba_unmultiplied(0, 0, 0, 22),
        menu: rgb(248, 248, 250),
        shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 30),
        console: rgb(252, 252, 253),
        window_border: Color32::from_rgba_unmultiplied(0, 0, 0, 60),
        hud: Color32::from_rgba_unmultiplied(40, 40, 44, 238),
        hud_text: rgb(250, 250, 252),
    }
}

pub fn dark() -> Pal {
    Pal {
        dark: true,
        window: rgb(30, 30, 32),
        sidebar: rgb(40, 40, 43),
        sidebar_line: rgb(16, 16, 18),
        toolbar_line: rgb(52, 52, 56),
        card: rgb(44, 44, 47),
        card_line: Color32::from_rgba_unmultiplied(255, 255, 255, 14),
        tile: rgb(54, 54, 58),
        text: rgb(245, 245, 247),
        text2: rgb(152, 152, 159),
        text3: rgb(99, 99, 104),
        sep: rgb(60, 60, 64),
        accent: rgb(10, 132, 255),
        accent_press: rgb(64, 156, 255),
        accent_soft: Color32::from_rgba_unmultiplied(10, 132, 255, 40),
        green: rgb(48, 209, 88),
        orange: rgb(255, 159, 10),
        red: rgb(255, 69, 58),
        yellow: rgb(255, 214, 10),
        purple: rgb(191, 90, 242),
        teal: rgb(64, 200, 224),
        indigo: rgb(94, 92, 230),
        control: rgb(72, 72, 76),
        control_line: rgb(84, 84, 88),
        control_press: rgb(96, 96, 100),
        field: rgb(36, 36, 39),
        field_line: rgb(70, 70, 75),
        track: rgb(64, 64, 68),
        seg_sel: rgb(99, 99, 104),
        hover: Color32::from_rgba_unmultiplied(255, 255, 255, 14),
        select_gray: Color32::from_rgba_unmultiplied(255, 255, 255, 26),
        menu: rgb(48, 48, 52),
        shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 90),
        console: rgb(26, 26, 28),
        window_border: Color32::from_rgba_unmultiplied(255, 255, 255, 34),
        hud: Color32::from_rgba_unmultiplied(64, 64, 68, 240),
        hud_text: rgb(250, 250, 252),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum W {
    Regular,
    Medium,
    Semibold,
    Bold,
    Display,
    Mono,
    Icon,
    IconFill,
}

pub fn font(size: f32, w: W) -> FontId {
    let family = match w {
        W::Regular => FontFamily::Proportional,
        W::Medium => FontFamily::Name("medium".into()),
        W::Semibold => FontFamily::Name("semibold".into()),
        W::Bold => FontFamily::Name("bold".into()),
        W::Display => FontFamily::Name("display".into()),
        W::Mono => FontFamily::Monospace,
        W::Icon => FontFamily::Name("phosphor".into()),
        W::IconFill => FontFamily::Name("phosphor-fill".into()),
    };
    FontId::new(size, family)
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    };
    add(&mut fonts, "inter", include_bytes!("../../assets/fonts/Inter-Regular.ttf"));
    add(&mut fonts, "inter-medium", include_bytes!("../../assets/fonts/Inter-Medium.ttf"));
    add(&mut fonts, "inter-semibold", include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"));
    add(&mut fonts, "inter-bold", include_bytes!("../../assets/fonts/Inter-Bold.ttf"));
    add(&mut fonts, "inter-display", include_bytes!("../../assets/fonts/InterDisplay-SemiBold.ttf"));

    // Texto: Inter primeiro; os padrões do egui ficam de reserva (emoji, símbolos).
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |main: &str| {
        let mut v = vec![main.to_string()];
        v.extend(fallback.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, family("inter"));
    for (name, key) in [("medium", "inter-medium"), ("semibold", "inter-semibold"), ("bold", "inter-bold"),
                        ("display", "inter-display")] {
        fonts.families.insert(FontFamily::Name(name.into()), family(key));
    }
    // Ícones: a fonte de ícones vem PRIMEIRO na família dela — a Inter tem glifos
    // próprios na mesma faixa Unicode (área privada) e ganharia dos ícones.
    add(&mut fonts, "phosphor", egui_phosphor::Variant::Regular.font_bytes());
    add(&mut fonts, "phosphor-fill", egui_phosphor::Variant::Fill.font_bytes());
    fonts.families.insert(FontFamily::Name("phosphor".into()), vec!["phosphor".into()]);
    fonts.families.insert(FontFamily::Name("phosphor-fill".into()), vec!["phosphor-fill".into()]);
    ctx.set_fonts(fonts);
}

pub fn apply_style(ctx: &egui::Context, p: &Pal) {
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.window;
    v.window_fill = p.menu;
    v.window_stroke = Stroke::new(1.0, p.card_line);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: p.shadow };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 20, spread: 0, color: p.shadow };
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.faint_bg_color = p.tile;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = alpha(p.accent, if p.dark { 0.45 } else { 0.28 });
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.text_cursor.stroke = Stroke::new(1.5, p.accent);
    v.warn_fg_color = p.orange;
    v.error_fg_color = p.red;
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered,
              &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(6);
        w.fg_stroke = Stroke::new(1.0, p.text);
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.sep);
    v.widgets.inactive.bg_fill = p.control;
    v.widgets.inactive.weak_bg_fill = p.control;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.control_line);
    v.widgets.hovered.bg_fill = p.control;
    v.widgets.hovered.weak_bg_fill = p.control;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.control_line);
    v.widgets.active.bg_fill = p.control_press;
    v.widgets.active.weak_bg_fill = p.control_press;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.control_line);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 4.0);
        s.spacing.interact_size = egui::vec2(24.0, 24.0);
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating_width = 3.0;
        s.text_styles.insert(egui::TextStyle::Body, font(13.0, W::Regular));
        s.text_styles.insert(egui::TextStyle::Button, font(13.0, W::Regular));
        s.text_styles.insert(egui::TextStyle::Small, font(11.0, W::Regular));
        s.text_styles.insert(egui::TextStyle::Heading, font(20.0, W::Bold));
        s.text_styles.insert(egui::TextStyle::Monospace, font(12.0, W::Mono));
        s.interaction.selectable_labels = false;
        s.animation_time = 0.18;
    });
}
