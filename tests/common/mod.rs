//! Uma API da Lidl de mentira, pra testar offline.
//!
//! Imita o que importa da de verdade: tokens JWT com validade, refresh token que
//! ROTACIONA (o usado morre), login por senha (com captcha opcional), a consulta
//! combinada `painel`, e a mutation de refill que devolve 500 genérico mesmo
//! quando a reserva entra — o volume só sobe algumas leituras depois. Fala
//! HTTP/1.1 com keep-alive, igual à API de verdade, e sabe derrubar a conexão.
#![allow(dead_code)]

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{json, Value};

pub fn make_jwt(exp: f64) -> String {
    let b64 = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!("{}.{}.assinatura", b64(json!({"alg": "none"})), b64(json!({"exp": exp, "sub": "teste"})))
}

pub fn now() -> f64 {
    tasker::util::now()
}

pub struct Fake {
    pub left: f64,
    pub max: f64,
    pub plan_left: f64,
    pub cooldown: bool,
    pub token_ttl: f64,
    pub access: HashSet<String>,
    pub refresh: HashSet<String>,
    pub password: (String, String),
    pub captcha: bool,
    /// (operação, status)
    pub calls: Vec<(String, i32)>,
    pub connections: usize,
    /// status a devolver nas próximas leituras
    pub fail_reads: Vec<u16>,
    /// "500" | "success" | "noop" | "drop"
    pub refill_mode: String,
    /// leituras depois da mutation até o volume subir
    pub rise_after_reads: i32,
    rise_in: Option<i32>,
    pub reject_combined: bool,
    pub omit_tariffs: bool,
    /// derruba a próxima conexão reaproveitada antes de responder
    pub drop_next: bool,
}

impl Fake {
    pub fn issue(&mut self, ttl: Option<f64>) -> (String, String) {
        let a = make_jwt(now() + ttl.unwrap_or(self.token_ttl));
        let r = format!("rt-{}", tasker::util::random_hex(6));
        self.access.insert(a.clone());
        self.refresh.insert(r.clone());
        (a, r)
    }

    pub fn count(&self, op: &str) -> usize {
        self.calls.iter().filter(|(o, _)| o == op).count()
    }

    pub fn ops(&self) -> Vec<String> {
        self.calls.iter().map(|(o, _)| o.clone()).collect()
    }

    fn tick_rise(&mut self) {
        if let Some(n) = self.rise_in {
            if n - 1 <= 0 {
                self.left = self.max;
                self.rise_in = None;
            } else {
                self.rise_in = Some(n - 1);
            }
        }
    }

    /// (status | None = derrubar, corpo, operação)
    fn handle(&mut self, path: &str, req: &Value, auth: &str) -> (Option<u16>, Value, String) {
        if path == "/api/token" {
            return self.token(req);
        }
        if path != "/api/graphql" {
            return (Some(404), json!({}), "404".into());
        }
        let token = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer ")).unwrap_or("");
        let query = req.get("query").and_then(Value::as_str).unwrap_or("");
        let op = req.get("operationName").and_then(Value::as_str).unwrap_or("").to_string();
        if !self.access.contains(token) || tasker::lidl::jwt_exp(token) < now() {
            return (Some(401), json!({"message": "Unauthenticated"}), op);
        }
        if query.contains("bookTariffoptionsDirect") {
            return self.book(op);
        }
        if !self.fail_reads.is_empty() {
            let st = self.fail_reads.remove(0);
            return (Some(st), json!({"errors": [{"message": "Ein Fehler ist aufgetreten"}]}), op);
        }
        if query.contains("consumptions") && query.contains("tariffoptions") && self.reject_combined {
            return (Some(400), json!({"errors": [{"message": "validation"}]}), op);
        }
        let mut data = serde_json::Map::new();
        if query.contains("consumptions") {
            self.tick_rise();
            data.insert("consumptions".into(), json!({"consumptionsForUnit": [
                {"type": "DATA", "left": self.plan_left, "max": 25, "unit": "GB"},
                {"type": "REFILLABLE_DATA", "left": (self.left * 100.0).round() / 100.0, "max": self.max, "unit": "GB"}]}));
        }
        if query.contains("tariffoptions") && !self.omit_tariffs {
            let disabled = if self.cooldown { json!([{"name": "Refill", "tariffoptionId": "CCS_92061"}]) } else { json!([]) };
            data.insert("tariffoptions".into(), json!({"disabledTariffoptions": {"disabledTariffoptions": disabled}}));
        }
        (Some(200), json!({"data": data}), op)
    }

    fn book(&mut self, op: String) -> (Option<u16>, Value, String) {
        match self.refill_mode.as_str() {
            "drop" => {
                self.rise_in = Some(self.rise_after_reads);
                (None, Value::Null, op)
            }
            "success" => {
                self.left = self.max;
                (Some(200), json!({"data": {"bookTariffoptionsDirect": {"success": true}}}), op)
            }
            mode => {
                if mode == "500" {
                    self.rise_in = Some(self.rise_after_reads);
                }
                (Some(500), json!({"errors": [{"message": "Ein Fehler ist aufgetreten. Bitte wende dich an die LIDL Connect Kundenbetreuung"}]}), op)
            }
        }
    }

    fn token(&mut self, req: &Value) -> (Option<u16>, Value, String) {
        let s = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match s("grant_type").as_str() {
            "refresh_token" => {
                let rt = s("refresh_token");
                if !self.refresh.remove(&rt) {
                    return (Some(400), json!({"error": "invalid_grant", "message": "The refresh token is invalid."}), "refresh".into());
                }
                let (a, r) = self.issue(None); // rotaciona: o usado morreu acima
                (Some(200), json!({"access_token": a, "refresh_token": r, "expires_in": self.token_ttl}), "refresh".into())
            }
            "password" => {
                if self.captcha {
                    return (Some(400), json!({"error": "captcha_required", "message": "captcha required"}), "password".into());
                }
                if (s("username"), s("password")) != self.password {
                    return (Some(401), json!({"error": "invalid_credentials", "message": "Benutzername oder Passwort falsch"}), "password".into());
                }
                let (a, r) = self.issue(None);
                (Some(200), json!({"access_token": a, "refresh_token": r, "expires_in": self.token_ttl}), "password".into())
            }
            _ => (Some(400), json!({"error": "unsupported_grant_type"}), "token".into()),
        }
    }
}

pub struct FakeLidl {
    pub s: Arc<Mutex<Fake>>,
    pub origin: String,
}

impl FakeLidl {
    pub fn start() -> FakeLidl {
        let s = Arc::new(Mutex::new(Fake {
            left: 1.0,
            max: 1.0,
            plan_left: 0.0,
            cooldown: false,
            token_ttl: 3600.0,
            access: HashSet::new(),
            refresh: HashSet::new(),
            password: ("01761234567".into(), "certa".into()),
            captcha: false,
            calls: vec![],
            connections: 0,
            fail_reads: vec![],
            refill_mode: "500".into(),
            rise_after_reads: 1,
            rise_in: None,
            reject_combined: false,
            omit_tariffs: false,
            drop_next: false,
        }));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let s2 = s.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                s2.lock().unwrap().connections += 1;
                let s3 = s2.clone();
                std::thread::spawn(move || serve(stream, s3));
            }
        });
        FakeLidl { s, origin }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Fake> {
        self.s.lock().unwrap()
    }
}

fn serve(stream: TcpStream, s: Arc<Mutex<Fake>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut out = stream;
    let mut first = true;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
        let mut len = 0usize;
        let mut auth = String::new();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap_or(0),
                    "authorization" => auth = v.trim().to_string(),
                    _ => {}
                }
            }
        }
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let req: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
        let (status, obj) = {
            let mut g = s.lock().unwrap();
            if !first && g.drop_next {
                g.drop_next = false;
                return; // fecha uma conexão reaproveitada sem responder
            }
            let (status, obj, op) = g.handle(&path, &req, &auth);
            g.calls.push((op, status.map(i32::from).unwrap_or(-1)));
            (status, obj)
        };
        first = false;
        let Some(status) = status else { return };
        let text = obj.to_string();
        let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{text}", text.len());
        if out.write_all(resp.as_bytes()).is_err() {
            return;
        }
    }
}

/// Pasta temporária apagada no fim do teste.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!("tasker-teste-{tag}-{}", tasker::util::random_hex(5)));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
