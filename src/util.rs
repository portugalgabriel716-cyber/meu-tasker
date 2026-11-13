//! Pequenas coisas usadas em todo canto: relógio, formatação, ambiente, aleatório.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use chrono::{Local, TimeZone};

/// Variáveis de ambiente, capturadas uma vez. O motor e os ajustes recebem isto
/// em vez de ler o processo direto: os testes passam um ambiente próprio sem
/// brigar entre si.
pub type Env = Arc<HashMap<String, String>>;

pub fn process_env() -> Env {
    Arc::new(std::env::vars().collect())
}

pub fn empty_env() -> Env {
    Arc::new(HashMap::new())
}

/// Valor não vazio (depois de tirar espaços) de uma variável.
pub fn env_get<'a>(env: &'a Env, key: &str) -> Option<&'a str> {
    env.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `strftime` no fuso local.
pub fn fmt_local(ts: f64, fmt: &str) -> String {
    let secs = ts.floor() as i64;
    let nanos = ((ts - ts.floor()) * 1e9) as u32;
    match Local.timestamp_opt(secs, nanos) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            t.format(fmt).to_string()
        }
        chrono::LocalResult::None => String::new(),
    }
}

pub fn hhmm(ts: f64) -> String {
    fmt_local(ts, "%H:%M")
}

/// Meia-noite de hoje (fuso local), em segundos.
pub fn local_midnight() -> f64 {
    let today = Local::now().date_naive();
    let midnight = today.and_hms_opt(0, 0, 0).unwrap_or_default();
    match Local.from_local_datetime(&midnight) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.timestamp() as f64,
        chrono::LocalResult::None => now() - now() % 86400.0,
    }
}

pub fn same_local_day(a: f64, b: f64) -> bool {
    fmt_local(a, "%Y-%m-%d") == fmt_local(b, "%Y-%m-%d")
}

/// Como o `%g` do Python: até 6 algarismos significativos, sem zeros sobrando.
pub fn fmt_g(x: f64) -> String {
    if !x.is_finite() {
        return x.to_string();
    }
    if x == 0.0 {
        return "0".into();
    }
    let mag = x.abs().log10().floor() as i32;
    let decimals = (5 - mag).clamp(0, 12) as usize;
    let s = format!("{:.*}", decimals, x);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

/// Número com vírgula (a tela fala português).
pub fn comma(s: String) -> String {
    s.replace('.', ",")
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if getrandom::fill(&mut buf).is_err() {
        // Sem fonte do sistema (não deveria acontecer): mistura relógio e PID.
        let mut x = (now() * 1e9) as u64 ^ ((std::process::id() as u64) << 32);
        for b in buf.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
    }
    buf
}

pub fn random_hex(n_bytes: usize) -> String {
    random_bytes(n_bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_token(n_bytes: usize) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(n_bytes))
}

/// Comparação em tempo constante (chave da tela local).
pub fn const_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Primeiros `n` caracteres, sem quebra de linha (pra log).
pub fn snippet(text: &str, n: usize) -> String {
    text.chars().take(n).collect::<String>().replace('\n', " ")
}

/// Duração legível: "45 s", "3 min 20 s", "2 h 5 min".
pub fn dur(s: f64) -> String {
    let s = s.max(0.0).round() as i64;
    if s < 60 {
        return format!("{s} s");
    }
    if s < 3600 {
        let extra = if s % 60 != 0 && s < 600 { format!(" {} s", s % 60) } else { String::new() };
        return format!("{} min{}", s / 60, extra);
    }
    format!("{} h {} min", s / 3600, (s % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_format() {
        assert_eq!(fmt_g(1.0), "1");
        assert_eq!(fmt_g(0.85), "0.85");
        assert_eq!(fmt_g(25.0), "25");
        assert_eq!(fmt_g(0.1 + 0.2), "0.3");
        assert_eq!(fmt_g(1500.0), "1500");
        assert_eq!(fmt_g(-0.0), "0");
    }

    #[test]
    fn durations() {
        assert_eq!(dur(5.0), "5 s");
        assert_eq!(dur(200.0), "3 min 20 s");
        assert_eq!(dur(900.0), "15 min");
        assert_eq!(dur(7500.0), "2 h 5 min");
    }
}
