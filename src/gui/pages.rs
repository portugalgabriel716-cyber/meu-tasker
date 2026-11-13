//! As páginas: Painel, Histórico, Log e Ajustes.
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Id, Rect, Sense, Stroke, Ui};
use serde_json::{json, Value};

use super::charts::{self, ChartData};
use super::data::Cmd;
use super::theme::{font, W};
use super::widgets::{self, ic, Btn, FieldOpts, Stat, Tone};
use super::{Page, TaskerApp, Then, ThemePref};
use crate::types::{AppState, History};
use crate::util::{comma, dur, fmt_g, hhmm, now};

pub const HOURS: [i64; 4] = [6, 24, 72, 168];
const HOUR_LABELS: [&str; 4] = ["6 h", "24 h", "3 dias", "7 dias"];

fn gb(v: f64) -> String {
    comma(format!("{v:.2}"))
}

fn gb_short(v: f64) -> String {
    comma(fmt_g((v * 100.0).round() / 100.0))
}

fn mmss(s: f64) -> String {
    let s = s.max(0.0).round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Título de cartão + texto à direita.
fn card_header(ui: &mut Ui, p: &super::theme::Pal, title: &str, right: &str) {
    ui.horizontal(|ui| {
        widgets::text(ui, title, 14.0, W::Semibold, p.text);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::text_trunc(ui, right, 12.0, W::Regular, p.text2);
        });
    });
}

fn summary(h: &History) -> String {
    let r = &h.resumo;
    let mut t = format!("{} refill{}", r.refills_periodo, if r.refills_periodo == 1 { "" } else { "s" });
    if r.falhas_periodo > 0 {
        t += &format!(" · {} falha{}", r.falhas_periodo, if r.falhas_periodo > 1 { "s" } else { "" });
    }
    if let Some(l) = r.latencia_media {
        t += &format!(" · entram em ~{} s", l.round());
    }
    t
}

impl TaskerApp {
    // ================================================================ painel

    pub fn page_painel(&mut self, ui: &mut Ui, st: &AppState) {
        let p = self.pal;
        let m = &st.monitor;
        if st.sistema.versao.starts_with("2.")
            && widgets::callout(ui, &p, Tone::Info, ic::ROCKET_LAUNCH, "O serviço rodando ainda é o da versão em Python",
                                "Esta janela já é a nova. Trocar encerra o serviço antigo e sobe o novo: a sessão e os ajustes continuam os mesmos.",
                                Some("Trocar agora")) {
            let _ = self.tx.send(Cmd::Migrate);
        }
        if m.precisa_login.is_some() || self.force_login {
            self.login_card(ui, st);
        }
        if m.pausado && m.ligado {
            let title = m.pausa_ate.map(|t| format!("Pausado até {}", hhmm(t))).unwrap_or_else(|| "Pausado até você retomar".into());
            if widgets::callout(ui, &p, Tone::Warn, ic::PAUSE, &title,
                                "O Tasker não toca na sessão: pode usar a conta no celular ou no site.", Some("Retomar agora")) {
                self.act(json!({"acao": "retomar"}));
            }
        }
        if !m.ligado {
            if widgets::callout(ui, &p, Tone::Bad, ic::POWER, "Monitor desligado",
                                "O Tasker está aberto, mas ninguém está vigiando o volume.", Some("Ligar monitor")) {
                self.act(json!({"acao": "ligar"}));
            }
        } else if let Some(e) = m.erro.as_ref().filter(|_| !m.pausado) {
            widgets::callout(ui, &p, Tone::Warn, ic::WARNING, "A última leitura falhou", e, None);
        }

        let wide = ui.available_width() >= 720.0;
        let h = 330.0;
        if wide {
            // Lado a lado, com retângulos explícitos (as duas alturas iguais).
            let row = ui.available_rect_before_wrap();
            let gw = 290.0;
            let r1 = Rect::from_min_size(row.min, vec2(gw, h));
            let r2 = Rect::from_min_max(pos2(r1.right() + 14.0, row.top()), pos2(row.right(), row.top() + h));
            let mut c1 = ui.new_child(egui::UiBuilder::new().max_rect(r1));
            self.gauge_card(&mut c1, st, h);
            let mut c2 = ui.new_child(egui::UiBuilder::new().max_rect(r2));
            self.stats_card(&mut c2, st, h);
            ui.allocate_exact_size(vec2(row.width(), h), Sense::hover());
        } else {
            self.gauge_card(ui, st, h);
            self.stats_card(ui, st, h);
        }
        self.stress_card(ui, st);
        self.chart_card(ui, st);
    }

    fn gauge_card(&mut self, ui: &mut Ui, st: &AppState, h: f32) {
        let p = self.pal;
        let m = &st.monitor;
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(h - 36.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            card_header(ui, &p, "Refill disponível", "");
            ui.add_space(6.0);
            let lim = m.limite.unwrap_or(0.0);
            let (frac, color, limit) = match &m.volume {
                Some(v) if v.total.unwrap_or(0.0) > 0.0 => {
                    let t = v.total.unwrap();
                    let c = if v.left <= lim { p.red } else if v.left <= lim + 0.2 * t { p.orange } else { p.green };
                    ((v.left / t) as f32, c, Some((lim / t) as f32))
                }
                _ => (0.0, p.text3, None),
            };
            let f = ui.ctx().animate_value_with_time(Id::new("anel"), frac, 0.7);
            ui.vertical_centered(|ui| {
                let r = charts::ring(ui, &p, 184.0, f, color, limit);
                let painter = ui.painter();
                let (big, unit) = match &m.volume {
                    Some(v) => (gb(v.left), v.total.map(|t| format!("de {} GB", gb_short(t))).unwrap_or_else(|| "GB".into())),
                    None => ("—".into(), "GB".into()),
                };
                painter.text(r.center() + vec2(0.0, -6.0), Align2::CENTER_CENTER, big, font(40.0, W::Display), p.text);
                painter.text(r.center() + vec2(0.0, 24.0), Align2::CENTER_CENTER, unit, font(12.5, W::Medium), p.text2);
                ui.add_space(10.0);
                let sub = match &m.volume {
                    Some(v) => {
                        let mut s = format!("refill abaixo de {} GB", gb_short(lim));
                        if let Some(pl) = v.plan_left {
                            s += &format!(" · plano {}{} GB", gb_short(pl), v.plan_total.map(|t| format!(" de {}", gb_short(t))).unwrap_or_default());
                        }
                        s
                    }
                    None if m.ligado => "esperando a primeira leitura".into(),
                    None => "monitor desligado".into(),
                };
                widgets::text(ui, sub, 12.0, W::Regular, p.text2);
            });
        });
    }

    fn stats_card(&mut self, ui: &mut Ui, st: &AppState, h: f32) {
        let p = self.pal;
        let m = &st.monitor;
        let agora = now();
        let offset = m.agora - agora; // relógio do serviço - daqui
        let t = agora + offset;
        let fase = if let Some(pend) = &m.pendente {
            format!("refill pedido às {} — confirmando pelo volume…", hhmm(pend.at))
        } else if let Some(u) = m.ultima_checagem {
            format!("última checagem às {}", hhmm(u))
        } else {
            String::new()
        };
        let (prox, prox_d) = if !m.ligado {
            ("—".to_string(), "monitor desligado".to_string())
        } else if m.pausado {
            ("pausado".into(), String::new())
        } else if m.fase == "checando" {
            ("agora".into(), m.motivo_intervalo.clone())
        } else if let Some(n) = m.proxima_checagem {
            let f = n - t;
            (if f <= 0.5 { "agora".into() } else { format!("em {}", dur(f)) }, m.motivo_intervalo.clone())
        } else {
            ("—".into(), String::new())
        };
        let (ult, ult_d) = match (&m.ultimo_refill, self.shared.lock().unwrap().hist24.as_ref()) {
            (Some(u), _) => (hhmm(u.t), last_detail(u.latencia, &u.why)),
            (None, Some(h)) => match h.refills.iter().rev().find(|r| r.ok) {
                Some(r) => (hhmm(r.t), last_detail(r.latencia, &r.why)),
                None => ("—".into(), String::new()),
            },
            _ => ("—".into(), String::new()),
        };
        let (tok, tok_d) = match m.token_expira {
            _ if m.precisa_login.is_some() => ("sem sessão".to_string(), "faça login acima".to_string()),
            Some(e) => {
                let f = e - t;
                if f > 0.0 { ("ativa".to_string(), format!("token renova em {}", dur((f - 300.0).max(0.0)))) } else { ("renovando".into(), String::new()) }
            }
            None => ("—".into(), String::new()),
        };
        let stats = [
            Stat { icon: ic::TIMER, tint: p.indigo, caption: "Próxima checagem", value: prox, detail: prox_d },
            Stat { icon: ic::PULSE, tint: p.orange, caption: "Ritmo de consumo",
                   value: m.ritmo_gb_h.map(|r| format!("{} GB/h", gb(r))).unwrap_or_else(|| "—".into()),
                   detail: "média dos últimos minutos".into() },
            Stat { icon: ic::ARROWS_CLOCKWISE, tint: p.green, caption: "Refills hoje",
                   value: m.refills_hoje.map(|n| n.to_string()).unwrap_or_else(|| "—".into()),
                   detail: if m.falhas_seguidas > 0 { format!("{} falha(s) seguida(s)", m.falhas_seguidas) } else { String::new() } },
            Stat { icon: ic::CHECK_CIRCLE, tint: p.teal, caption: "Último refill", value: ult, detail: ult_d },
            Stat { icon: ic::KEY, tint: p.purple, caption: "Sessão", value: tok, detail: tok_d },
            Stat { icon: ic::LOCK_SIMPLE, tint: if m.credenciais { p.accent } else { p.text3 }, caption: "Login automático",
                   value: if m.credenciais { "ligado" } else { "desligado" }.into(),
                   detail: if m.credenciais { "número e senha guardados" } else { "sem senha guardada" }.into() },
        ];
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(h - 36.0);
            let gap = 10.0;
            ui.spacing_mut().item_spacing.y = gap;
            card_header(ui, &p, "Agora", &fase);
            let cols = if ui.available_width() > 640.0 { 3 } else { 2 };
            let rows = stats.len().div_ceil(cols);
            let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
            let th = ((ui.available_height() - gap * (rows as f32 - 1.0)) / rows as f32).clamp(60.0, 110.0);
            for row in stats.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for s in row {
                        widgets::stat_tile(ui, &p, vec2(w, th), s);
                    }
                });
            }
        });
    }

    fn stress_card(&mut self, ui: &mut Ui, st: &AppState) {
        let p = self.pal;
        let e = &st.estresse;
        let m = &st.monitor;
        let running = e.estado == "rodando";
        let stopping = e.estado == "parando";
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(6), p.orange);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, ic::LIGHTNING, font(15.0, W::IconFill), Color32::WHITE);
                widgets::text(ui, "Teste de estresse da rede", 14.0, W::Semibold, p.text);
                if running {
                    widgets::pill(ui, &p, "baixando", p.orange);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        if widgets::button_icon(ui, &p, Some(ic::STOP), "Parar teste", Btn::Danger).clicked() {
                            self.act(json!({"acao": "estresse_parar"}));
                        }
                    } else if stopping {
                        ui.add_enabled_ui(false, |ui| widgets::button(ui, &p, "Parando…", Btn::Normal));
                    } else {
                        let resp = ui.add_enabled_ui(m.ligado, |ui| widgets::button_icon(ui, &p, Some(ic::PLAY), "Iniciar teste", Btn::Primary)).inner;
                        let resp = if m.ligado { resp } else { resp.on_disabled_hover_text("Ligue o monitor antes: sem ele ninguém faz o refill.") };
                        if resp.clicked() {
                            self.ask("Iniciar o teste de estresse?",
                                     &format!("Ele baixa o mais rápido possível e consome volume de verdade (até {} GB).", gb_short(e.cfg_gb)),
                                     "Iniciar", false, Then::Act(json!({"acao": "estresse_iniciar"})));
                        }
                    }
                });
            });
            widgets::text(ui, format!("Baixa o máximo que a rede aguentar, em várias conexões, pra ver o refill dando conta \
                                       de consumo pesado. Nada vai pro disco. Para sozinho em {} GB ou {} min (mude em Ajustes).",
                                      gb_short(e.cfg_gb), e.cfg_min), 12.0, W::Regular, p.text2);
            if e.inicio.is_none() {
                return;
            }
            ui.add_space(4.0);
            let frac = (if e.limite_bytes > 0 { e.bytes as f64 / e.limite_bytes as f64 } else { 0.0 })
                .max(if e.limite_segundos > 0 { e.segundos / e.limite_segundos as f64 } else { 0.0 });
            widgets::progress(ui, &p, frac as f32, if running { p.orange } else { p.green });
            ui.add_space(4.0);
            let lim = m.limite.unwrap_or(0.0);
            let min_d = match e.volume_minimo {
                None => String::new(),
                Some(v) if v <= 0.005 => "zerou — o refill não acompanhou".into(),
                Some(v) if v < lim => "passou um pouco do limite".into(),
                Some(_) => "sempre acima do limite".into(),
            };
            let stats = [
                Stat { icon: ic::SPEEDOMETER, tint: p.orange, caption: "Velocidade agora",
                       value: if running { format!("{} Mbit/s", e.mbps.round()) } else { "—".into() },
                       detail: format!("média {} · pico {}", e.media_mbps.round(), e.pico_mbps.round()) },
                Stat { icon: ic::DOWNLOAD_SIMPLE, tint: p.accent, caption: "Baixado", value: format!("{} GB", gb(e.bytes as f64 / 1e9)),
                       detail: format!("limite {} GB", gb_short(e.limite_bytes as f64 / 1e9)) },
                Stat { icon: ic::CLOCK, tint: p.indigo, caption: "Tempo", value: mmss(e.segundos),
                       detail: format!("limite {} min", (e.limite_segundos as f64 / 60.0).round()) },
                Stat { icon: ic::ARROWS_CLOCKWISE, tint: p.green, caption: "Refills no teste",
                       value: e.refills.map(|n| n.to_string()).unwrap_or_else(|| "—".into()),
                       detail: match (e.falhas, e.refills) {
                           (Some(f), _) if f > 0 => format!("{f} falha(s)!"),
                           (_, Some(r)) if r > 0 => "o Tasker deu conta".into(),
                           _ => String::new(),
                       } },
                Stat { icon: ic::ARROW_DOWN, tint: p.red, caption: "Volume mais baixo",
                       value: e.volume_minimo.map(|v| format!("{} GB", gb(v))).unwrap_or_else(|| "—".into()), detail: min_d },
                Stat { icon: ic::GLOBE, tint: p.teal, caption: "Conexões ativas",
                       value: if running { e.conexoes.to_string() } else { "—".into() },
                       detail: if e.erros > 0 { format!("{} erro(s) de servidor", e.erros) } else { String::new() } },
            ];
            let gap = 10.0;
            let cols = if ui.available_width() > 900.0 { 6 } else if ui.available_width() > 560.0 { 3 } else { 2 };
            let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
            for row in stats.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for s in row {
                        widgets::stat_tile(ui, &p, vec2(w, 78.0), s);
                    }
                });
            }
            if e.estado == "parado" {
                let ok = e.falhas.unwrap_or(0) == 0 && e.volume_minimo.is_some_and(|v| v > 0.005);
                let why = if e.motivo.is_empty() { "parado" } else { &e.motivo };
                let (tone, icon, msg) = if ok {
                    (Tone::Good, ic::CHECK_CIRCLE, "O refill acompanhou o consumo.")
                } else if e.falhas.unwrap_or(0) > 0 {
                    (Tone::Bad, ic::WARNING_CIRCLE, "Houve falha de refill — veja o Log.")
                } else if e.volume_minimo.is_none() {
                    (Tone::Info, ic::INFO, "")
                } else {
                    (Tone::Bad, ic::WARNING_CIRCLE, "O volume chegou a zerar — veja o Log.")
                };
                widgets::callout(ui, &p, tone, icon, &format!("Terminou ({why})"), msg, None);
            }
        });
    }

    fn chart_card(&mut self, ui: &mut Ui, st: &AppState) {
        let p = self.pal;
        let h = self.shared.lock().unwrap().hist24.clone();
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            card_header(ui, &p, "Últimas 24 horas", &h.as_ref().map(summary).unwrap_or_default());
            ui.add_space(4.0);
            match &h {
                Some(h) => charts::line_chart(ui, &p, Id::new("graf24"), 190.0, &ChartData {
                    volume: &h.volume, refills: &h.refills, hours: 24.0, limit: st.monitor.limite, now: st.monitor.agora.max(now()),
                }),
                None => {
                    ui.allocate_exact_size(vec2(ui.available_width(), 190.0), Sense::hover());
                }
            }
            charts::legend(ui, &p, &[(p.accent, "volume de refill"), (p.green, "refill entrou"), (p.red, "refill falhou")]);
        });
    }

    fn login_card(&mut self, ui: &mut Ui, st: &AppState) {
        let p = self.pal;
        let m = &st.monitor;
        let nav = &st.login_navegador;
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(9), p.accent);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, ic::USER_CIRCLE, font(22.0, W::IconFill), Color32::WHITE);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    widgets::text(ui, "Entrar na Lidl Connect", 15.0, W::Semibold, p.text);
                    let why = m.precisa_login.clone().unwrap_or_else(|| "Entre de novo pra trocar a sessão atual por uma nova.".into());
                    widgets::text(ui, why, 12.0, W::Regular, p.text2);
                });
                if self.force_login && m.precisa_login.is_none() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::button(ui, &p, "Cancelar", Btn::Normal).clicked() {
                            self.force_login = false;
                        }
                    });
                }
            });
            ui.add_space(6.0);
            let mut submit = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                widgets::field(ui, &p, Id::new("login-num"), &mut self.login_num,
                               FieldOpts { hint: "Número (Mobilfunknummer)", width: 220.0, icon: Some(ic::DEVICE_MOBILE), ..Default::default() });
                let r = widgets::field(ui, &p, Id::new("login-senha"), &mut self.login_pw,
                                       FieldOpts { hint: "Senha", width: 200.0, password: true, icon: Some(ic::KEY), ..Default::default() });
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if widgets::button(ui, &p, "Entrar", Btn::Primary).clicked() {
                    submit = true;
                }
            });
            widgets::checkbox(ui, &p, &mut self.login_remember, "Lembrar pro login automático");
            if submit {
                self.act(json!({"acao": "login_senha", "msisdn": self.login_num, "senha": self.login_pw, "lembrar": self.login_remember}));
                self.login_pw.clear();
                self.force_login = false;
            }
            ui.add_space(2.0);
            widgets::row_sep(ui, &p);
            ui.horizontal(|ui| {
                let busy = nav.estado == "abrindo" || nav.estado == "aguardando";
                ui.add_enabled_ui(st.sistema.navegador, |ui| {
                    let label = if busy { "Cancelar login pelo navegador" } else { "Entrar pelo navegador" };
                    let r = widgets::button_icon(ui, &p, Some(ic::BROWSER), label, Btn::Normal);
                    let r = if st.sistema.navegador { r } else { r.on_disabled_hover_text("Não achei Chrome, Edge nem Chromium instalado.") };
                    if r.clicked() {
                        self.act(json!({"acao": if busy { "login_cancelar" } else { "login_navegador" }}));
                    }
                });
                if busy {
                    widgets::spinner(ui, &p, 14.0);
                }
                let txt = if nav.estado != "parado" && !nav.msg.is_empty() {
                    nav.msg.clone()
                } else {
                    "Abre o site da Lidl numa janela à parte; faça login lá e o Tasker pega a sessão sozinho (serve quando a Lidl pede captcha).".into()
                };
                widgets::text(ui, txt, 12.0, W::Regular, p.text2);
            });
        });
    }

    // ============================================================= histórico

    pub fn page_historico(&mut self, ui: &mut Ui, st: &AppState) {
        let p = self.pal;
        let hours = HOURS[self.hist_sel];
        let h = {
            let s = self.shared.lock().unwrap();
            match &s.hist {
                Some((hh, h)) if *hh == hours => Some(h.clone()),
                _ if hours == 24 => s.hist24.clone(),
                _ => None,
            }
        };
        widgets::card(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::text(ui, "Volume de refill", 14.0, W::Semibold, p.text);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::segmented(ui, &p, Id::new("periodo"), &HOUR_LABELS, &mut self.hist_sel) {
                        let _ = self.tx.send(Cmd::LoadHistory(HOURS[self.hist_sel]));
                    }
                });
            });
            ui.add_space(6.0);
            match &h {
                Some(h) => charts::line_chart(ui, &p, Id::new("graf-hist"), 250.0, &ChartData {
                    volume: &h.volume, refills: &h.refills, hours: hours as f64, limit: st.monitor.limite, now: st.monitor.agora.max(now()),
                }),
                None => {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 250.0), Sense::hover());
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, "carregando…", font(12.5, W::Regular), p.text2);
                }
            }
            if let Some(h) = &h {
                ui.horizontal(|ui| {
                    charts::legend(ui, &p, &[(p.accent, "volume"), (p.green, "refill entrou"), (p.red, "falhou")]);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::text(ui, summary(h), 12.0, W::Regular, p.text2);
                    });
                });
            }
        });

        widgets::section_title(ui, &p, "Refills no período");
        widgets::group(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let items: Vec<_> = h.as_ref().map(|h| h.refills.iter().rev().take(150).cloned().collect()).unwrap_or_default();
            if items.is_empty() {
                ui.add_space(14.0);
                ui.vertical_centered(|ui| widgets::text(ui, "Nenhum refill no período", 13.0, W::Regular, p.text2));
                ui.add_space(14.0);
                return;
            }
            let t_now = now();
            for (i, r) in items.iter().enumerate() {
                if i > 0 {
                    widgets::row_sep(ui, &p);
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
                if resp.hovered() {
                    ui.painter().rect_filled(rect.expand2(vec2(10.0, 0.0)), CornerRadius::same(4), p.hover);
                }
                let painter = ui.painter();
                let (icon, c) = if r.ok { (ic::CHECK_CIRCLE, p.green) } else { (ic::X_CIRCLE, p.red) };
                painter.text(pos2(rect.left() + 9.0, rect.center().y), Align2::CENTER_CENTER, icon, font(17.0, W::IconFill), c);
                painter.text(pos2(rect.left() + 26.0, rect.center().y), Align2::LEFT_CENTER, charts::day_time(r.t, t_now),
                             font(13.0, W::Medium), p.text);
                let g = match (r.before, r.after) {
                    (Some(b), Some(a)) => format!("{} → {} GB", gb(b), gb(a)),
                    (Some(b), None) => format!("com {} GB", gb(b)),
                    _ => String::new(),
                };
                let desc = if r.ok {
                    let mut d = if g.is_empty() { "refill".into() } else { g };
                    if !r.why.is_empty() {
                        d += &format!(" — {}", r.why);
                    }
                    d
                } else if g.is_empty() {
                    "não entrou".into()
                } else {
                    format!("não entrou ({g})")
                };
                painter.text(pos2(rect.left() + 130.0, rect.center().y), Align2::LEFT_CENTER, desc, font(13.0, W::Regular), p.text);
                if let Some(l) = r.latencia.filter(|_| r.ok) {
                    let pill = format!("{} s", l.round());
                    let g = painter.layout_no_wrap(pill, font(11.5, W::Medium), p.text2);
                    let br = Rect::from_min_size(pos2(rect.right() - g.size().x - 14.0, rect.center().y - 10.0), vec2(g.size().x + 14.0, 20.0));
                    painter.rect_filled(br, CornerRadius::same(10), p.tile);
                    painter.galley(br.center() - g.size() / 2.0, g, p.text2);
                }
            }
        });
    }

    // =================================================================== log

    pub fn page_log(&mut self, ui: &mut Ui, focus_search: bool) {
        let p = self.pal;
        let (lines, pasta, windows) = {
            let s = self.shared.lock().unwrap();
            let f = self.log_filter.trim().to_lowercase();
            let lines: Vec<String> = if f.is_empty() {
                s.log.clone()
            } else {
                s.log.iter().filter(|l| l.to_lowercase().contains(&f)).cloned().collect()
            };
            let sis = s.state.as_ref().map(|x| (x.sistema.pasta.clone(), x.sistema.windows)).unwrap_or_default();
            (lines, sis.0, sis.1)
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let r = widgets::field(ui, &p, Id::new("filtro-log"), &mut self.log_filter,
                                   FieldOpts { hint: "Filtrar (ex.: refill)", width: 260.0, icon: Some(ic::MAGNIFYING_GLASS), ..Default::default() });
            if focus_search {
                r.request_focus();
            }
            widgets::checkbox(ui, &p, &mut self.log_follow, "Acompanhar o fim");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let sep = if windows { "\\" } else { "/" };
                widgets::text_trunc(ui, format!("completo em {pasta}{sep}tasker.log"), 11.5, W::Regular, p.text3);
            });
        });
        ui.add_space(10.0);
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect(rect, CornerRadius::same(10), p.console, Stroke::new(1.0, p.card_line), egui::StrokeKind::Inside);
        let inner = rect.shrink2(vec2(12.0, 8.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        let row_h = 18.0;
        egui::ScrollArea::both()
            .id_salt("log-rolagem")
            .auto_shrink([false, false])
            .stick_to_bottom(self.log_follow)
            .show_rows(&mut child, row_h, lines.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for line in &lines[range] {
                    let (stamp, msg) = match line.find("] ") {
                        Some(i) if line.starts_with('[') => (&line[..=i], &line[i + 1..]),
                        _ => ("", line.as_str()),
                    };
                    let color = line_color(&p, msg);
                    let mut job = egui::text::LayoutJob::default();
                    job.append(stamp, 0.0, egui::TextFormat { font_id: font(12.0, W::Mono), color: p.text3, ..Default::default() });
                    job.append(msg, 0.0, egui::TextFormat { font_id: font(12.0, W::Mono), color, ..Default::default() });
                    job.wrap.max_width = f32::INFINITY;
                    let g = ui.painter().layout_job(job);
                    let (r, _) = ui.allocate_exact_size(vec2(g.size().x.max(ui.available_width()), row_h), Sense::hover());
                    ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text);
                }
                if lines.is_empty() {
                    widgets::text(ui, "nada por aqui ainda", 12.0, W::Regular, p.text3);
                }
            });
    }

    // =============================================================== ajustes

    pub fn page_ajustes(&mut self, ui: &mut Ui, st: &AppState) {
        let p = self.pal;
        let (payload, gen, errors) = {
            let s = self.shared.lock().unwrap();
            (s.settings.clone(), s.settings_gen, s.save_errors.clone())
        };
        let Some(payload) = payload else {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| widgets::spinner(ui, &p, 20.0));
            if self.form_gen == 0 {
                let _ = self.tx.send(Cmd::LoadSettings);
                self.form_gen = u64::MAX; // pediu; espera chegar
            }
            return;
        };
        if !errors.is_empty() {
            self.form_saving = false; // recusado: o que a pessoa digitou continua na tela
        }
        if gen != self.form_gen && (!self.form_dirty() || self.form_saving) {
            self.load_form(&payload);
            self.form_gen = gen;
            self.form_saving = false;
        }
        for warn in &payload.avisos {
            widgets::callout(ui, &p, Tone::Warn, ic::WARNING, warn, "", None);
        }
        for group in &payload.grupos {
            widgets::section_title(ui, &p, group);
            widgets::group(&p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                let fields: Vec<_> = payload.campos.iter().filter(|c| &c.group == group).collect();
                for (i, c) in fields.iter().enumerate() {
                    if i > 0 {
                        widgets::row_sep(ui, &p);
                    }
                    let locked = c.origem == "ambiente";
                    let err = errors.get(&c.key).cloned();
                    let note = if locked { Some((format!("Definido pela variável {} — o ambiente tem a última palavra.", c.env), p.orange)) }
                               else { err.clone().map(|e| (e, p.red)) };
                    let ctl_w = if c.kind == "str" { 250.0 } else if c.kind == "bool" { 40.0 } else { 160.0 };
                    settings_row(ui, &p, &c.label, &c.help, note, ctl_w, |ui| {
                        ui.add_enabled_ui(!locked, |ui| match c.kind.as_str() {
                            "bool" => {
                                let v = self.form_bool.entry(c.key.clone()).or_insert(false);
                                widgets::toggle(ui, &p, v);
                            }
                            "str" => {
                                let v = self.form.entry(c.key.clone()).or_default();
                                widgets::field(ui, &p, Id::new(("cfg", &c.key)), v,
                                               FieldOpts { width: 250.0, invalid: err.is_some(), hint: "vazio", ..Default::default() });
                            }
                            _ => {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let (r, _) = ui.allocate_exact_size(vec2(58.0, 20.0), Sense::hover());
                                    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, &c.unit_label, font(12.0, W::Regular), p.text2);
                                    let v = self.form.entry(c.key.clone()).or_default();
                                    let tip = match (&c.min, &c.max) {
                                        (Some(a), Some(b)) => format!("de {} a {} · padrão {}", num_text(a), num_text(b), num_text(&c.padrao)),
                                        _ => String::new(),
                                    };
                                    widgets::field(ui, &p, Id::new(("cfg", &c.key)), v,
                                                   FieldOpts { width: 86.0, align_right: true, invalid: err.is_some(), ..Default::default() })
                                        .on_hover_text(tip);
                                });
                            }
                        });
                    });
                }
            });
        }

        // ---- conta
        widgets::section_title(ui, &p, "Conta");
        let m = &st.monitor;
        widgets::group(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let cred = if m.credenciais { "Há número e senha guardados." } else { "Nada guardado." };
            settings_row(ui, &p, "Número e senha guardados",
                         "Com eles o Tasker refaz o login sozinho quando a sessão cai. Ficam em dados/credenciais.json, \
                          legível só pelo seu usuário (é texto puro). Errar a senha várias vezes trava a conta na Lidl — \
                          por isso há o limite de tentativas em \"Login automático\".",
                         Some((cred.into(), p.text2)), 90.0, |ui| {
                ui.add_enabled_ui(m.credenciais, |ui| {
                    if widgets::button(ui, &p, "Apagar", Btn::Danger).clicked() {
                        self.ask("Apagar número e senha?", "O login automático para de funcionar até você guardar de novo.",
                                 "Apagar", true, Then::Act(json!({"acao": "credenciais_apagar"})));
                    }
                });
            });
            widgets::row_sep(ui, &p);
            settings_row(ui, &p, "Fazer login agora", "Troca a sessão atual por uma nova (útil depois de mudar a senha).", None, 110.0, |ui| {
                if widgets::button(ui, &p, "Abrir login", Btn::Normal).clicked() {
                    self.force_login = true;
                    self.go(Page::Painel);
                }
            });
        });

        // ---- sistema
        widgets::section_title(ui, &p, "Sistema");
        let sis = &st.sistema;
        widgets::group(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            settings_row(ui, &p, "Iniciar junto com o computador",
                         "O Tasker sobe em segundo plano quando você entra na sua conta. O atalho continua abrindo esta janela.",
                         None, 40.0, |ui| {
                let mut on = sis.autostart;
                if widgets::toggle(ui, &p, &mut on).changed() {
                    self.act(json!({"acao": "autostart", "ligado": on}));
                }
            });
            widgets::row_sep(ui, &p);
            settings_row(ui, &p, "Atalho na área de trabalho e no menu",
                         "Um clique abre o Tasker (ou traz esta janela pra frente, se ela já estiver aberta).", None, 120.0, |ui| {
                if widgets::button(ui, &p, "Criar atalho", Btn::Normal).clicked() {
                    self.act(json!({"acao": "atalho"}));
                }
            });
            widgets::row_sep(ui, &p);
            settings_row(ui, &p, "Monitor", "Desligar para de vigiar o volume sem fechar o Tasker. Fechar a janela não desliga nada.",
                         None, 140.0, |ui| {
                let (label, act) = if m.ligado { ("Desligar monitor", "desligar") } else { ("Ligar monitor", "ligar") };
                if widgets::button(ui, &p, label, Btn::Normal).clicked() {
                    self.act(json!({"acao": act}));
                }
            });
            widgets::row_sep(ui, &p);
            settings_row(ui, &p, "Avisos", "Dispara uma notificação de teste (sistema, ntfy e webhook, o que estiver ligado).", None, 120.0, |ui| {
                if widgets::button(ui, &p, "Testar aviso", Btn::Normal).clicked() {
                    self.act(json!({"acao": "testar_aviso"}));
                }
            });
            widgets::row_sep(ui, &p);
            settings_row(ui, &p, "Encerrar o Tasker", &format!("Versão {} · dados em {}.", sis.versao, sis.pasta), None, 90.0, |ui| {
                if widgets::button(ui, &p, "Sair", Btn::Danger).clicked() {
                    self.ask("Encerrar o Tasker?", "Ninguém vai vigiar o volume até você abrir de novo.", "Encerrar", true,
                             Then::Act(json!({"acao": "sair"})));
                }
            });
        });

        // ---- aparência (só da janela, gravada em dados/janela.json)
        widgets::section_title(ui, &p, "Aparência");
        widgets::group(&p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            settings_row(ui, &p, "Tema", "Automático segue o tema do sistema.", None, 230.0, |ui| {
                let mut i = match self.prefs.theme {
                    ThemePref::Auto => 0,
                    ThemePref::Light => 1,
                    ThemePref::Dark => 2,
                };
                if widgets::segmented(ui, &p, Id::new("tema"), &["Automático", "Claro", "Escuro"], &mut i) {
                    self.prefs.theme = [ThemePref::Auto, ThemePref::Light, ThemePref::Dark][i];
                    self.prefs.save(&self.paths);
                }
            });
            widgets::row_sep(ui, &p);
            settings_row(ui, &p, "Barra de título do sistema",
                         "Usa a moldura do sistema em vez da própria (cantos arredondados e semáforo). Vale na próxima vez que a janela abrir.",
                         None, 40.0, |ui| {
                let mut on = self.prefs.system_frame;
                if widgets::toggle(ui, &p, &mut on).changed() {
                    self.prefs.system_frame = on;
                    self.prefs.save(&self.paths);
                }
            });
        });
    }

    fn load_form(&mut self, payload: &crate::types::SettingsPayload) {
        self.form.clear();
        self.form_bool.clear();
        for c in &payload.campos {
            match &c.valor {
                Value::Bool(b) => {
                    self.form_bool.insert(c.key.clone(), *b);
                }
                v => {
                    self.form.insert(c.key.clone(), num_text(v));
                }
            }
        }
        self.form_base = (self.form.clone(), self.form_bool.clone());
    }

    pub fn form_dirty(&self) -> bool {
        self.form_gen != 0 && self.form_gen != u64::MAX && (self.form != self.form_base.0 || self.form_bool != self.form_base.1)
    }

    pub fn save_form(&mut self) {
        let locked: Vec<String> = self.shared.lock().unwrap().settings.as_ref()
            .map(|s| s.campos.iter().filter(|c| c.origem == "ambiente").map(|c| c.key.clone()).collect())
            .unwrap_or_default();
        let mut valores = serde_json::Map::new();
        for (k, v) in &self.form {
            if !locked.contains(k) {
                valores.insert(k.clone(), Value::from(v.clone()));
            }
        }
        for (k, v) in &self.form_bool {
            if !locked.contains(k) {
                valores.insert(k.clone(), Value::from(*v));
            }
        }
        // Deu certo: o serviço manda os ajustes de novo e o formulário recarrega.
        self.form_saving = true;
        self.shared.lock().unwrap().save_errors.clear();
        let _ = self.tx.send(Cmd::SaveSettings(valores));
    }
}

fn last_detail(lat: Option<f64>, why: &str) -> String {
    let mut s = lat.map(|l| format!("entrou em {} s", l.round())).unwrap_or_default();
    if !why.is_empty() {
        if !s.is_empty() {
            s += " · ";
        }
        s += why;
    }
    s
}

fn num_text(v: &Value) -> String {
    match v {
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => comma(fmt_g(n.as_f64().unwrap_or(0.0))),
        },
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn line_color(p: &super::theme::Pal, l: &str) -> Color32 {
    let low = l.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| low.contains(w));
    if any(&["falh", "erro", "não consegui", "recusad", "caiu"]) || l.contains("NÃO") {
        p.red
    } else if any(&["confirmado", "renovado", "login por senha ok", "aceito", "sessão nova"]) {
        p.green
    } else if any(&["aviso", "pausa", "cooldown", "espera"]) {
        p.orange
    } else {
        p.text
    }
}

/// Linha de ajuste: rótulo + explicação à esquerda, controle à direita.
fn settings_row(ui: &mut Ui, p: &super::theme::Pal, label: &str, help: &str, note: Option<(String, Color32)>, ctl_w: f32,
                add: impl FnOnce(&mut Ui)) {
    ui.add_space(10.0);
    ui.horizontal_top(|ui| {
        let w = (ui.available_width() - ctl_w - 24.0).max(160.0);
        ui.vertical(|ui| {
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 2.0;
            widgets::text(ui, label, 13.0, W::Medium, p.text);
            if !help.is_empty() {
                widgets::text(ui, help, 11.5, W::Regular, p.text2);
            }
            if let Some((n, c)) = note {
                widgets::text(ui, n, 11.5, W::Medium, c);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            ui.add_space(0.0);
            ui.allocate_ui_with_layout(vec2(ctl_w, 26.0), egui::Layout::right_to_left(egui::Align::Center), add);
        });
    });
    ui.add_space(10.0);
}
