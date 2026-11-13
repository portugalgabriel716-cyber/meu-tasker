//! O serviço: motor + API local, num processo que fica rodando sem janela.
//!
//! A janela é outro processo (o mesmo executável) que conversa com este pela API
//! local. Fechar a janela não para nada; clicar no atalho de novo só abre outra.
//!
//! Segurança (igual à versão 2): só escuta em 127.0.0.1, recusa Host que não
//! seja local (DNS rebinding) e toda rota exige a chave no CABEÇALHO — a chave
//! fica em dados/tela.json, legível só pelo seu usuário.
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::browser_login::{BrowserLogin, Tokens};
use crate::config::{self, Kind};
use crate::engine::{watch_control_files, Monitor};
use crate::history::{self, clear_pause, pause_state, refills_since, set_pause};
use crate::lidl::{LidlApi, PORTAL_URL};
use crate::logs::Logger;
use crate::notify::Notifier;
use crate::paths::Paths;
use crate::store::{self, write_json};
use crate::stress::StressTest;
use crate::system;
use crate::types::{ActionResult, AppState, Field, SettingsPayload, SysInfo};
use crate::util::{const_eq, env_get, fmt_local, random_token, Env};

pub struct App {
    pub paths: Arc<Paths>,
    pub env: Env,
    pub log: Arc<Logger>,
    pub monitor: Arc<Monitor>,
    pub login: Arc<BrowserLogin>,
    pub stress: Arc<StressTest>,
    browser: bool,
    today_cache: Mutex<(Option<std::time::SystemTime>, String, i64)>,
    focus: AtomicU64,
    quit: AtomicBool,
    pub key: String,
    port: Mutex<u16>,
}

impl App {
    pub fn new(paths: Arc<Paths>, env: Env, log: Arc<Logger>, api: Option<LidlApi>) -> Arc<App> {
        let monitor = Monitor::new(paths.clone(), env.clone(), log.clone(), api);
        let m2 = monitor.clone();
        monitor.set_notifier(Arc::new(Notifier::new(Arc::new(move || m2.cfg()), Some(log.clone()),
                                                    env_get(&env, "TASKER_NTFY_BASE"))));
        let m3 = monitor.clone();
        let on_tokens = Arc::new(move |t: Tokens| {
            m3.adopt_tokens(&t.access, t.refresh.as_deref(), t.expires_at_ms.map(|ms| ms / 1000.0), "navegador");
        });
        let url = env_get(&env, "TASKER_LOGIN_URL").unwrap_or(PORTAL_URL).to_string();
        let login = BrowserLogin::new(log.clone(), env.clone(), on_tokens, &url);
        let m4 = monitor.clone();
        let stress = StressTest::new(log.clone(), Arc::new(move || m4.status().volume.map(|v| v.left)));
        Arc::new(App {
            browser: system::find_browser(&env).is_some(),
            paths,
            env,
            log,
            monitor,
            login,
            stress,
            today_cache: Mutex::new((None, String::new(), 0)),
            focus: AtomicU64::new(0),
            quit: AtomicBool::new(false),
            key: random_token(24),
            port: Mutex::new(0),
        })
    }

    pub fn port(&self) -> u16 {
        *self.port.lock().unwrap()
    }

    fn refills_today(&self) -> i64 {
        let mtime = std::fs::metadata(&self.paths.refills_file).and_then(|m| m.modified()).ok();
        let day = fmt_local(crate::util::now(), "%Y-%m-%d");
        let mut c = self.today_cache.lock().unwrap();
        if c.0 != mtime || c.1 != day {
            *c = (mtime, day, history::history(&self.paths, 24).resumo.refills_hoje);
        }
        c.2
    }

    pub fn state(&self) -> AppState {
        let cfg = self.monitor.cfg();
        let mut snap = self.monitor.snapshot();
        snap.refills_hoje = Some(self.refills_today());
        snap.limite = Some(cfg.gb_threshold);
        let mut stress = self.stress.info();
        stress.cfg_gb = cfg.stress_max_gb;
        stress.cfg_min = (cfg.stress_max_minutes / 60.0) as i64;
        if let Some(t0) = stress.inicio {
            let (ok, fail) = refills_since(&self.paths, t0);
            stress.refills = Some(ok);
            stress.falhas = Some(fail);
        }
        AppState {
            monitor: snap,
            login_navegador: self.login.info(),
            estresse: stress,
            sistema: SysInfo {
                versao: crate::VERSION.into(),
                autostart: system::autostart_enabled(&self.env),
                navegador: self.browser,
                pasta: self.paths.data_dir.to_string_lossy().into_owned(),
                windows: store::IS_WINDOWS,
                foco: self.focus.load(Ordering::SeqCst),
            },
        }
    }

    pub fn settings_payload(&self) -> SettingsPayload {
        let (values, sources) = config::load_with_sources(&self.paths, &self.env);
        let campos = config::SETTINGS
            .iter()
            .map(|spec| {
                let num = matches!(spec.kind, Kind::Int | Kind::Float);
                let v = &values[spec.key];
                let bound = |x: f64| {
                    let val = if spec.kind == Kind::Int { config::Val::Int(x as i64) } else { config::Val::Float(x) };
                    config::to_display(spec, &val)
                };
                Field {
                    key: spec.key.into(),
                    label: spec.label.into(),
                    help: spec.help.into(),
                    group: spec.group.into(),
                    kind: spec.kind.name().into(),
                    env: spec.env.into(),
                    unit_label: config::unit_label(spec.unit).into(),
                    origem: sources[spec.key].into(),
                    valor: config::to_display(spec, v),
                    padrao: config::to_display(spec, &spec.default_val()),
                    min: num.then(|| bound(spec.min)),
                    max: num.then(|| bound(spec.max)),
                }
            })
            .collect();
        SettingsPayload {
            grupos: config::groups().into_iter().map(String::from).collect(),
            campos,
            avisos: config::warnings(&values),
        }
    }

    pub fn save_settings(&self, sent: &Map<String, Value>) -> ActionResult {
        let mut keep = config::Values::new();
        let mut errors = std::collections::BTreeMap::new();
        for spec in config::SETTINGS {
            if let Some(v) = sent.get(spec.key) {
                match config::from_display(spec, v) {
                    Ok(x) => {
                        keep.insert(spec.key.to_string(), x);
                    }
                    Err(e) => {
                        errors.insert(spec.key.to_string(), e);
                    }
                }
            }
        }
        if !errors.is_empty() {
            return ActionResult { ok: Some(false), msg: Some("Tem campo fora do limite — nada foi gravado.".into()),
                                  erros: Some(errors), ..Default::default() };
        }
        // O que já estava no arquivo (e é válido) continua; o que veio da tela vence.
        let mut fin = config::Values::new();
        for (k, raw) in config::read_file(&self.paths) {
            if let Some(spec) = config::spec(&k) {
                if let Ok(v) = config::coerce(spec, &raw) {
                    fin.insert(k, v);
                }
            }
        }
        fin.extend(keep);
        if let Err(e) = config::save(&self.paths, &fin) {
            return ActionResult::ok(false, format!("Não consegui gravar: {e}"));
        }
        self.monitor.reload_config();
        ActionResult { ok: Some(true), msg: Some("Ajustes gravados e já valendo.".into()),
                       erros: Some(Default::default()), ..Default::default() }
    }

    pub fn action(self: &Arc<Self>, body: &Value) -> ActionResult {
        let act = body.get("acao").and_then(Value::as_str).unwrap_or("");
        let m = &self.monitor;
        let s = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match act {
            "checar" => {
                if !m.running() {
                    return ActionResult::msg("O monitor está desligado — ligue primeiro.");
                }
                if pause_state(&self.paths).0 {
                    return ActionResult::msg("Está pausado — retome primeiro.");
                }
                m.poke();
                ActionResult::msg("Checando agora…")
            }
            "pausar" => {
                let minutes = body.get("minutos").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
                let until = set_pause(&self.paths, minutes);
                m.poke();
                let when = if until > 0.0 { fmt_local(until, "%H:%M") } else { "você retomar".into() };
                ActionResult::msg(format!("Pausado até {when}: o Tasker não toca na sessão — pode usar a conta \
                                           no celular à vontade."))
            }
            "retomar" => {
                clear_pause(&self.paths);
                m.poke();
                ActionResult::msg("Retomando.")
            }
            "ligar" => ActionResult::msg(if m.start() { "Monitor ligado." } else { "Já estava ligado." }),
            "desligar" => {
                let m = m.clone();
                std::thread::spawn(move || m.stop(Duration::from_secs(25)));
                ActionResult::msg("Desligando o monitor (o Tasker continua aberto).")
            }
            "testar_aviso" => {
                let n = m.notifier().map(|n| n.send("Teste do Tasker: se você está lendo isto, os avisos funcionam."));
                ActionResult::msg(if n.unwrap_or(0) > 0 { "Aviso de teste disparado — chegou?" }
                                  else { "Os avisos estão todos desligados nos ajustes." })
            }
            "login_senha" => {
                let msisdn = s("msisdn").trim().to_string();
                let senha = s("senha");
                if msisdn.is_empty() || senha.is_empty() {
                    return ActionResult::ok(false, "Preencha número e senha.");
                }
                let (ok, mut msg) = m.password_login(false, Some((msisdn.clone(), senha.clone())));
                if ok && body.get("lembrar").and_then(Value::as_bool).unwrap_or(false) {
                    match store::save_credentials(&self.paths, &msisdn, &senha) {
                        Ok(()) => msg += " Número e senha guardados pro login automático.",
                        Err(e) => msg += &format!(" (Não consegui guardar a senha: {e}.)"),
                    }
                }
                ActionResult::ok(ok, msg)
            }
            "login_navegador" => ActionResult::msg(self.login.start()),
            "login_cancelar" => {
                self.login.cancel();
                ActionResult::msg("Cancelando…")
            }
            "credenciais_apagar" => ActionResult::msg(if store::forget_credentials(&self.paths) {
                "Número e senha apagados."
            } else {
                "Não havia nada guardado."
            }),
            "padroes" => {
                if let Err(e) = config::save(&self.paths, &config::defaults()) {
                    return ActionResult::msg(format!("Não consegui gravar: {e}"));
                }
                m.reload_config();
                ActionResult::msg("Ajustes de volta ao padrão.")
            }
            "autostart" => {
                let on = body.get("ligado").and_then(Value::as_bool).unwrap_or(false);
                match system::set_autostart(&self.env, on) {
                    Ok(msg) => ActionResult::msg(msg),
                    Err(e) => ActionResult::msg(format!("Não consegui mudar: {e}")),
                }
            }
            "atalho" => {
                let made = system::create_shortcuts(&self.env);
                ActionResult::msg(if made.is_empty() {
                    "Não consegui criar o atalho.".to_string()
                } else {
                    format!("Atalho criado: {}", made.join(" · "))
                })
            }
            "estresse_iniciar" => {
                let c = m.cfg();
                let msg = self.stress.start(c.stress_streams, c.stress_max_gb, c.stress_max_minutes, &c.stress_url);
                m.poke(); // leitura nova já, pro ritmo de consumo começar a contar
                ActionResult::msg(msg)
            }
            "estresse_parar" => ActionResult::msg(self.stress.stop("parado por você")),
            "focar" => {
                self.focus.fetch_add(1, Ordering::SeqCst);
                ActionResult::msg("ok")
            }
            "sair" => {
                let me = Arc::clone(self);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    me.shutdown();
                });
                ActionResult::msg("Encerrando o Tasker…")
            }
            _ => ActionResult { erro: Some("Ação desconhecida.".into()), ..Default::default() },
        }
    }

    // -- execução ------------------------------------------------------------

    pub fn shutdown(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    pub fn quitting(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }

    /// Sobe o serviço e fica nele até alguém pedir pra sair.
    pub fn run_service(self: &Arc<Self>, port: u16) -> Result<(), String> {
        let server = bind(port)?;
        let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);
        *self.port.lock().unwrap() = port;
        self.publish()?;
        let watch_stop = Arc::new(AtomicBool::new(false));
        let me = Arc::clone(self);
        let watcher = watch_control_files(self.monitor.clone(), Arc::new(move || me.shutdown()), watch_stop.clone());
        self.monitor.start();
        self.log.log(format!("Tasker {} — serviço local na porta {port}.", crate::VERSION));
        let (env, log) = (self.env.clone(), self.log.clone());
        std::thread::spawn(move || {
            let fixed = system::refresh_entries(&env);
            if !fixed.is_empty() {
                log.log(format!("Atalhos atualizados pra este programa: {}.", fixed.join(" · ")));
            }
        });
        let server = Arc::new(server);
        while !self.quitting() {
            match server.recv_timeout(Duration::from_millis(400)) {
                Ok(Some(req)) => {
                    let me = Arc::clone(self);
                    std::thread::spawn(move || me.handle(req));
                }
                Ok(None) => {}
                Err(e) => {
                    self.log.log(format!("Serviço local: {e}"));
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        watch_stop.store(true, Ordering::SeqCst);
        let _ = watcher.join();
        self.login.cancel();
        self.stress.stop("o Tasker foi encerrado");
        self.monitor.stop(Duration::from_secs(25));
        self.unpublish();
        self.log.log("Tasker encerrado.");
        Ok(())
    }

    /// dados/tela.json: como a janela e a linha de comando acham este serviço.
    fn publish(&self) -> Result<(), String> {
        write_json(&self.paths.ui_file, &json!({"port": self.port(), "key": self.key, "pid": std::process::id()}), true)
            .map_err(|e| format!("não consegui gravar {}: {e}", self.paths.ui_file.display()))
    }

    fn unpublish(&self) {
        // Só apaga se ainda for o nosso (outro serviço pode ter subido depois).
        if store::read_json(&self.paths.ui_file)
            .and_then(|m| m.get("key").and_then(Value::as_str).map(str::to_string))
            .is_some_and(|k| k == self.key)
        {
            let _ = std::fs::remove_file(&self.paths.ui_file);
        }
    }

    fn handle(self: Arc<Self>, mut req: tiny_http::Request) {
        let header = |name: &str| {
            req.headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default()
        };
        let host = header("Host");
        let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(&host).trim_matches(|c| c == '[' || c == ']').to_string();
        if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "") {
            return respond(req, 403, json!({"erro": "Só aceito conexão local."}));
        }
        if !const_eq(&header("X-Tasker-Key"), &self.key) {
            return respond(req, 403, json!({"erro": "Chave errada."}));
        }
        let url = req.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let param = |name: &str| {
            query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v.to_string()))
        };
        let path = path.to_string();
        match req.method() {
            tiny_http::Method::Get => {
                let body = match path.as_str() {
                    "/api/estado" => serde_json::to_value(self.state()).unwrap_or_default(),
                    "/api/ajustes" => serde_json::to_value(self.settings_payload()).unwrap_or_default(),
                    "/api/historico" => {
                        let hours = param("horas").and_then(|h| h.parse().ok()).unwrap_or(24);
                        serde_json::to_value(history::history(&self.paths, hours)).unwrap_or_default()
                    }
                    "/api/log" => {
                        let since = param("desde").and_then(|h| h.parse().ok()).unwrap_or(0);
                        serde_json::to_value(self.log.since(since)).unwrap_or_default()
                    }
                    _ => return respond(req, 404, json!({"erro": "Não existe."})),
                };
                respond(req, 200, body)
            }
            tiny_http::Method::Post => {
                let mut raw = String::new();
                if req.as_reader().take(64 * 1024).read_to_string(&mut raw).is_err() {
                    return respond(req, 400, json!({"erro": "Corpo inválido."}));
                }
                let body: Value = if raw.trim().is_empty() { json!({}) } else {
                    match serde_json::from_str(&raw) {
                        Ok(v @ Value::Object(_)) => v,
                        _ => return respond(req, 400, json!({"erro": "Corpo inválido."})),
                    }
                };
                let out = match path.as_str() {
                    "/api/ajustes" => {
                        let valores = body.get("valores").and_then(Value::as_object).cloned().unwrap_or_default();
                        self.save_settings(&valores)
                    }
                    "/api/acao" => self.action(&body),
                    _ => return respond(req, 404, json!({"erro": "Não existe."})),
                };
                respond(req, 200, serde_json::to_value(out).unwrap_or_default())
            }
            _ => respond(req, 405, json!({"erro": "Método não aceito."})),
        }
    }
}

fn respond(req: tiny_http::Request, code: u16, body: Value) {
    let data = body.to_string().into_bytes();
    let resp = tiny_http::Response::from_data(data)
        .with_status_code(code)
        .with_header(tiny_http::Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap())
        .with_header(tiny_http::Header::from_bytes("Cache-Control", "no-store").unwrap());
    let _ = req.respond(resp);
}

/// Primeira porta livre a partir de `first` (ou qualquer uma).
fn bind(first: u16) -> Result<tiny_http::Server, String> {
    if first > 0 {
        for port in first..first.saturating_add(30) {
            if let Ok(s) = tiny_http::Server::http(("127.0.0.1", port)) {
                return Ok(s);
            }
        }
    }
    tiny_http::Server::http(("127.0.0.1", 0)).map_err(|e| format!("não consegui abrir o serviço local: {e}"))
}
