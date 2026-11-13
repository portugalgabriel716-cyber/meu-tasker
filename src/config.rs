//! Ajustes: um lugar só descreve cada opção (rótulo, explicação, padrão, limites).
//!
//! Precedência:  padrão do código  <  dados/config.json  <  variável TASKER_*
//!
//! O ambiente vence de propósito: mexer na tela nunca pode desligar um TASKER_*
//! posto num serviço. Valores são guardados em segundos/GB/número puro; `unit`
//! diz só como a tela mostra. As chaves são as mesmas das versões anteriores.
use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::paths::Paths;
use crate::store::{read_json, write_json};
use crate::util::{fmt_g, Env};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Int,
    Float,
    Bool,
    Str,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Int => "int",
            Kind::Float => "float",
            Kind::Bool => "bool",
            Kind::Str => "str",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    None,
    /// vazio ou http(s)://...
    UrlOrEmpty,
    /// [A-Za-z0-9_-]* ou http(s)://...
    TopicOrUrl,
}

#[derive(Debug, Clone, Copy)]
pub enum Default {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(&'static str),
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub key: &'static str,
    pub env: &'static str,
    pub default: Default,
    pub kind: Kind,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub max_len: usize,
    pub pattern: Pattern,
    pub group: &'static str,
    pub label: &'static str,
    pub help: &'static str,
}

#[allow(clippy::too_many_arguments)]
const fn num(
    key: &'static str, env: &'static str, default: Default, kind: Kind, unit: &'static str,
    min: f64, max: f64, group: &'static str, label: &'static str, help: &'static str,
) -> Spec {
    Spec { key, env, default, kind, unit, min, max, max_len: 0, pattern: Pattern::None, group, label, help }
}

const fn flag(key: &'static str, env: &'static str, default: bool, group: &'static str,
              label: &'static str, help: &'static str) -> Spec {
    Spec { key, env, default: Default::Bool(default), kind: Kind::Bool, unit: "", min: 0.0, max: 0.0,
           max_len: 0, pattern: Pattern::None, group, label, help }
}

const fn text(key: &'static str, env: &'static str, max_len: usize, pattern: Pattern,
              group: &'static str, label: &'static str, help: &'static str) -> Spec {
    Spec { key, env, default: Default::Str(""), kind: Kind::Str, unit: "", min: 0.0, max: 0.0,
           max_len, pattern, group, label, help }
}

pub const SETTINGS: &[Spec] = &[
    // ------------------------------------------------------------- checagem
    num("check_interval", "TASKER_CHECK_INTERVAL", Default::Int(60), Kind::Int, "s", 15.0, 1800.0,
        "Checagem", "Olhar o volume a cada",
        "Intervalo normal entre as perguntas à Lidl. Com a checagem adaptativa ligada, ele \
         encurta sozinho quando o volume está acabando rápido."),
    flag("adaptive", "TASKER_ADAPTIVE", true, "Checagem", "Checagem adaptativa",
         "Mede o ritmo de consumo e, quando o volume vai cruzar o limite antes da próxima \
          checagem, olha mais cedo. O refill sai perto do limite em vez de bem abaixo dele."),
    num("fast_interval", "TASKER_FAST_INTERVAL", Default::Int(15), Kind::Int, "s", 5.0, 300.0,
        "Checagem", "Intervalo mais curto permitido",
        "O mínimo que a checagem adaptativa pode chegar. Também é o ritmo usado pra confirmar \
         um refill recém-pedido."),
    num("gb_threshold", "TASKER_GB_THRESHOLD", Default::Float(0.85), Kind::Float, "GB", 0.05, 5.0,
        "Checagem", "Pedir refill quando sobrar",
        "Quando o volume restante chegar nesse valor (ou menos), o Tasker dispara o Refill. O \
         pacote é de 1 GB."),
    num("max_without_refill", "TASKER_MAX_WITHOUT_REFILL", Default::Int(13 * 60), Kind::Int, "min",
        300.0, 43200.0, "Checagem", "Forçar refill se passar",
        "Rede parada não gasta volume, e aí o limite acima nunca chega. Passado esse tempo o \
         Tasker pede refill mesmo assim (com o balde cheio ele só zera o relógio)."),
    // ------------------------------------------------------ quando dá errado
    num("pending_grace", "TASKER_PENDING_GRACE", Default::Int(150), Kind::Int, "s", 30.0, 900.0,
        "Quando dá errado", "Esperar o refill aparecer por",
        "A Lidl responde erro 500 mesmo quando o refill deu certo, então quem decide é o volume \
         subir. Este é o prazo pra ele subir antes de contar como falha."),
    num("refill_backoff_max", "TASKER_REFILL_BACKOFF_MAX", Default::Int(30 * 60), Kind::Int, "min",
        60.0, 14400.0, "Quando dá errado", "Espera máxima após refill falhar",
        "A cada falha seguida a espera dobra, até esse teto."),
    num("token_margin", "TASKER_TOKEN_MARGIN", Default::Int(5 * 60), Kind::Int, "min", 60.0, 2700.0,
        "Quando dá errado", "Renovar o token com antecedência de",
        "O token da API dura 1 h. Renovar um pouco antes evita a corrida entre expirar e usar."),
    num("refresh_retry_delay", "TASKER_REFRESH_RETRY_DELAY", Default::Int(60), Kind::Int, "s", 10.0,
        900.0, "Quando dá errado", "Espera pra tentar o token de novo",
        "Depois de uma renovação que falhou por rede/instabilidade. Token recusado de vez espera \
         bem mais (30 min)."),
    // ---------------------------------------------------------------- login
    num("login_backoff", "TASKER_LOGIN_BACKOFF", Default::Int(5 * 60), Kind::Int, "min", 60.0,
        14400.0, "Login automático", "Espera entre tentativas de login",
        "Só vale com número e senha guardados. A espera dobra a cada falha."),
    num("login_max_fails", "TASKER_LOGIN_MAX_FAILS", Default::Int(3), Kind::Int, "x", 1.0, 5.0,
        "Login automático", "Senhas erradas antes de parar",
        "A Lidl trava a conta depois de algumas senhas erradas. Este limite garante que o Tasker \
         nunca seja o motivo — deixe baixo."),
    num("login_block_long", "TASKER_LOGIN_BLOCK_LONG", Default::Int(6 * 3600), Kind::Int, "h",
        3600.0, 172800.0, "Login automático", "Parada longa após o limite",
        "Depois de estourar o limite acima, o Tasker fica esse tempo sem tentar login e avisa você."),
    // ------------------------------------------------------ teste de estresse
    num("stress_streams", "TASKER_STRESS_STREAMS", Default::Int(6), Kind::Int, "x", 1.0, 32.0,
        "Teste de estresse", "Conexões paralelas",
        "Quantos downloads ao mesmo tempo. Mais conexões enchem melhor uma rede rápida; 6 \
         costuma bastar pra 4G/5G."),
    num("stress_max_gb", "TASKER_STRESS_MAX_GB", Default::Float(3.0), Kind::Float, "GB", 0.1, 200.0,
        "Teste de estresse", "Parar depois de baixar",
        "O teste consome volume de verdade (é o objetivo). Ele para sozinho ao chegar aqui."),
    num("stress_max_minutes", "TASKER_STRESS_MAX_MINUTES", Default::Int(15 * 60), Kind::Int, "min",
        60.0, 21600.0, "Teste de estresse", "Parar depois de",
        "Limite de tempo, valendo o que vier primeiro com o de cima."),
    text("stress_url", "TASKER_STRESS_URL", 500, Pattern::UrlOrEmpty, "Teste de estresse",
         "Arquivo de teste próprio (opcional)",
         "Vazio = servidores de teste de velocidade da Hetzner, OVH e Tele2 (arquivos de 10 GB, \
          revezando). Ponha uma URL se quiser usar outro."),
    // --------------------------------------------------------------- avisos
    flag("notify", "TASKER_NOTIFY", true, "Avisos", "Avisos na área de trabalho",
         "Notificação quando precisa de login ou o refill falha várias vezes seguidas."),
    text("ntfy_topic", "TASKER_NTFY_TOPIC", 200, Pattern::TopicOrUrl, "Avisos",
         "Tópico ntfy.sh (push no celular)",
         "Opcional. Assine no app ntfy um tópico com nome difícil de adivinhar e escreva o mesmo \
          nome aqui (ou a URL de um servidor ntfy seu). Tópico ntfy não tem senha: quem souber o \
          nome lê."),
    text("webhook", "TASKER_WEBHOOK", 500, Pattern::UrlOrEmpty, "Avisos", "Webhook (opcional)",
         "Recebe um POST JSON {\"text\": \"...\"} a cada aviso — serve pra Slack, Discord \
          (/slack), Telegram via ponte, etc."),
];

pub fn spec(key: &str) -> Option<&'static Spec> {
    SETTINGS.iter().find(|s| s.key == key)
}

pub fn groups() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = vec![];
    for s in SETTINGS {
        if !out.contains(&s.group) {
            out.push(s.group);
        }
    }
    out
}

fn unit_seconds(unit: &str) -> f64 {
    match unit {
        "min" => 60.0,
        "h" => 3600.0,
        _ => 1.0,
    }
}

pub fn unit_label(unit: &str) -> &'static str {
    match unit {
        "s" => "segundos",
        "min" => "minutos",
        "h" => "horas",
        "GB" => "GB",
        "x" => "vezes",
        _ => "",
    }
}

/// Um valor de ajuste, já no tipo certo.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl Val {
    pub fn to_json(&self) -> Value {
        match self {
            Val::Int(i) => Value::from(*i),
            Val::Float(f) => Value::from(*f),
            Val::Bool(b) => Value::from(*b),
            Val::Str(s) => Value::from(s.clone()),
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Val::Int(i) => *i as f64,
            Val::Float(f) => *f,
            Val::Bool(b) => *b as i64 as f64,
            Val::Str(_) => 0.0,
        }
    }
}

impl Spec {
    pub fn default_val(&self) -> Val {
        match self.default {
            Default::Int(i) => Val::Int(i),
            Default::Float(f) => Val::Float(f),
            Default::Bool(b) => Val::Bool(b),
            Default::Str(s) => Val::Str(s.to_string()),
        }
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on" | "sim"),
        Value::Number(n) => n.as_i64() == Some(1),
        _ => false,
    }
}

fn pattern_ok(p: Pattern, val: &str) -> bool {
    let url = |s: &str| {
        let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://"));
        rest.is_some_and(|r| !r.is_empty() && !r.chars().any(char::is_whitespace))
    };
    match p {
        Pattern::None => true,
        Pattern::UrlOrEmpty => val.is_empty() || url(val),
        Pattern::TopicOrUrl => {
            val.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || url(val)
        }
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Texto/JSON -> valor do tipo certo, dentro dos limites.
pub fn coerce(spec: &Spec, raw: &Value) -> Result<Val, String> {
    match spec.kind {
        Kind::Bool => return Ok(Val::Bool(truthy(raw))),
        Kind::Str => {
            let val = value_text(raw).trim().to_string();
            if val.chars().count() > spec.max_len {
                return Err(format!("máximo de {} caracteres", spec.max_len));
            }
            if !val.is_empty() && !pattern_ok(spec.pattern, &val) {
                return Err("formato inválido".into());
            }
            return Ok(Val::Str(val));
        }
        Kind::Int | Kind::Float => {}
    }
    let num = match raw {
        Value::Bool(_) => return Err("não é um número".into()),
        Value::Number(n) => n.as_f64().ok_or("não é um número")?,
        Value::String(s) => {
            // quem digita em português usa vírgula
            let t = s.trim().replace(',', ".");
            if t.is_empty() {
                return Err("está vazio".into());
            }
            t.parse::<f64>().map_err(|_| format!("'{}' não é um número", s.trim()))?
        }
        _ => return Err("não é um número".into()),
    };
    if !num.is_finite() {
        return Err("não é um número".into());
    }
    let val = if spec.kind == Kind::Int { Val::Int(num.trunc() as i64) } else { Val::Float(num) };
    let x = val.as_f64();
    if x < spec.min {
        return Err(format!("mínimo é {}", fmt(spec, &num_val(spec, spec.min))));
    }
    if x > spec.max {
        return Err(format!("máximo é {}", fmt(spec, &num_val(spec, spec.max))));
    }
    Ok(val)
}

fn num_val(spec: &Spec, x: f64) -> Val {
    if spec.kind == Kind::Int { Val::Int(x as i64) } else { Val::Float(x) }
}

/// Valor guardado -> como a tela mostra (minutos, horas...).
pub fn to_display(spec: &Spec, value: &Val) -> Value {
    match (spec.kind, value) {
        (Kind::Bool, _) | (Kind::Str, _) => value.to_json(),
        _ => {
            let div = unit_seconds(spec.unit);
            if div == 1.0 {
                return value.to_json();
            }
            let v = value.as_f64() / div;
            if spec.kind == Kind::Int && v == v.trunc() {
                Value::from(v as i64)
            } else {
                Value::from((v * 100.0).round() / 100.0)
            }
        }
    }
}

pub fn from_display(spec: &Spec, shown: &Value) -> Result<Val, String> {
    if matches!(spec.kind, Kind::Bool | Kind::Str) {
        return coerce(spec, shown);
    }
    let num = match shown {
        Value::Number(n) => n.as_f64().ok_or("não é um número")?,
        Value::String(s) => {
            let t = s.trim().replace(',', ".");
            if t.is_empty() {
                return Err("está vazio".into());
            }
            t.parse::<f64>().map_err(|_| format!("'{}' não é um número", s.trim()))?
        }
        _ => return Err("não é um número".into()),
    };
    // Arredonda pra não virar 839.9999 segundos por causa do ponto flutuante.
    let raw = (num * unit_seconds(spec.unit) * 1e6).round() / 1e6;
    coerce(spec, &Value::from(raw))
}

pub fn fmt(spec: &Spec, value: &Val) -> String {
    match value {
        Val::Bool(b) => (if *b { "ligado" } else { "desligado" }).into(),
        Val::Str(s) => if s.is_empty() { "vazio".into() } else { s.clone() },
        _ => {
            let shown = to_display(spec, value).as_f64().unwrap_or(0.0);
            let suffix = match spec.unit {
                "s" => " s",
                "min" => " min",
                "h" => " h",
                "GB" => " GB",
                "x" => "x",
                _ => "",
            };
            format!("{}{}", fmt_g(shown), suffix)
        }
    }
}

pub type Values = BTreeMap<String, Val>;
pub type Sources = BTreeMap<String, &'static str>;

pub fn defaults() -> Values {
    SETTINGS.iter().map(|s| (s.key.to_string(), s.default_val())).collect()
}

/// Só o que está no arquivo. Corrompido/ausente = vazio (nunca impede de subir).
pub fn read_file(paths: &Paths) -> Map<String, Value> {
    read_json(&paths.config_file).unwrap_or_default()
}

/// (valores, origens) — origem é "padrão", "arquivo" ou "ambiente".
pub fn load_with_sources(paths: &Paths, env: &Env) -> (Values, Sources) {
    let arquivo = read_file(paths);
    let mut values = Values::new();
    let mut sources = Sources::new();
    for spec in SETTINGS {
        let mut v = spec.default_val();
        let mut src = "padrão";
        if let Some(raw) = arquivo.get(spec.key) {
            if let Ok(x) = coerce(spec, raw) {
                v = x;
                src = "arquivo";
            }
        }
        if let Some(raw) = env.get(spec.env).filter(|s| !s.trim().is_empty()) {
            if let Ok(x) = coerce(spec, &Value::from(raw.clone())) {
                v = x;
                src = "ambiente";
            }
        }
        values.insert(spec.key.to_string(), v);
        sources.insert(spec.key.to_string(), src);
    }
    (values, sources)
}

pub fn load(paths: &Paths, env: &Env) -> Cfg {
    Cfg::from_values(&load_with_sources(paths, env).0)
}

/// Grava só chaves conhecidas e válidas (as obsoletas da versão 1 caem aqui).
pub fn save(paths: &Paths, values: &Values) -> Result<(), String> {
    let mut clean = Map::new();
    for spec in SETTINGS {
        if let Some(v) = values.get(spec.key) {
            let checked = coerce(spec, &v.to_json())?;
            clean.insert(spec.key.to_string(), checked.to_json());
        }
    }
    write_json(&paths.config_file, &Value::Object(clean), false).map_err(|e| e.to_string())
}

/// Combinações que passam na validação mas se mordem.
pub fn warnings(v: &Values) -> Vec<String> {
    let g = |k: &str| v.get(k).map(Val::as_f64).unwrap_or(0.0);
    let mut out = vec![];
    if g("token_margin") >= 55.0 * 60.0 {
        out.push("O token dura 1 h — com essa margem ele seria renovado sem parar.".into());
    }
    if g("gb_threshold") >= 1.0 {
        out.push("O pacote da Lidl é de 1 GB — pedir refill com 1 GB ou mais sobrando dispara sempre.".into());
    }
    if g("fast_interval") > g("check_interval") {
        out.push("O intervalo mais curto é maior que o normal: a checagem adaptativa não tem como \
                  encurtar nada.".into());
    }
    if g("fast_interval") * 3.0 > g("pending_grace") {
        out.push("O prazo pra confirmar o refill é curto pro intervalo rápido: sobram menos de 3 \
                  conferências antes de contar como falha.".into());
    }
    out
}

/// Os ajustes já tipados, como o motor usa (tempos em segundos).
#[derive(Debug, Clone)]
pub struct Cfg {
    pub check_interval: f64,
    pub adaptive: bool,
    pub fast_interval: f64,
    pub gb_threshold: f64,
    pub max_without_refill: f64,
    pub pending_grace: f64,
    pub refill_backoff_max: f64,
    pub token_margin: f64,
    pub refresh_retry_delay: f64,
    pub login_backoff: f64,
    pub login_max_fails: i64,
    pub login_block_long: f64,
    pub stress_streams: usize,
    pub stress_max_gb: f64,
    pub stress_max_minutes: f64,
    pub stress_url: String,
    pub notify: bool,
    pub ntfy_topic: String,
    pub webhook: String,
}

impl Cfg {
    pub fn from_values(v: &Values) -> Cfg {
        let f = |k: &str| v.get(k).map(Val::as_f64).unwrap_or(0.0);
        let b = |k: &str| matches!(v.get(k), Some(Val::Bool(true)));
        let s = |k: &str| match v.get(k) {
            Some(Val::Str(x)) => x.clone(),
            _ => String::new(),
        };
        Cfg {
            check_interval: f("check_interval"),
            adaptive: b("adaptive"),
            fast_interval: f("fast_interval"),
            gb_threshold: f("gb_threshold"),
            max_without_refill: f("max_without_refill"),
            pending_grace: f("pending_grace"),
            refill_backoff_max: f("refill_backoff_max"),
            token_margin: f("token_margin"),
            refresh_retry_delay: f("refresh_retry_delay"),
            login_backoff: f("login_backoff"),
            login_max_fails: f("login_max_fails") as i64,
            login_block_long: f("login_block_long"),
            stress_streams: f("stress_streams").max(1.0) as usize,
            stress_max_gb: f("stress_max_gb"),
            stress_max_minutes: f("stress_max_minutes"),
            stress_url: s("stress_url"),
            notify: b("notify"),
            ntfy_topic: s("ntfy_topic"),
            webhook: s("webhook"),
        }
    }
}

impl std::default::Default for Cfg {
    fn default() -> Cfg {
        Cfg::from_values(&defaults())
    }
}
