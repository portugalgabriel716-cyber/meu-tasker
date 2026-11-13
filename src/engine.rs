//! Motor: lê o volume e dispara o refill, só com HTTP.
//!
//! Regras que valem desde a versão 1:
//!   * a mutation de refill NUNCA é repetida às cegas: a resposta (500 genérico)
//!     não prova nada, então a reserva vira "pendente" — persistida em disco, pra
//!     sobreviver a crash — e quem decide é o efeito (volume subiu ou a opção
//!     entrou em cooldown);
//!   * cooldown que não deu pra ler adia a reserva em vez de reservar às cegas;
//!   * o refresh token rotaciona: o novo é gravado ANTES de qualquer outra coisa;
//!   * login por senha tem trava persistida (errar senha trava a conta na Lidl).
//!
//! Da versão 2:
//!   * volume e cooldown vêm numa query só, pela mesma conexão HTTPS;
//!   * depois de pedir um refill, confirma em poucos segundos (intervalo rápido);
//!   * checagem adaptativa: mede o ritmo de consumo e olha mais cedo quando o
//!     volume vai cruzar o limite antes da próxima checagem.
use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::config::{self, Cfg};
use crate::history::{pause_state, record_event, record_volume};
use crate::lidl::{consumptions_query, jwt_exp, parse_painel, tariffoptions_query, LidlApi, Units};
use crate::logs::Logger;
use crate::notify::Notify;
use crate::paths::Paths;
use crate::store::{self, get_f64, get_str, has_credentials, load_credentials, write_json, Json, StateStore};
use crate::types::{LastRefill, MonitorSnap, Pending, Volume};
use crate::util::{env_get, fmt_g, fmt_local, now, Env};

pub const FATAL_LOGIN_WORDS: &[&str] = &["captcha", "multifactor", "locked", "blocked", "gesperrt", "lockout"];
/// Refresh recusado de vez: não marretar a API.
pub const DEAD_REFRESH_WAIT: f64 = 30.0 * 60.0;

fn obj(v: Value) -> Json {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn opt(v: Option<f64>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

/// (restante, total, plano): só loga o volume quando isto muda.
type VolumeKey = (f64, Option<f64>, Option<f64>);

/// Estado mutável do motor. Um ciclo, um login e uma troca de token nunca
/// acontecem ao mesmo tempo: tudo isso passa pelo mesmo lock.
pub struct Inner {
    pub st: Json,
    pub api: LidlApi,
    pub samples: VecDeque<(f64, f64)>,
    pub refresh_blocked_until: f64,
    pub errors: u32,
    pub combined: bool,
    last_login_notice: f64,
    last_volume_log: (Option<VolumeKey>, f64),
}

pub struct Monitor {
    pub paths: Arc<Paths>,
    pub env: Env,
    pub log: Arc<Logger>,
    notifier: RwLock<Option<Arc<dyn Notify>>>,
    cfg: RwLock<Cfg>,
    pub inner: Mutex<Inner>,
    status: Mutex<MonitorSnap>,
    store: StateStore,
    stop: AtomicBool,
    active: AtomicBool,
    wake: (Mutex<bool>, Condvar),
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Monitor {
    pub fn new(paths: Arc<Paths>, env: Env, log: Arc<Logger>, api: Option<LidlApi>) -> Arc<Monitor> {
        let cfg = config::load(&paths, &env);
        let api = api.unwrap_or_else(|| LidlApi::new(env_get(&env, "TASKER_API_ORIGIN"), 20));
        Arc::new(Monitor {
            store: StateStore::new(&paths),
            paths,
            env,
            log,
            notifier: RwLock::new(None),
            cfg: RwLock::new(cfg),
            inner: Mutex::new(Inner {
                st: Map::new(),
                api,
                samples: VecDeque::new(),
                refresh_blocked_until: 0.0,
                errors: 0,
                combined: true,
                last_login_notice: 0.0,
                last_volume_log: (None, 0.0),
            }),
            status: Mutex::new(MonitorSnap::default()),
            stop: AtomicBool::new(false),
            active: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            thread: Mutex::new(None),
        })
    }

    pub fn set_notifier(&self, n: Arc<dyn Notify>) {
        *self.notifier.write().unwrap() = Some(n);
    }

    pub fn notifier(&self) -> Option<Arc<dyn Notify>> {
        self.notifier.read().unwrap().clone()
    }

    pub fn cfg(&self) -> Cfg {
        self.cfg.read().unwrap().clone()
    }

    pub fn update_cfg(&self, f: impl FnOnce(&mut Cfg)) {
        f(&mut self.cfg.write().unwrap());
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // -- ciclo de vida -------------------------------------------------------

    pub fn start(self: &Arc<Self>) -> bool {
        if self.running() {
            return false;
        }
        let mut slot = self.thread.lock().unwrap();
        if let Some(old) = slot.take() {
            if !old.is_finished() {
                *slot = Some(old);
                return false; // ainda desligando
            }
            let _ = old.join();
        }
        self.stop.store(false, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
        let me = Arc::clone(self);
        *slot = Some(std::thread::Builder::new().name("monitor".into()).spawn(move || me.run(false)).unwrap());
        true
    }

    pub fn stop(&self, wait: Duration) {
        self.stop.store(true, Ordering::SeqCst);
        self.poke();
        let deadline = Instant::now() + wait;
        let mut slot = self.thread.lock().unwrap();
        if let Some(h) = slot.as_ref() {
            if h.thread().id() == std::thread::current().id() {
                return;
            }
        }
        while slot.as_ref().is_some_and(|h| !h.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
        }
        if slot.as_ref().is_some_and(|h| h.is_finished()) {
            if let Some(h) = slot.take() {
                let _ = h.join();
            }
        }
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn running(&self) -> bool {
        self.active.load(Ordering::SeqCst) && !self.stopping()
    }

    pub fn poke(&self) {
        let (m, c) = &self.wake;
        *m.lock().unwrap() = true;
        c.notify_all();
    }

    pub fn reload_config(&self) {
        *self.cfg.write().unwrap() = config::load(&self.paths, &self.env);
        self.poke();
    }

    pub fn set_status(&self, f: impl FnOnce(&mut MonitorSnap)) {
        f(&mut self.status.lock().unwrap());
    }

    pub fn status(&self) -> MonitorSnap {
        self.status.lock().unwrap().clone()
    }

    pub fn snapshot(&self) -> MonitorSnap {
        let mut snap = self.status();
        let (paused, until) = pause_state(&self.paths);
        snap.ligado = self.running();
        snap.pausado = paused;
        snap.pausa_ate = (until > 0.0).then_some(until);
        snap.credenciais = has_credentials(&self.paths, &self.env);
        snap.agora = now();
        if paused && snap.ligado {
            snap.fase = "pausado".into();
        }
        snap
    }

    pub fn write_status_file(&self) {
        if let Ok(v) = serde_json::to_value(self.snapshot()) {
            let _ = write_json(&self.paths.status_file, &v, false);
        }
    }

    // -- estado em disco -----------------------------------------------------

    pub fn load_state(&self) {
        let mut g = self.lock();
        g.st = self.store.load();
        if let Some(seed) = env_get(&self.env, "TASKER_REFRESH_TOKEN") {
            if get_str(&g.st, "refresh").is_none() {
                self.log.log("Usando TASKER_REFRESH_TOKEN do ambiente como semente.");
                g.st.insert("refresh".into(), Value::from(seed));
            }
        }
        let pending_ok = g.st.get("pending_refill").and_then(Value::as_object).is_some_and(|p| get_f64(p, "at").unwrap_or(0.0) != 0.0);
        if !pending_ok {
            g.st.insert("pending_refill".into(), Value::Null);
        }
        let saved = get_f64(&g.st, "last_refill").unwrap_or(0.0);
        // Sem histórico, o relógio do refill forçado começa agora. Com um
        // last_refill de dias atrás, ele vence agora (não "há dias").
        let cfg = self.cfg();
        let last = if saved > 0.0 { saved.max(now() - cfg.max_without_refill) } else { now() };
        g.st.insert("last_refill".into(), Value::from(last));
        let pend = pending_of(&g.st);
        let exp = get_f64(&g.st, "expires_at");
        let fails = get_f64(&g.st, "refill_fails").unwrap_or(0.0) as i64;
        self.set_status(|s| {
            s.token_expira = exp;
            s.pendente = pend;
            s.falhas_seguidas = fails;
        });
    }

    fn persist(&self, g: &mut Inner, patch: Json) {
        for (k, v) in &patch {
            g.st.insert(k.clone(), v.clone());
        }
        if let Err(e) = self.store.update(&patch) {
            let name = self.paths.state_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.log.log(format!("NÃO consegui gravar {name}: {e}"));
            if patch.contains_key("refresh") {
                self.notify(&format!("Não consegui gravar a sessão em disco ({e}). Se o Tasker \
                                      reiniciar agora, vai precisar de login."));
            }
        }
    }

    pub fn notify(&self, msg: &str) {
        match self.notifier() {
            Some(n) => {
                n.send(msg);
            }
            None => self.log.log(format!("Aviso: {msg}")),
        }
    }

    // -- tokens --------------------------------------------------------------

    fn token_valid(&self, g: &Inner, margin: f64) -> bool {
        get_str(&g.st, "access").is_some() && get_f64(&g.st, "expires_at").unwrap_or(0.0) - margin > now()
    }

    fn set_tokens(&self, g: &mut Inner, access: &str, refresh: Option<&str>, expires_in: Option<f64>, expires_at: Option<f64>) {
        let from_jwt = jwt_exp(access);
        let exp = if from_jwt > 0.0 {
            from_jwt
        } else {
            expires_at.unwrap_or_else(|| now() + expires_in.unwrap_or(3600.0))
        };
        let refresh = refresh.map(str::to_string).or_else(|| get_str(&g.st, "refresh").map(str::to_string));
        // Rotacionou: grava JÁ, antes de usar. Perder o refresh novo = login manual.
        self.persist(g, obj(json!({"access": access, "refresh": refresh, "expires_at": exp})));
        g.refresh_blocked_until = 0.0;
        self.set_status(|s| {
            s.token_expira = Some(exp);
            s.precisa_login = None;
        });
    }

    /// Alguém gravou tokens mais novos no arquivo (login por outra via).
    fn adopt_disk(&self, g: &mut Inner) -> bool {
        let disk = self.store.load();
        let mine = get_f64(&g.st, "expires_at").unwrap_or(0.0);
        if get_str(&disk, "access").is_some() && get_f64(&disk, "expires_at").unwrap_or(0.0) > mine {
            for k in ["access", "refresh", "expires_at"] {
                g.st.insert(k.into(), disk.get(k).cloned().unwrap_or(Value::Null));
            }
            g.refresh_blocked_until = 0.0;
            return true;
        }
        if let Some(rt) = get_str(&disk, "refresh") {
            if Some(rt) != get_str(&g.st, "refresh") {
                g.st.insert("refresh".into(), Value::from(rt));
                g.refresh_blocked_until = 0.0;
                return true;
            }
        }
        false
    }

    fn refresh(&self, g: &mut Inner, force: bool) -> bool {
        let Some(rt) = get_str(&g.st, "refresh").map(str::to_string) else { return false };
        if !force && now() < g.refresh_blocked_until {
            return false;
        }
        let access = get_str(&g.st, "access").map(str::to_string);
        let r = g.api.refresh(&rt, access.as_deref());
        let data = r.json_obj();
        if r.status == 200 {
            if let Some(at) = get_str(&data, "access_token") {
                let new_rt = get_str(&data, "refresh_token").unwrap_or(&rt).to_string();
                self.set_tokens(g, at, Some(&new_rt), get_f64(&data, "expires_in"), None);
                self.log.log(format!("Token renovado (vale {} min).", token_minutes(g)));
                return true;
            }
        }
        if matches!(r.status, 400 | 401 | 403) {
            // Recusado de vez (revogado, ou você logou em outro lugar).
            g.refresh_blocked_until = now() + DEAD_REFRESH_WAIT;
            self.log.log(format!("Refresh token recusado (HTTP {}): {}", r.status, r.snippet(120)));
        } else {
            let wait = self.cfg().refresh_retry_delay;
            g.refresh_blocked_until = now() + wait;
            self.log.log(format!("Renovação do token falhou (HTTP {}: {}) — tento de novo em {} s.",
                                 r.status, r.snippet(100), wait));
        }
        false
    }

    fn access(g: &Inner) -> Option<String> {
        get_str(&g.st, "access").map(str::to_string)
    }

    fn ensure_token(&self, g: &mut Inner) -> Option<String> {
        let margin = self.cfg().token_margin;
        if self.token_valid(g, margin) {
            return Self::access(g);
        }
        if self.adopt_disk(g) && self.token_valid(g, margin) {
            self.log.log("Peguei do disco um token mais novo.");
            return Self::access(g);
        }
        if self.refresh(g, false) {
            return Self::access(g);
        }
        if self.token_valid(g, 30.0) {
            return Self::access(g); // sem margem, mas ainda vale
        }
        if self.password_login_locked(g, true, None).0 {
            return Self::access(g);
        }
        self.need_login(g);
        None
    }

    fn need_login(&self, g: &mut Inner) {
        let reason = if has_credentials(&self.paths, &self.env) {
            "A sessão da Lidl caiu e o login automático não resolveu agora."
        } else {
            "A sessão da Lidl caiu: faça login no Tasker."
        };
        self.set_status(|s| {
            s.precisa_login = Some(reason.into());
            s.fase = "sem sessão".into();
        });
        if now() - g.last_login_notice > 3600.0 {
            g.last_login_notice = now();
            self.notify(&format!("{reason} (abra o Tasker)"));
        }
    }

    /// (ok, mensagem). auto=true respeita a trava anti-bloqueio de conta.
    pub fn password_login(&self, auto: bool, creds: Option<(String, String)>) -> (bool, String) {
        let mut g = self.lock();
        self.password_login_locked(&mut g, auto, creds)
    }

    fn password_login_locked(&self, g: &mut Inner, auto: bool, creds: Option<(String, String)>) -> (bool, String) {
        let Some((msisdn, password)) = creds.or_else(|| load_credentials(&self.paths, &self.env)) else {
            return (false, "Sem número/senha guardados.".into());
        };
        if msisdn.is_empty() || password.is_empty() {
            return (false, "Sem número/senha guardados.".into());
        }
        let cfg = self.cfg();
        let t0 = now();
        // A trava pode ter sido zerada por fora (--set-credentials): vale o disco.
        let disk = self.store.load();
        for k in ["login_fails", "login_blocked_until"] {
            if let Some(v) = disk.get(k) {
                g.st.insert(k.into(), v.clone());
            }
        }
        let fails = get_f64(&g.st, "login_fails").unwrap_or(0.0) as i64;
        let blocked = get_f64(&g.st, "login_blocked_until").unwrap_or(0.0);
        if auto && t0 < blocked {
            self.log.once("login-espera",
                          format!("Login automático em espera por {:.0} min (proteção da conta).", (blocked - t0) / 60.0),
                          1800.0);
            return (false, "Login automático em espera.".into());
        }

        let prefix: String = msisdn.chars().take(4).collect();
        self.log.log(format!("Login por senha ({prefix}…{}).", if auto { ", automático" } else { "" }));
        let r = g.api.password_login(&msisdn, &password);
        let data = r.json_obj();
        if r.status == 200 {
            if let Some(at) = get_str(&data, "access_token").map(str::to_string) {
                let mfa = data.get("needs_multifactor_authentication").and_then(Value::as_bool).unwrap_or(false);
                let mfa_ok = data.get("mfa_authenticated").and_then(Value::as_bool).unwrap_or(false);
                if mfa && !mfa_ok {
                    self.persist(g, obj(json!({"login_fails": 0, "login_blocked_until": t0 + cfg.login_block_long})));
                    let msg = "A Lidl pediu segundo fator — use \"Entrar pelo navegador\".";
                    self.log.log(msg);
                    if auto {
                        self.notify(msg);
                    }
                    return (false, msg.into());
                }
                let rt = get_str(&data, "refresh_token").map(str::to_string);
                self.set_tokens(g, &at, rt.as_deref(), get_f64(&data, "expires_in"), None);
                self.persist(g, obj(json!({"login_fails": 0, "login_blocked_until": 0})));
                self.log.log(format!("Login por senha OK (token vale {} min).", token_minutes(g)));
                self.poke();
                return (true, "Login feito.".into());
            }
        }

        let api_msg = ["message", "error_description", "error"]
            .iter()
            .find_map(|k| get_str(&data, k).map(str::to_string))
            .unwrap_or_else(|| r.snippet(160));
        self.log.log(format!("Login por senha falhou (HTTP {}): {}", r.status, r.snippet(200)));
        if r.status == -1 || r.status >= 500 {
            // Rede ou erro do servidor não é senha errada: não conta na trava.
            return (false, format!("A Lidl não respondeu direito (HTTP {}). Tente de novo.", r.status));
        }
        let lower = r.text.to_lowercase();
        if FATAL_LOGIN_WORDS.iter().any(|w| lower.contains(w)) {
            self.persist(g, obj(json!({"login_fails": 0, "login_blocked_until": t0 + cfg.login_block_long})));
            let msg = "A Lidl pediu captcha ou travou a conta — o login automático parou. Use \
                       \"Entrar pelo navegador\".";
            if auto {
                self.notify(msg);
            }
            return (false, msg.into());
        }
        let fails = fails + 1;
        let mut delay = (cfg.login_backoff * 2f64.powi((fails - 1) as i32)).min(cfg.login_block_long);
        if fails >= cfg.login_max_fails {
            delay = cfg.login_block_long;
            if auto {
                self.notify(&format!("O login automático falhou {fails} vezes — parei por {:.0} h pra não \
                                      travar a conta. Confira a senha.", delay / 3600.0));
            }
        }
        self.persist(g, obj(json!({"login_fails": fails, "login_blocked_until": t0 + delay})));
        (false, format!("Login recusado: {api_msg}"))
    }

    /// Tokens capturados por outra via (login pelo navegador).
    pub fn adopt_tokens(&self, access: &str, refresh: Option<&str>, expires_at: Option<f64>, source: &str) {
        {
            let mut g = self.lock();
            self.set_tokens(&mut g, access, refresh, None, expires_at);
            self.persist(&mut g, obj(json!({"login_fails": 0, "login_blocked_until": 0})));
            self.log.log(format!("Sessão nova pelo {source} (token vale {} min).", token_minutes(&g)));
        }
        self.poke();
    }

    /// Lê o volume uma vez (linha de comando `--status`).
    pub fn read_now(&self) -> Result<(Units, Option<bool>), String> {
        let mut g = self.lock();
        let Some(token) = self.ensure_token(&mut g) else {
            return Err("sem sessão válida — faça login no Tasker".into());
        };
        self.read(&mut g, token)
    }

    pub fn refresh_token(&self) -> Option<String> {
        get_str(&self.store.load(), "refresh").map(str::to_string)
    }

    // -- leitura -------------------------------------------------------------

    /// (unidades, cooldown) ou erro. Volume e cooldown numa ida só.
    fn read(&self, g: &mut Inner, mut token: String) -> Result<(Units, Option<bool>), String> {
        let mut r = None;
        for attempt in 0..2 {
            let resp = if g.combined {
                g.api.painel(&token)
            } else {
                g.api.graphql("consumptions", &consumptions_query(), &token, json!({}), false)
            };
            if resp.status == 401 && attempt == 0 {
                self.log.log("A API respondeu 401 — renovando o token.");
                if !self.refresh(g, true) {
                    self.persist(g, obj(json!({"access": null}))); // revogado: não adianta insistir
                    if self.ensure_token(g).is_none() {
                        // senha guardada, ou pede login
                        return Err("a Lidl recusou a sessão".into());
                    }
                }
                token = Self::access(g).unwrap_or_default();
                continue;
            }
            if matches!(resp.status, 500 | 502 | 503 | 504) && attempt == 0 {
                // A API erra sozinha algumas vezes por hora nas leituras.
                if self.wait_stop(Duration::from_millis(1500)) {
                    return Err("encerrando".into());
                }
                continue;
            }
            if resp.status == 400 && g.combined && attempt == 0 {
                g.combined = false;
                self.log.log("A API recusou a consulta combinada — passo a usar duas.");
                continue;
            }
            r = Some(resp);
            break;
        }
        let r = r.expect("o laço sempre termina com uma resposta");
        if r.status != 200 {
            return Err(format!("HTTP {} {}", r.status, r.snippet(120)).trim().to_string());
        }
        let Some(Value::Object(body)) = r.json() else {
            return Err("resposta não era JSON".into());
        };
        if let Some(Value::Array(errs)) = body.get("errors") {
            if !errs.is_empty() {
                let msgs: Vec<String> = errs
                    .iter()
                    .take(2)
                    .map(|e| crate::util::snippet(&e.get("message").map(|m| m.as_str().map(str::to_string).unwrap_or_else(|| m.to_string())).unwrap_or_else(|| "None".into()), 90))
                    .collect();
                self.log.once("gql-err", format!("Erro da API: {}", msgs.join("; ")), 600.0);
            }
        }
        let (units, mut cooldown) = parse_painel(body.get("data"));
        if !g.combined && units.is_some() {
            let r2 = g.api.graphql("tariffOptions", &tariffoptions_query(), &token, json!({}), false);
            if r2.status == 200 {
                cooldown = parse_painel(r2.json().as_ref().and_then(|b| b.get("data"))).1;
            }
        }
        match units {
            Some(u) => Ok((u, cooldown)),
            None => Err("a resposta não trouxe o consumo".into()),
        }
    }

    // -- o ciclo -------------------------------------------------------------

    /// Uma checagem completa. Devolve (segundos até a próxima, motivo).
    pub fn cycle(&self) -> (f64, String) {
        let mut g = self.lock();
        self.cycle_locked(&mut g)
    }

    fn cycle_locked(&self, g: &mut Inner) -> (f64, String) {
        let cfg = self.cfg();
        self.set_status(|s| s.fase = "checando".into());
        let Some(token) = self.ensure_token(g) else {
            return self.error_delay(g, true);
        };

        let (units, cooldown) = match self.read(g, token.clone()) {
            Ok(x) => x,
            Err(err) => {
                if self.status().precisa_login.is_some() {
                    return self.error_delay(g, true);
                }
                g.errors += 1;
                self.log.log(format!("Não consegui ler o volume: {err}"));
                self.set_status(|s| {
                    s.erro = Some(err);
                    s.fase = "erro".into();
                });
                return self.error_delay(g, false);
            }
        };
        g.errors = 0;

        let t = now();
        let (mut left, mut total) = (None, None);
        match &units.refill {
            None => self.log.once("sem-balde", "Nenhum balde REFILLABLE_DATA na conta (tarifa mudou?).", 3600.0),
            Some(r) if r.get("unit").and_then(Value::as_str) == Some("UNLIMITED") => {
                self.log.once("ilimitado", "Refill marcado como ilimitado — nada a fazer.", 3600.0)
            }
            Some(r) => {
                left = store::as_f64(r.get("left"));
                total = store::as_f64(r.get("max"));
            }
        }

        let mut volume = None;
        if let Some(l) = left {
            g.samples.push_back((t, l));
            while g.samples.len() > 200 {
                g.samples.pop_front();
            }
            record_volume(&self.paths, l);
            let plan = units.data.as_ref();
            let v = Volume {
                left: l,
                total,
                plan_left: plan.and_then(|p| store::as_f64(p.get("left"))),
                plan_total: plan.and_then(|p| store::as_f64(p.get("max"))),
                unit: units.refill.as_ref().and_then(|r| r.get("unit")).and_then(Value::as_str).unwrap_or("GB").into(),
                t,
            };
            self.log_volume(g, &v);
            volume = Some(v);
        }
        let exp = get_f64(&g.st, "expires_at");
        self.set_status(|s| {
            s.volume = volume;
            s.cooldown = cooldown;
            s.ultima_checagem = Some(t);
            s.erro = None;
            s.fase = "aguardando".into();
            s.precisa_login = None;
            s.token_expira = exp;
        });

        if pending_of(&g.st).is_some() && self.resolve_pending(g, left, cooldown) {
            return self.confirm_delay();
        }
        let Some(left) = left else {
            return (cfg.check_interval, "volume ilegível".into());
        };
        self.decide(g, &token, left, total, cooldown)
    }

    fn decide(&self, g: &mut Inner, token: &str, left: f64, total: Option<f64>, cooldown: Option<bool>) -> (f64, String) {
        let cfg = self.cfg();
        let t = now();
        let since = t - get_f64(&g.st, "last_refill").unwrap_or(t);
        let forced = since >= cfg.max_without_refill;
        let low = left <= cfg.gb_threshold;
        if !(forced || low) {
            return self.next_delay(g, Some(left));
        }
        let (reason, short) = if low {
            (format!("GB baixo ({left:.2})"), "GB baixo")
        } else {
            (format!("timeout ({:.0} min)", since / 60.0), "timeout")
        };

        if let Some(tot) = total.filter(|tot| *tot > 0.0) {
            if left >= tot - 0.005 {
                self.log.log(format!("Refill pedido ({reason}) mas o balde está cheio ({}/{} GB) — só zero o relógio.",
                                     fmt_g(left), fmt_g(tot)));
                self.persist(g, obj(json!({"last_refill": t, "refill_fails": 0, "next_refill_try": 0})));
                return self.next_delay(g, Some(left));
            }
        }
        let next_try = get_f64(&g.st, "next_refill_try").unwrap_or(0.0);
        if t < next_try {
            self.log.once("backoff", format!("Refill pedido ({short}) mas em espera por mais {:.0} min (falhas seguidas).",
                                             (next_try - t) / 60.0), 300.0);
            let (delay, _) = self.next_delay(g, Some(left));
            return (delay.min((next_try - t).max(2.0)), "fim da espera de falha".into());
        }
        if cooldown == Some(true) {
            self.log.once("cooldown", format!("Refill pedido ({short}) mas a opção está em cooldown — espero."), 600.0);
            if low {
                return (cfg.fast_interval, "esperando o cooldown acabar".into());
            }
            return self.next_delay(g, Some(left));
        }
        if cooldown.is_none() {
            // Reservar sem conferir o cooldown é o único jeito de reservar duas
            // vezes sem perceber. Esperar uns segundos não custa nada.
            self.log.log(format!("Refill pedido ({reason}) mas não consegui conferir o cooldown — confiro de novo em instantes."));
            return (cfg.fast_interval, "cooldown ilegível".into());
        }
        self.book(g, token, left, &reason)
    }

    fn book(&self, g: &mut Inner, token: &str, left: f64, reason: &str) -> (f64, String) {
        self.log.log(format!("Refill: {reason} — pedindo."));
        let mut sent_at = now();
        let mut r = g.api.book_refill(token);
        if r.status == 401 && self.refresh(g, true) {
            // 401 = recusada antes de processar: mandar de novo é seguro.
            sent_at = now();
            let t = Self::access(g).unwrap_or_default();
            r = g.api.book_refill(&t);
        }
        let success = r.status == 200
            && r.json()
                .as_ref()
                .and_then(|b| b.get("data"))
                .and_then(|d| d.get("bookTariffoptionsDirect"))
                .and_then(|x| x.get("success"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
        if success {
            self.log.log(format!("Refill aceito pela API ({} ms).", r.ms));
            self.worked(g, Some(left), None, "API confirmou", sent_at);
            return self.next_delay(g, Some(left));
        }
        if r.status == 500 {
            self.log.log(format!("Refill enviado ({} ms): HTTP 500 genérico — normal aqui, confirmo pelo efeito.", r.ms));
        } else if r.status == -1 {
            self.log.log(format!("Refill: a conexão falhou ({}) — pode ter chegado; confiro pelo efeito, sem repetir.",
                                 crate::util::snippet(&r.text, 80)));
        } else {
            self.log.log(format!("Refill: resposta HTTP {} {} — confiro pelo efeito.", r.status, r.snippet(100)));
        }
        let pend = Pending { at: sent_at, before: Some(left) };
        self.persist(g, obj(json!({"pending_refill": {"at": sent_at, "before": left}})));
        self.set_status(|s| {
            s.pendente = Some(pend);
            s.fase = "confirmando refill".into();
        });
        self.confirm_delay()
    }

    /// true se a reserva anterior segue sem veredito (não mexer em nada).
    fn resolve_pending(&self, g: &mut Inner, left: Option<f64>, cooldown: Option<bool>) -> bool {
        let Some(pend) = pending_of(&g.st) else { return false };
        let (before, at) = (pend.before, pend.at);
        if let (Some(l), Some(b)) = (left, before) {
            if l > b + 0.005 {
                self.worked(g, before, left, "volume subiu", at);
                return false;
            }
        }
        if cooldown == Some(true) {
            self.worked(g, before, left, "cooldown da opção", at);
            return false;
        }
        let grace = self.cfg().pending_grace;
        if now() - at < grace {
            self.set_status(|s| s.fase = "confirmando refill".into());
            return true;
        }
        self.log.log(format!("A reserva não surtiu efeito em {grace} s."));
        self.failed(g, before);
        false
    }

    fn worked(&self, g: &mut Inner, before: Option<f64>, after: Option<f64>, why: &str, sent_at: f64) {
        let t = now();
        let latency = (sent_at > 0.0).then(|| ((t - sent_at) * 10.0).round() / 10.0);
        self.persist(g, obj(json!({"pending_refill": null, "last_refill": t, "refill_fails": 0, "next_refill_try": 0})));
        record_event(&self.paths, "refill_ok",
                     obj(json!({"before": opt(before), "after": opt(after), "why": why, "latency": opt(latency)})));
        let txt = match (before, after) {
            (Some(b), Some(a)) => format!(": {b:.2} → {a:.2} GB"),
            _ => String::new(),
        };
        let took = match latency {
            Some(l) if l != 0.0 => format!(" — levou {l:.0} s"),
            _ => String::new(),
        };
        self.log.log(format!("Refill confirmado ({why}){txt}{took}."));
        self.set_status(|s| {
            s.pendente = None;
            s.falhas_seguidas = 0;
            s.ultimo_refill = Some(LastRefill { t, before, after, why: why.into(), latencia: latency });
        });
    }

    fn failed(&self, g: &mut Inner, before: Option<f64>) {
        let cfg = self.cfg();
        let fails = get_f64(&g.st, "refill_fails").unwrap_or(0.0) as i64 + 1;
        let backoff = (cfg.check_interval * 2f64.powi(fails as i32)).min(cfg.refill_backoff_max);
        self.persist(g, obj(json!({"pending_refill": null, "refill_fails": fails, "next_refill_try": now() + backoff})));
        record_event(&self.paths, "refill_fail", obj(json!({"before": opt(before), "fails": fails})));
        self.log.log(format!("Refill falhou ({fails}x seguidas) — nova tentativa em {:.0} min.", backoff / 60.0));
        self.set_status(|s| {
            s.pendente = None;
            s.falhas_seguidas = fails;
        });
        if fails == 4 {
            self.notify("O refill da Lidl falhou 4 vezes seguidas. Vale olhar o Tasker.");
        }
    }

    // -- ritmo ---------------------------------------------------------------

    /// Consumo em GB/s: o maior entre a janela de 2 min e a de 10 min.
    ///
    /// A curta pega um download que acabou de começar; a longa dá estabilidade.
    /// Subidas (refills) são ignoradas — só conta o que foi gasto.
    pub fn rate(samples: &VecDeque<(f64, f64)>) -> Option<f64> {
        let t = now();
        let mut best: Option<f64> = None;
        for window in [120.0, 600.0] {
            let pts: Vec<&(f64, f64)> = samples.iter().filter(|p| p.0 >= t - window).collect();
            if pts.len() < 2 {
                continue;
            }
            let span = pts[pts.len() - 1].0 - pts[0].0;
            if span < 40.0 {
                continue;
            }
            let spent: f64 = pts.windows(2).map(|w| (w[0].1 - w[1].1).max(0.0)).sum();
            let rate = spent / span;
            best = Some(best.map_or(rate, |b| b.max(rate)));
        }
        best
    }

    /// Próxima checagem pelo ritmo atual (os testes chamam direto).
    pub fn next_delay_for(&self, left: Option<f64>) -> (f64, String) {
        let mut g = self.lock();
        self.next_delay(&mut g, left)
    }

    fn next_delay(&self, g: &mut Inner, left: Option<f64>) -> (f64, String) {
        let cfg = self.cfg();
        let mut delay = cfg.check_interval;
        let mut why = "intervalo normal".to_string();
        let rate = Self::rate(&g.samples);
        self.set_status(|s| s.ritmo_gb_h = rate.map(|r| (r * 3600.0 * 100.0).round() / 100.0));
        if let (true, Some(rate), Some(left)) = (cfg.adaptive, rate.filter(|r| *r > 0.0), left) {
            let head = left - cfg.gb_threshold;
            if head > 0.0 && head / rate / 2.0 < delay {
                let eta = head / rate;
                delay = cfg.fast_interval.max(eta / 2.0);
                why = format!("adaptativo: no ritmo atual cruza o limite em ~{eta:.0} s");
            }
        }
        let last = get_f64(&g.st, "last_refill").unwrap_or_else(now);
        let until_forced = cfg.max_without_refill - (now() - last);
        if 0.0 < until_forced + 1.0 && until_forced + 1.0 < delay {
            delay = until_forced + 1.0;
            why = "hora do refill forçado".into();
        }
        (delay.max(2.0), why)
    }

    fn confirm_delay(&self) -> (f64, String) {
        (self.cfg().fast_interval, "confirmando o refill".into())
    }

    fn error_delay(&self, g: &Inner, login: bool) -> (f64, String) {
        let base = self.cfg().check_interval;
        if login {
            return (base.max(60.0), "sem sessão".into());
        }
        let exp = (g.errors.max(1) - 1).min(3) as i32;
        ((base * 2f64.powi(exp)).min(300.0), "erro na leitura".into())
    }

    fn log_volume(&self, g: &mut Inner, v: &Volume) {
        let key = (v.left, v.total, v.plan_left);
        let (last_key, last_t) = g.last_volume_log;
        if last_key == Some(key) && now() - last_t < 600.0 {
            return;
        }
        g.last_volume_log = (Some(key), now());
        let mut plan = String::new();
        if let Some(pl) = v.plan_left {
            plan = format!(" | plano: {}", fmt_g(pl));
            plan += &match v.plan_total {
                Some(pt) => format!("/{} GB", fmt_g(pt)),
                None => " GB".into(),
            };
        }
        let total = v.total.map(|t| format!("/{}", fmt_g(t))).unwrap_or_default();
        self.log.log(format!("Volume de refill: {:.2}{total} GB{plan}", v.left));
    }

    // -- loop ----------------------------------------------------------------

    pub fn describe(&self) -> String {
        let c = self.cfg();
        let adapt = if c.adaptive { format!(" (adaptativa, até {} s)", c.fast_interval) } else { String::new() };
        format!("checagem a cada {} s{adapt} | refill se ≤ {} GB | força após {} min",
                c.check_interval, fmt_g(c.gb_threshold), (c.max_without_refill / 60.0).floor())
    }

    pub fn run(&self, once: bool) {
        self.active.store(true, Ordering::SeqCst);
        self.load_state();
        self.log.log(format!("Monitor ligado | {}", self.describe()));
        self.set_status(|s| s.fase = "iniciando".into());
        while !self.stopping() {
            let (paused, until) = pause_state(&self.paths);
            if paused {
                self.wait_pause(until);
                if once {
                    break;
                }
                continue;
            }
            let res = std::panic::catch_unwind(AssertUnwindSafe(|| self.cycle()));
            let (delay, why) = match res {
                Ok(x) => x,
                Err(e) => {
                    // Nunca deixa o loop morrer por um erro solto.
                    let msg = e.downcast_ref::<String>().cloned()
                        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "?".into());
                    let mut g = self.lock();
                    g.errors += 1;
                    self.log.log(format!("Erro inesperado no ciclo: {msg}"));
                    self.error_delay(&g, false)
                }
            };
            self.write_status_file();
            // No modo de uma passada (cron), um refill recém-pedido é confirmado
            // aqui mesmo, no intervalo rápido — senão a pendência ficaria pra
            // próxima execução, minutos depois.
            if once && pending_of(&self.lock().st).is_none() {
                break;
            }
            self.sleep(delay, &why);
        }
        self.lock().api.close();
        self.set_status(|s| {
            s.fase = "parado".into();
            s.proxima_checagem = None;
        });
        self.active.store(false, Ordering::SeqCst);
        self.write_status_file();
        if !once {
            self.log.log("Monitor desligado.");
        }
    }

    fn sleep(&self, delay: f64, why: &str) {
        self.set_status(|s| {
            s.proxima_checagem = Some(now() + delay);
            s.motivo_intervalo = why.into();
        });
        self.wait_wake(Duration::from_secs_f64(delay.max(0.0)));
    }

    /// Dorme até o prazo, um `poke` ou a parada.
    fn wait_wake(&self, d: Duration) -> bool {
        let (m, c) = &self.wake;
        let deadline = Instant::now() + d;
        let mut woke = m.lock().unwrap();
        while !*woke && !self.stopping() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            woke = c.wait_timeout(woke, left).unwrap().0;
        }
        let was = *woke;
        *woke = false;
        was
    }

    /// Dorme até o prazo ou a parada. true = parou.
    fn wait_stop(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline {
            if self.stopping() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())));
        }
        self.stopping()
    }

    fn wait_pause(&self, until: f64) {
        let txt = if until > 0.0 { fmt_local(until, "até %H:%M") } else { "até você retomar".into() };
        self.log.log(format!("Pausado {txt} — não toco na sessão (conta liberada pra você)."));
        self.set_status(|s| {
            s.fase = "pausado".into();
            s.proxima_checagem = None;
        });
        self.write_status_file();
        while !self.stopping() && pause_state(&self.paths).0 {
            self.wait_wake(Duration::from_secs(2));
        }
        if !self.stopping() {
            self.log.log("Pausa encerrada — voltando ao trabalho.");
        }
    }
}

fn token_minutes(g: &Inner) -> i64 {
    (((get_f64(&g.st, "expires_at").unwrap_or(0.0) - now()) / 60.0).max(0.0)) as i64
}

fn pending_of(st: &Json) -> Option<Pending> {
    let p = st.get("pending_refill")?.as_object()?;
    let at = get_f64(p, "at").filter(|a| *a != 0.0)?;
    Some(Pending { at, before: get_f64(p, "before") })
}

/// `tasker.poke` = checar agora; `tasker.stop` = encerrar limpo (qualquer script cria).
pub fn watch_control_files(monitor: Arc<Monitor>, on_stop: Arc<dyn Fn() + Send + Sync>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("controle".into())
        .spawn(move || {
            let p = monitor.paths.clone();
            for f in [&p.stop_file, &p.poke_file] {
                let _ = std::fs::remove_file(f); // sobras da execução anterior
            }
            loop {
                for _ in 0..10 {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if p.poke_file.exists() && std::fs::remove_file(&p.poke_file).is_ok() {
                    monitor.log.log("Checagem adiantada (tasker.poke).");
                    monitor.poke();
                }
                if p.stop_file.exists() && std::fs::remove_file(&p.stop_file).is_ok() {
                    monitor.log.log("Parada pedida por tasker.stop — encerrando limpo.");
                    on_stop();
                }
                let fase = monitor.status().fase;
                if pause_state(&p).0 && fase != "pausado" && fase != "parado" {
                    monitor.poke(); // pausa criada por fora: o motor percebe já
                }
            }
        })
        .unwrap()
}
