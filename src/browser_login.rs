//! Login pelo navegador: abre o Chrome/Edge do sistema e captura o token.
//!
//! Pra quando o login por senha não serve (a Lidl pediu captcha/segundo fator)
//! ou você prefere não guardar senha. O navegador abre num perfil TEMPORÁRIO,
//! você loga no site normalmente, e o Tasker lê a sessão da API do
//! sessionStorage (onde o app do site a guarda) pelo protocolo DevTools. Assim
//! que o token aparece, o navegador fecha e o perfil é apagado — fechar logo
//! importa: o app do site, se ficasse aberto, renovaria o token por conta
//! própria e derrubaria o nosso (o refresh token rotaciona).
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::logs::Logger;
use crate::system::find_browser;
use crate::types::LoginInfo;
use crate::util::{now, random_hex, Env};

/// A sessão vive em sessionStorage['portal-session'] (vuex-persistedstate). O
/// caminho dentro do JSON muda entre builds do site, então procura o objeto que
/// tem accessToken. Às vezes o valor vem JSON dentro de string.
const JS_READ_TOKENS: &str = r#"(() => {
  const raw = sessionStorage.getItem('portal-session');
  if (!raw) return null;
  let obj; try { obj = JSON.parse(raw); } catch (e) { return null; }
  if (typeof obj === 'string') { try { obj = JSON.parse(obj); } catch (e) { return null; } }
  const seen = new Set();
  const walk = (n) => {
    if (!n || typeof n !== 'object' || seen.has(n)) return null;
    seen.add(n);
    if (typeof n.accessToken === 'string' && n.accessToken) return n;
    for (const k of Object.keys(n)) { const h = walk(n[k]); if (h) return h; }
    return null;
  };
  const t = walk(obj);
  return t ? JSON.stringify({access: t.accessToken, refresh: t.refreshToken || null,
                             expiresAtTS: t.expiresAtTS || null}) : null;
})()"#;

const TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    /// Validade em ms (como o site guarda).
    pub expires_at_ms: Option<f64>,
}

pub type OnTokens = Arc<dyn Fn(Tokens) + Send + Sync>;

pub struct BrowserLogin {
    log: Arc<Logger>,
    env: Env,
    on_tokens: OnTokens,
    url: String,
    info: Mutex<LoginInfo>,
    cancel: AtomicBool,
    active: AtomicBool,
    /// Os testes rodam sem janela (`--headless=new`).
    pub extra_args: Mutex<Vec<String>>,
}

fn http_json(url: &str) -> Option<Value> {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
        .get(url)
        .call()
        .ok()?
        .into_json()
        .ok()
}

/// Uma chamada do protocolo DevTools num alvo. Sem cabeçalho Origin: o Chrome
/// recusa conexão DevTools com Origin desconhecido, e sem ele aceita.
fn cdp_call(ws_url: &str, method: &str, params: Value, timeout: Duration) -> Option<Value> {
    let u = ws_url.strip_prefix("ws://")?;
    let host = u.split('/').next()?;
    let stream = std::net::TcpStream::connect(host).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    let (mut ws, _) = tungstenite::client::client(ws_url, stream).ok()?;
    ws.send(tungstenite::Message::text(json!({"id": 1, "method": method, "params": params}).to_string())).ok()?;
    let deadline = Instant::now() + timeout;
    let result = loop {
        if Instant::now() > deadline {
            break None;
        }
        match ws.read() {
            Ok(tungstenite::Message::Text(t)) => {
                if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                    if v.get("id").and_then(Value::as_i64) == Some(1) {
                        break Some(v);
                    }
                }
            }
            Ok(_) => continue,
            Err(_) => break None,
        }
    };
    let _ = ws.close(None);
    result
}

impl BrowserLogin {
    pub fn new(log: Arc<Logger>, env: Env, on_tokens: OnTokens, url: &str) -> Arc<BrowserLogin> {
        Arc::new(BrowserLogin {
            log,
            env,
            on_tokens,
            url: url.into(),
            info: Mutex::new(LoginInfo { estado: "parado".into(), msg: String::new() }),
            cancel: AtomicBool::new(false),
            active: AtomicBool::new(false),
            extra_args: Mutex::new(vec![]),
        })
    }

    pub fn info(&self) -> LoginInfo {
        self.info.lock().unwrap().clone()
    }

    fn set(&self, estado: &str, msg: &str) {
        *self.info.lock().unwrap() = LoginInfo { estado: estado.into(), msg: msg.into() };
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn start(self: &Arc<Self>) -> String {
        if self.active.swap(true, Ordering::SeqCst) {
            return "O login pelo navegador já está aberto.".into();
        }
        let Some(exe) = find_browser(&self.env) else {
            self.active.store(false, Ordering::SeqCst);
            let msg = "Não achei Chrome, Edge nem Chromium instalado. Use o login com número e senha.";
            self.set("erro", msg);
            return msg.into();
        };
        self.cancel.store(false, Ordering::SeqCst);
        self.set("abrindo", "Abrindo o navegador…");
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("login-navegador".into())
            .spawn(move || {
                me.run(&exe);
                me.active.store(false, Ordering::SeqCst);
            })
            .unwrap();
        "Abrindo o navegador: faça login na Lidl lá. Ele fecha sozinho quando der certo.".into()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn run(&self, exe: &Path) {
        let profile = std::env::temp_dir().join(format!("tasker-login-{}", random_hex(6)));
        let _ = std::fs::create_dir_all(&profile);
        let mut port = None;
        let mut proc: Option<Child> = None;
        let outcome = (|| -> Result<(), String> {
            let mut args = vec![
                format!("--user-data-dir={}", profile.to_string_lossy()),
                "--remote-debugging-port=0".into(),
                "--no-first-run".into(),
                "--no-default-browser-check".into(),
                "--disable-sync".into(),
                "--window-size=1100,900".into(),
            ];
            args.extend(self.extra_args.lock().unwrap().iter().cloned());
            args.push("--new-window".into());
            args.push(self.url.clone());
            let mut cmd = Command::new(exe);
            cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let child = cmd.spawn().map_err(|e| format!("não consegui abrir o navegador: {e}"))?;
            proc = Some(child);
            let p = self.wait_port(&profile, proc.as_mut().unwrap())
                .ok_or("o navegador não abriu a porta de depuração")?;
            port = Some(p);
            self.set("aguardando", "Esperando você fazer login na janela…");
            self.log.log("Login pelo navegador: janela aberta, esperando o login.");
            let tokens = self.wait_tokens(p, proc.as_mut().unwrap());
            let exited = proc.as_mut().unwrap().try_wait().ok().flatten().is_some();
            if let Some(t) = tokens {
                (self.on_tokens)(t);
                self.set("ok", "Login capturado — o navegador foi fechado.");
            } else if self.cancelled() {
                self.set("cancelado", "Login pelo navegador cancelado.");
            } else if exited {
                self.set("cancelado", "A janela foi fechada antes do login.");
            } else {
                self.set("erro", "Passou de 15 min sem login — fechei a janela.");
            }
            self.log.log(format!("Login pelo navegador: {}", self.info().msg));
            Ok(())
        })();
        if let Err(e) = outcome {
            let msg = format!("Login pelo navegador falhou: {e}");
            self.set("erro", &msg);
            self.log.log(msg);
        }
        close_browser(proc, port, &profile);
    }

    fn wait_port(&self, profile: &Path, proc: &mut Child) -> Option<u16> {
        let marker = profile.join("DevToolsActivePort");
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !self.cancelled() {
            if proc.try_wait().ok().flatten().is_some() {
                return None;
            }
            if let Ok(text) = std::fs::read_to_string(&marker) {
                if let Some(Ok(p)) = text.lines().next().map(|l| l.trim().parse::<u16>()) {
                    return Some(p);
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        None
    }

    fn wait_tokens(&self, port: u16, proc: &mut Child) -> Option<Tokens> {
        let deadline = Instant::now() + TIMEOUT;
        let host = self.url.split('/').nth(2).unwrap_or("").to_string();
        while Instant::now() < deadline && !self.cancelled() && proc.try_wait().ok().flatten().is_none() {
            let pages: Vec<String> = http_json(&format!("http://127.0.0.1:{port}/json/list"))
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .into_iter()
                .filter(|t| {
                    let url = t.get("url").and_then(Value::as_str).unwrap_or("");
                    t.get("type").and_then(Value::as_str) == Some("page")
                        && (url.contains(&host) || url.contains("lidl-connect.de"))
                })
                .filter_map(|t| t.get("webSocketDebuggerUrl").and_then(Value::as_str).map(str::to_string))
                .collect();
            for ws in pages {
                if let Some(t) = read_page(&ws) {
                    return Some(t);
                }
            }
            for _ in 0..15 {
                if self.cancelled() {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        None
    }
}

fn read_page(ws_url: &str) -> Option<Tokens> {
    let res = cdp_call(ws_url, "Runtime.evaluate",
                       json!({"expression": JS_READ_TOKENS, "returnByValue": true}), Duration::from_secs(10))?;
    let value = res.get("result")?.get("result")?.get("value")?.as_str()?.to_string();
    let data: Value = serde_json::from_str(&value).ok()?;
    let access = data.get("access")?.as_str().filter(|s| !s.is_empty())?.to_string();
    let exp_ms = data.get("expiresAtTS").and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()));
    if exp_ms.is_some_and(|ms| ms / 1000.0 < now() + 60.0) {
        return None; // resto de sessão velha; espera o login de verdade
    }
    Some(Tokens {
        access,
        refresh: data.get("refresh").and_then(Value::as_str).map(str::to_string),
        expires_at_ms: exp_ms,
    })
}

fn wait_exit(proc: &mut Child, d: Duration) -> bool {
    let deadline = Instant::now() + d;
    while Instant::now() < deadline {
        if proc.try_wait().ok().flatten().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn close_browser(proc: Option<Child>, port: Option<u16>, profile: &PathBuf) {
    if let Some(mut proc) = proc {
        if proc.try_wait().ok().flatten().is_none() {
            if let Some(port) = port {
                // Fechar pelo DevTools derruba o navegador inteiro (no Windows o
                // kill só mataria o processo principal).
                if let Some(ws) = http_json(&format!("http://127.0.0.1:{port}/json/version"))
                    .and_then(|v| v.get("webSocketDebuggerUrl").and_then(Value::as_str).map(str::to_string))
                {
                    cdp_call(&ws, "Browser.close", json!({}), Duration::from_secs(5));
                }
                wait_exit(&mut proc, Duration::from_secs(5));
            }
        }
        if proc.try_wait().ok().flatten().is_none() {
            #[cfg(unix)]
            unsafe {
                libc::kill(proc.id() as i32, libc::SIGTERM);
            }
            if !wait_exit(&mut proc, Duration::from_secs(8)) {
                let _ = proc.kill();
                wait_exit(&mut proc, Duration::from_secs(5));
            }
        }
    }
    // No Windows o perfil pode ficar preso por um instante depois do fim.
    for _ in 0..10 {
        let _ = std::fs::remove_dir_all(profile);
        if !profile.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(if cfg!(windows) { 500 } else { 200 }));
    }
}
