//! Medidor em anel e gráfico de linha, desenhados direto no `Painter`.
use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Shadow, Shape, Stroke, Ui};

use super::theme::{alpha, font, Pal, W};
use crate::types::RefillItem;
use crate::util::{comma, fmt_local, hhmm, same_local_day};

pub fn day_time(ts: f64, now: f64) -> String {
    if same_local_day(ts, now) { hhmm(ts) } else { fmt_local(ts, "%d/%m %H:%M") }
}

fn arc_points(c: Pos2, r: f32, from: f32, to: f32) -> Vec<Pos2> {
    let n = (((to - from).abs() / std::f32::consts::TAU) * 120.0).ceil().max(2.0) as usize;
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect()
}

/// Anel de progresso, com a marca do limite. `frac` já animado.
pub fn ring(ui: &mut Ui, p: &Pal, diameter: f32, frac: f32, color: Color32, limit: Option<f32>) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(diameter, diameter), Sense::hover());
    let painter = ui.painter();
    let w = (diameter * 0.085).round();
    let r = diameter / 2.0 - w / 2.0 - 1.0;
    let c = rect.center();
    painter.circle_stroke(c, r, Stroke::new(w, alpha(color, if p.dark { 0.18 } else { 0.13 })));
    let start = -std::f32::consts::FRAC_PI_2;
    let f = frac.clamp(0.0, 1.0);
    if f > 0.002 {
        let end = start + f * std::f32::consts::TAU;
        let pts = arc_points(c, r, start, end);
        let (a, b) = (pts[0], *pts.last().unwrap());
        painter.add(PathShape::line(pts, PathStroke::new(w, color)));
        painter.circle_filled(a, w / 2.0, color);
        painter.circle_filled(b, w / 2.0, color);
        // sombrinha na ponta, como nos anéis de atividade
        if f > 0.05 {
            let dir = (b - c).normalized();
            let tangent = vec2(-dir.y, dir.x);
            painter.circle_filled(b + tangent * 1.2, w / 2.0, alpha(Color32::BLACK, 0.10));
            painter.circle_filled(b, w / 2.0, color);
        }
    }
    if let Some(l) = limit.filter(|l| *l > 0.0 && *l < 1.0) {
        let a = start + l * std::f32::consts::TAU;
        let dir = vec2(a.cos(), a.sin());
        painter.line_segment([c + dir * (r - w / 2.0 - 1.0), c + dir * (r + w / 2.0 + 1.0)], Stroke::new(2.5, p.card));
        painter.circle_filled(c + dir * (r + w / 2.0 + 6.0), 2.2, p.text3);
    }
    rect
}

pub struct ChartData<'a> {
    pub volume: &'a [[f64; 2]],
    pub refills: &'a [RefillItem],
    pub hours: f64,
    pub limit: Option<f64>,
    pub now: f64,
}

pub fn line_chart(ui: &mut Ui, p: &Pal, id: Id, height: f32, d: &ChartData) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect.expand(6.0));
    let fim = d.now;
    let ini = fim - d.hours * 3600.0;
    let pts: Vec<[f64; 2]> = d.volume.iter().copied().filter(|p| p[0] >= ini).collect();
    if pts.len() < 2 {
        painter.text(rect.center(), Align2::CENTER_CENTER, "ainda sem amostras suficientes — uma por checagem",
                     font(12.5, W::Regular), p.text2);
        return;
    }
    let plot = Rect::from_min_max(pos2(rect.left() + 34.0, rect.top() + 8.0), pos2(rect.right() - 6.0, rect.bottom() - 22.0));
    let top = pts.iter().map(|p| p[1]).fold(0.0, f64::max);
    let ymax = ((top * 2.0).ceil() / 2.0).max(1.0);
    let x = |t: f64| plot.left() + ((t - ini) / (fim - ini)) as f32 * plot.width();
    let y = |g: f64| plot.top() + (1.0 - (g / ymax) as f32) * plot.height();

    // grade e eixo Y
    let step = if ymax > 3.0 { 1.0 } else { 0.5 };
    let mut g = 0.0;
    while g <= ymax + 1e-9 {
        let yy = y(g);
        painter.line_segment([pos2(plot.left(), yy), pos2(plot.right(), yy)],
                             Stroke::new(1.0, if g == 0.0 { p.sep } else { alpha(p.sep, 0.7) }));
        painter.text(pos2(plot.left() - 8.0, yy), Align2::RIGHT_CENTER, comma(crate::util::fmt_g(g)), font(10.5, W::Regular), p.text3);
        g += step;
    }
    // eixo X
    for k in 0..=4 {
        let t = ini + k as f64 * (fim - ini) / 4.0;
        let anchor = match k {
            0 => Align2::LEFT_TOP,
            4 => Align2::RIGHT_TOP,
            _ => Align2::CENTER_TOP,
        };
        let label = if k == 4 { "agora".to_string() } else if d.hours > 24.0 { day_time(t, fim) } else { hhmm(t) };
        painter.text(pos2(x(t), plot.bottom() + 7.0), anchor, label, font(10.5, W::Regular), p.text3);
    }

    // trechos contínuos (buraco = monitor parado; não vira rampa inventada)
    let gap = (d.hours * 3600.0 / 300.0).max(600.0);
    let mut runs: Vec<Vec<Pos2>> = vec![];
    let mut prev: Option<f64> = None;
    for q in &pts {
        let pt = pos2(x(q[0]), y(q[1]));
        if prev.is_none_or(|t| q[0] - t > gap) {
            runs.push(vec![]);
        }
        runs.last_mut().unwrap().push(pt);
        prev = Some(q[0]);
    }
    let base = plot.bottom();
    let top_c = alpha(p.accent, if p.dark { 0.32 } else { 0.22 });
    let bot_c = alpha(p.accent, 0.0);
    for run in &runs {
        if run.len() < 2 {
            continue;
        }
        let mut mesh = Mesh::default();
        for w in run.windows(2) {
            let (a, b) = (w[0], w[1]);
            let i = mesh.vertices.len() as u32;
            // A cor depende só da altura absoluta: degradê contínuo, sem listras.
            let ta = ((a.y - plot.top()) / plot.height()).clamp(0.0, 1.0);
            let tb = ((b.y - plot.top()) / plot.height()).clamp(0.0, 1.0);
            // (interpolação no espaço pré-multiplicado, igual ao da placa de vídeo)
            mesh.colored_vertex(a, top_c.gamma_multiply(1.0 - ta));
            mesh.colored_vertex(b, top_c.gamma_multiply(1.0 - tb));
            mesh.colored_vertex(pos2(b.x, base), bot_c);
            mesh.colored_vertex(pos2(a.x, base), bot_c);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
        }
        painter.add(Shape::mesh(mesh));
        painter.add(PathShape::line(run.clone(), PathStroke::new(1.8, p.accent)));
    }

    if let Some(lim) = d.limit.filter(|l| *l < ymax) {
        let yy = y(lim);
        painter.add(Shape::dashed_line(&[pos2(plot.left(), yy), pos2(plot.right(), yy)], Stroke::new(1.0, alpha(p.red, 0.65)), 4.0, 4.0));
        painter.text(pos2(plot.right() - 2.0, yy - 3.0), Align2::RIGHT_BOTTOM, "limite", font(10.0, W::Medium), alpha(p.red, 0.8));
    }

    if d.refills.len() < 300 {
        for r in d.refills.iter().filter(|r| r.t >= ini) {
            let v = if r.ok { r.after.unwrap_or(ymax) } else { r.before.unwrap_or(0.0) };
            let c = pos2(x(r.t), y(v.min(ymax)));
            let rad = if d.hours > 24.0 { 2.6 } else { 3.6 };
            painter.circle_filled(c, rad + 1.5, p.card);
            painter.circle_filled(c, rad, if r.ok { p.green } else { p.red });
        }
    }

    // hover: linha vertical + bolinha + balão
    if let Some(pos) = resp.hover_pos() {
        if plot.expand2(vec2(0.0, 8.0)).contains(pos) {
            let t = ini + ((pos.x - plot.left()) / plot.width()) as f64 * (fim - ini);
            let q = pts.iter().min_by(|a, b| (a[0] - t).abs().total_cmp(&(b[0] - t).abs())).copied().unwrap();
            let px = x(q[0]);
            let py = y(q[1]);
            painter.line_segment([pos2(px, plot.top()), pos2(px, plot.bottom())], Stroke::new(1.0, alpha(p.text2, 0.5)));
            painter.circle_filled(pos2(px, py), 4.5, p.card);
            painter.circle_stroke(pos2(px, py), 4.0, Stroke::new(2.0, p.accent));
            let label = format!("{}  ·  {} GB", day_time(q[0], fim), comma(format!("{:.2}", q[1])));
            let g = painter.layout_no_wrap(label, font(12.0, W::Medium), p.text);
            let size = g.size() + vec2(20.0, 12.0);
            let mut bx = px + 12.0;
            if bx + size.x > rect.right() {
                bx = px - 12.0 - size.x;
            }
            let by = (py - size.y - 10.0).max(rect.top() - 4.0);
            let br = Rect::from_min_size(pos2(bx, by), size);
            let tip = ui.painter().clone().with_layer_id(egui::LayerId::new(egui::Order::Tooltip, id.with("tip")));
            tip.add(Shadow { offset: [0, 3], blur: 10, spread: 0, color: p.shadow }.as_shape(br, CornerRadius::same(7)));
            tip.rect(br, CornerRadius::same(7), p.menu, Stroke::new(1.0, p.card_line), egui::StrokeKind::Inside);
            tip.galley(br.min + vec2(10.0, 6.0), g, p.text);
        }
    }
}

/// Bloco de legenda pequeno (cor + texto).
pub fn legend(ui: &mut Ui, p: &Pal, items: &[(Color32, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        for (c, label) in items {
            let g = ui.painter().layout_no_wrap(label.to_string(), font(11.0, W::Regular), p.text2);
            let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 14.0, 14.0), Sense::hover());
            ui.painter().circle_filled(pos2(r.left() + 4.0, r.center().y), 3.5, *c);
            ui.painter().galley(pos2(r.left() + 12.0, r.center().y - g.size().y / 2.0), g, p.text2);
        }
    });
}
