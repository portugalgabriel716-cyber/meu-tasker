//! Cliente HTTP da API da Lidl Connect.
//!
//! Medido contra a API de verdade (out/2026): com a conexão HTTPS mantida
//! aberta, a primeira chamada leva ~1 s (aperto de mão TLS) e as seguintes
//! ~130 ms. Por isso a conexão é reaproveitada entre checagens, e volume +
//! cooldown vêm numa query só (`painel`): quando o refill precisa sair, a
//! mutation vai na hora, sem mais uma ida e volta pra conferir o cooldown.
//!
//! Três coisas que não são adivinháveis:
//!   * a mutation de refill registra a reserva e AINDA devolve HTTP 500 genérico
//!     (o botão do site toma o mesmo 500) — ela nunca é repetida às cegas;
//!   * o refresh token ROTACIONA: cada renovação mata o anterior;
//!   * a conta aceita uma sessão por vez.
use std::io::ErrorKind;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};

use crate::util::{now, random_hex, snippet};

pub const API_ORIGIN: &str = "https://api.lidl-connect.de";
pub const PORTAL_ORIGIN: &str = "https://kundenkonto.lidl-connect.de";
pub const PORTAL_URL: &str = "https://kundenkonto.lidl-connect.de/mein-lidl-connect/uebersicht.html";
// Credenciais públicas do cliente OAuth do portal (estão no bundle JS do site).
const CLIENT_ID: &str = "lidl";
const CLIENT_SECRET: &str = "lidl";
/// tariffoption do botão "Refill aktivieren".
pub const REFILL_OPTION_ID: &str = "CCS_92061";

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                          Chrome/148.0.0.0 Safari/537.36";

const UNITS: &str = "consumptionsForUnit { type left max unit __typename }";
const DISABLED: &str = "disabledTariffoptions { disabledTariffoptions { name tariffoptionId } }";

pub fn painel_query() -> String {
    format!("query painel {{ consumptions {{ {UNITS} __typename }} tariffoptions {{ {DISABLED} }} }}")
}
pub fn consumptions_query() -> String {
    format!("query consumptions {{ consumptions {{ {UNITS} __typename }} }}")
}
pub fn tariffoptions_query() -> String {
    format!("query tariffOptions {{ tariffoptions {{ {DISABLED} }} }}")
}
/// Idêntica à do app (só `success`): campo a mais é risco de erro sem ganho.
const REFILL_MUTATION: &str = "mutation bookTariffOptionsDirect\
($bookTariffoptionsDirectInput: BookTariffoptionsDirectInput!) \
{ bookTariffoptionsDirect(bookTariffoptionsDirectInput: \
$bookTariffoptionsDirectInput) { success __typename } }";

/// Validade lida do próprio JWT (não valida assinatura). 0 se não der.
pub fn jwt_exp(token: &str) -> f64 {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return 0.0;
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].trim_end_matches('='))
        .ok();
    raw.and_then(|r| serde_json::from_slice::<Value>(&r).ok())
        .and_then(|v| v.get("exp").and_then(Value::as_f64))
        .unwrap_or(0.0)
}

/// O hash do bundle do site (vai no header X-Transaction).
pub fn js_hash(text: &str) -> i64 {
    let mut acc: i64 = 0;
    for ch in text.encode_utf16() {
        acc = (acc.wrapping_mul(31) + ch as i64) & 0xFFFF_FFFF;
        if acc >= 0x8000_0000 {
            acc -= 0x1_0000_0000;
        }
    }
    acc + 2_147_483_647
}

pub fn x_transaction(label: &str, access_token: Option<&str>) -> String {
    let ident = match access_token {
        Some(t) if !t.is_empty() => js_hash(t).to_string(),
        _ => "anonymous".into(),
    };
    format!("{label}-{}-{ident}", random_hex(4))
}

#[derive(Debug, Clone)]
pub struct Response {
    /// -1 = nem houve resposta (rede, timeout, DNS).
    pub status: i32,
    pub text: String,
    pub ms: u64,
}

impl Response {
    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.text).ok()
    }

    pub fn json_obj(&self) -> serde_json::Map<String, Value> {
        match self.json() {
            Some(Value::Object(m)) => m,
            _ => Default::default(),
        }
    }

    pub fn snippet(&self, n: usize) -> String {
        snippet(&self.text, n)
    }
}

pub struct LidlApi {
    origin: String,
    timeout: Duration,
    agent: ureq::Agent,
    last_use: f64,
}

impl LidlApi {
    pub fn new(origin: Option<&str>, timeout_secs: u64) -> LidlApi {
        let timeout = Duration::from_secs(timeout_secs);
        LidlApi {
            origin: origin.unwrap_or(API_ORIGIN).trim_end_matches('/').to_string(),
            timeout,
            agent: Self::make_agent(timeout),
            last_use: 0.0,
        }
    }

    fn make_agent(timeout: Duration) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout(timeout)
            .max_idle_connections_per_host(2)
            .build()
    }

    /// Fecha a conexão aberta (a próxima chamada abre outra).
    pub fn close(&mut self) {
        self.agent = Self::make_agent(self.timeout);
    }

    /// POST JSON. Leitura repete uma vez se a conexão estava velha.
    ///
    /// A mutation (idempotent=false) nunca repete: se a conexão está parada há
    /// mais de alguns segundos ela é refeita ANTES de enviar, e qualquer falha
    /// depois de enviar vira "não sei" pra quem chamou decidir pelo efeito.
    pub fn post(&mut self, path: &str, payload: &Value, headers: &[(&str, String)], idempotent: bool) -> Response {
        let body = payload.to_string();
        if !idempotent && now() - self.last_use > 5.0 {
            self.close();
        }
        let attempts = if idempotent { 2 } else { 1 };
        let mut last = Response { status: -1, text: "sem resposta".into(), ms: 0 };
        for attempt in 0..attempts {
            let started = Instant::now();
            let mut req = self
                .agent
                .post(&format!("{}{}", self.origin, path))
                .set("content-type", "application/json")
                .set("accept", "application/json, text/plain, */*")
                .set("accept-language", "de-DE")
                .set("origin", PORTAL_ORIGIN)
                .set("referer", &format!("{PORTAL_ORIGIN}/"))
                .set("user-agent", USER_AGENT);
            for (k, v) in headers {
                req = req.set(k, v);
            }
            let result = req.send_string(&body);
            let ms = started.elapsed().as_millis() as u64;
            let resp = match result {
                Ok(r) => r,
                Err(ureq::Error::Status(_, r)) => r,
                Err(ureq::Error::Transport(t)) => {
                    self.close();
                    let timed_out = io_kind(&t).is_some_and(|k| matches!(k, ErrorKind::TimedOut | ErrorKind::WouldBlock));
                    last = Response { status: -1, text: format!("{:?}: {}", t.kind(), t), ms };
                    if attempt + 1 < attempts && !timed_out {
                        continue; // conexão velha: o servidor fechou enquanto dormíamos
                    }
                    return last;
                }
            };
            let status = resp.status() as i32;
            let close = resp.header("connection").is_some_and(|c| c.eq_ignore_ascii_case("close"));
            let text = match resp.into_string() {
                Ok(t) => t,
                Err(e) => {
                    self.close();
                    return Response { status: -1, text: format!("resposta cortada: {e}"), ms };
                }
            };
            self.last_use = now();
            if close {
                self.close();
            }
            return Response { status, text, ms: started.elapsed().as_millis() as u64 };
        }
        last
    }

    // -- token -------------------------------------------------------------

    pub fn refresh(&mut self, refresh_token: &str, access_token: Option<&str>) -> Response {
        let payload = json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "refresh_token": refresh_token,
        });
        self.post("/api/token", &payload, &[("X-Transaction", x_transaction("refresh", access_token))], true)
    }

    pub fn password_login(&mut self, msisdn: &str, password: &str) -> Response {
        // Mesmo corpo do site quando não há captcha.
        let payload = json!({
            "grant_type": "password",
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "username": msisdn,
            "password": password,
            "captcha_code": null,
            "captcha_token": null,
        });
        self.post("/api/token", &payload, &[("X-Transaction", x_transaction("login", None))], false)
    }

    // -- GraphQL -------------------------------------------------------------

    /// O app manda X-Transaction nas queries mas NÃO na mutation — igual aqui.
    pub fn graphql(&mut self, operation: &str, query: &str, token: &str, variables: Value, mutation: bool) -> Response {
        let mut headers = vec![("authorization", format!("Bearer {token}")), ("accept", "*/*".to_string())];
        if !mutation {
            headers.push(("X-Transaction", x_transaction(operation, Some(token))));
        }
        let payload = json!({"operationName": operation, "variables": variables, "query": query});
        self.post("/api/graphql", &payload, &headers, !mutation)
    }

    pub fn painel(&mut self, token: &str) -> Response {
        self.graphql("painel", &painel_query(), token, json!({}), false)
    }

    pub fn book_refill(&mut self, token: &str) -> Response {
        let vars = json!({"bookTariffoptionsDirectInput": {
            "bookTariffoptions": [{"tariffoptionId": REFILL_OPTION_ID}]}});
        self.graphql("bookTariffOptionsDirect", REFILL_MUTATION, token, vars, true)
    }
}

fn io_kind(t: &ureq::Transport) -> Option<ErrorKind> {
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(t);
    while let Some(e) = src {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            return Some(io.kind());
        }
        src = e.source();
    }
    None
}

/// Unidades de consumo que interessam.
#[derive(Debug, Clone, Default)]
pub struct Units {
    pub refill: Option<Value>,
    pub data: Option<Value>,
}

/// data do GraphQL -> (unidades, cooldown).
///
/// unidades = None se a parte de consumo não veio. cooldown = None se a parte
/// de tarifas não veio (não saber o cooldown NUNCA pode virar "pode reservar").
pub fn parse_painel(data: Option<&Value>) -> (Option<Units>, Option<bool>) {
    let Some(data) = data.filter(|d| d.is_object()) else {
        return (None, None);
    };
    let units = data
        .get("consumptions")
        .and_then(|c| c.get("consumptionsForUnit"))
        .and_then(|list| match list {
            Value::Array(items) => Some(items.clone()),
            Value::Null => Some(vec![]),
            _ => None,
        })
        .map(|items| {
            let mut u = Units::default();
            for unit in items {
                match unit.get("type").and_then(Value::as_str) {
                    Some("REFILLABLE_DATA") => u.refill = Some(unit),
                    Some("DATA") => u.data = Some(unit),
                    _ => {}
                }
            }
            u
        });
    let cooldown = data
        .get("tariffoptions")
        .and_then(|t| t.get("disabledTariffoptions"))
        .and_then(|d| d.get("disabledTariffoptions"))
        .and_then(|list| match list {
            Value::Array(items) => Some(
                items.iter().any(|o| o.get("tariffoptionId").and_then(Value::as_str) == Some(REFILL_OPTION_ID)),
            ),
            Value::Null => Some(false),
            _ => None,
        });
    (units, cooldown)
}

pub fn as_float(v: Option<&Value>) -> Option<f64> {
    crate::store::as_f64(v)
}
