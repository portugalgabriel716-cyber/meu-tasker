//! Ponte entre a janela e o serviço: uma thread busca o estado (1x/s), o log, o
//! histórico e os ajustes, e executa as ações. A janela só lê o que já chegou —
//! nunca espera rede no meio de um quadro.
use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::client::Client;
use crate::paths::Paths;
use crate::types::{ActionResult, AppState, History, SettingsPayload};
use crate::util::now;

pub enum Cmd {
    Action(Value),
    SaveSettings(serde_json::Map<String, Value>),
    LoadSettings,
    LoadHistory(i64),
    /// Sobe o serviço de novo (botão "Abrir o Tasker" quando ele caiu).
    StartService,
    /// Encerra o serviço da versão em Python e sobe este no lugar.
    Migrate,
}

#[derive(Default)]
pub struct Shared {
    pub state: Option<AppState>,
    /// Instante da última resposta boa do serviço.
    pub last_ok: f64,
    pub connected: bool,
    pub ever_connected: bool,
    pub hist24: Option<History>,
    pub hist: Option<(i64, History)>,
    pub log: Vec<String>,
    pub log_seq: u64,
    pub settings: Option<SettingsPayload>,
    /// Sobe a cada carga de ajustes (a tela recarrega o formulário).
    pub settings_gen: u64,
    pub save_errors: BTreeMap<String, String>,
    pub toasts: VecDeque<(String, f64)>,
    pub busy: usize,
    /// O serviço confirmou o "Sair": a janela fecha.
    pub quit: bool,
    pub starting: bool,
    pub last_action: Option<(String, ActionResult)>,
}

impl Shared {
    pub fn toast(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if msg.is_empty() || msg == "ok" {
            return;
        }
        self.toasts.push_back((msg, now()));
        while self.toasts.len() > 3 {
            self.toasts.pop_front();
        }
    }
}

pub fn spawn(paths: Arc<Paths>, shared: Arc<Mutex<Shared>>, rx: Receiver<Cmd>, ctx: egui::Context) {
    std::thread::Builder::new()
        .name("janela-dados".into())
        .spawn(move || worker(paths, shared, rx, ctx))
        .unwrap();
}

fn worker(paths: Arc<Paths>, shared: Arc<Mutex<Shared>>, rx: Receiver<Cmd>, ctx: egui::Context) {
    let mut client: Option<Client> = None;
    let mut last_hist = Instant::now() - Duration::from_secs(3600);
    let mut tick = Instant::now() - Duration::from_secs(10);
    loop {
        // Ações têm prioridade; sem ação, acorda a cada segundo pra puxar o estado.
        let wait = Duration::from_millis(1000).saturating_sub(tick.elapsed());
        match rx.recv_timeout(wait) {
            Ok(cmd) => {
                if client.is_none() {
                    client = Client::find(&paths);
                }
                run_cmd(&paths, &shared, client.as_ref(), cmd, &ctx);
                ctx.request_repaint();
                tick = Instant::now() - Duration::from_secs(10); // estado novo já
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        tick = Instant::now();
        // tela.json pode mudar (serviço reiniciado): relê se a conexão falhar.
        let found = Client::find(&paths);
        if let (Some(f), Some(c)) = (&found, &client) {
            if !f.same_as(c) {
                client = found.clone();
            }
        } else if client.is_none() {
            client = found;
        }
        let Some(c) = client.clone() else {
            set_disconnected(&shared);
            ctx.request_repaint();
            continue;
        };
        match c.get::<AppState>("/api/estado") {
            Ok(st) => {
                let since = shared.lock().unwrap().log_seq;
                let log = c.get::<crate::logs::LogChunk>(&format!("/api/log?desde={since}")).ok();
                let mut s = shared.lock().unwrap();
                s.state = Some(st);
                s.connected = true;
                s.ever_connected = true;
                s.starting = false;
                s.last_ok = now();
                if let Some(l) = log {
                    if l.reset {
                        s.log.clear();
                    }
                    s.log.extend(l.linhas);
                    let extra = s.log.len().saturating_sub(3000);
                    s.log.drain(..extra);
                    s.log_seq = l.seq;
                }
            }
            Err(_) => {
                client = None;
                set_disconnected(&shared);
            }
        }
        if client.is_some() && (last_hist.elapsed() > Duration::from_secs(30) || shared.lock().unwrap().hist24.is_none()) {
            if let Ok(h) = c.get::<History>("/api/historico?horas=24") {
                shared.lock().unwrap().hist24 = Some(h);
                last_hist = Instant::now();
            }
            let hours = shared.lock().unwrap().hist.as_ref().map(|(h, _)| *h);
            if let Some(hours) = hours.filter(|h| *h != 24) {
                if let Ok(h) = c.get::<History>(&format!("/api/historico?horas={hours}")) {
                    shared.lock().unwrap().hist = Some((hours, h));
                }
            }
        }
        ctx.request_repaint();
    }
}

fn set_disconnected(shared: &Arc<Mutex<Shared>>) {
    let mut s = shared.lock().unwrap();
    s.connected = false;
}

fn start_service(paths: &Paths, shared: &Arc<Mutex<Shared>>) {
    shared.lock().unwrap().starting = true;
    let _ = crate::system::spawn_detached(&crate::system::exe_path(), &["--fundo"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if Client::find(paths).is_some_and(|c| c.alive()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    shared.lock().unwrap().starting = false;
}

/// A versão antiga solta o lock da sessão ao sair; só então a nova pode subir.
fn migrate(paths: &Paths, shared: &Arc<Mutex<Shared>>, client: Option<&Client>) {
    let Some(c) = client else { return };
    if c.post::<Value>("/api/acao", &json!({"acao": "sair"})).is_err() {
        shared.lock().unwrap().toast("Não consegui falar com o serviço antigo.");
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(40);
    while crate::store::instance_running(&paths.lock_file).0 {
        if Instant::now() > deadline {
            shared.lock().unwrap().toast("O serviço antigo não encerrou. Tente de novo.");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    start_service(paths, shared);
    shared.lock().unwrap().toast("Pronto: o serviço novo assumiu.");
}

fn run_cmd(paths: &Paths, shared: &Arc<Mutex<Shared>>, client: Option<&Client>, cmd: Cmd, ctx: &egui::Context) {
    match cmd {
        Cmd::StartService => return start_service(paths, shared),
        Cmd::Migrate => return migrate(paths, shared, client),
        _ => {}
    }
    let Some(c) = client else {
        shared.lock().unwrap().toast("O serviço do Tasker não está respondendo.");
        return;
    };
    shared.lock().unwrap().busy += 1;
    ctx.request_repaint();
    match cmd {
        Cmd::Action(body) => {
            let name = body.get("acao").and_then(Value::as_str).unwrap_or("").to_string();
            match c.post::<ActionResult>("/api/acao", &body) {
                Ok(r) => {
                    let mut s = shared.lock().unwrap();
                    s.toast(r.text());
                    if name == "sair" {
                        s.quit = true;
                    }
                    s.last_action = Some((name, r));
                }
                Err(e) => shared.lock().unwrap().toast(format!("Falhou: {e}")),
            }
        }
        Cmd::SaveSettings(valores) => match c.post::<ActionResult>("/api/ajustes", &json!({"valores": valores})) {
            Ok(r) => {
                let ok = r.ok == Some(true);
                {
                    let mut s = shared.lock().unwrap();
                    s.toast(r.text());
                    s.save_errors = r.erros.clone().unwrap_or_default();
                }
                if ok {
                    load_settings(shared, c);
                }
            }
            Err(e) => shared.lock().unwrap().toast(format!("Falhou: {e}")),
        },
        Cmd::LoadSettings => load_settings(shared, c),
        Cmd::LoadHistory(hours) => {
            if let Ok(h) = c.get::<History>(&format!("/api/historico?horas={hours}")) {
                let mut s = shared.lock().unwrap();
                if hours == 24 {
                    s.hist24 = Some(h.clone());
                }
                s.hist = Some((hours, h));
            }
        }
        Cmd::StartService | Cmd::Migrate => {}
    }
    let mut s = shared.lock().unwrap();
    s.busy = s.busy.saturating_sub(1);
}

fn load_settings(shared: &Arc<Mutex<Shared>>, c: &Client) {
    if let Ok(p) = c.get::<SettingsPayload>("/api/ajustes") {
        let mut s = shared.lock().unwrap();
        s.settings = Some(p);
        s.settings_gen += 1;
    }
}
