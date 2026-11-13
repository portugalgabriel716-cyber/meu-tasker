//! Históricos em JSONL (volume e refills) e os arquivos de controle (pausa).
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::paths::Paths;
use crate::store::{get_f64, read_json, write_json};
use crate::types::{History, RefillItem, Resumo};
use crate::util::{local_midnight, now};

// ---------------------------------------------------------------- pausa

/// (pausado?, until). until=0 = até retomarem. Pausa vencida é apagada.
pub fn pause_state(paths: &Paths) -> (bool, f64) {
    let Some(data) = read_json(&paths.pause_file) else {
        return (false, 0.0);
    };
    let until = get_f64(&data, "until").unwrap_or(0.0);
    if until > 0.0 && now() >= until {
        let _ = std::fs::remove_file(&paths.pause_file);
        return (false, 0.0);
    }
    (true, until)
}

pub fn set_pause(paths: &Paths, minutes: f64) -> f64 {
    let until = if minutes > 0.0 { now() + minutes * 60.0 } else { 0.0 };
    let _ = write_json(&paths.pause_file, &json!({"until": until}), false);
    until
}

pub fn clear_pause(paths: &Paths) -> bool {
    std::fs::remove_file(&paths.pause_file).is_ok()
}

// ------------------------------------------------------------- históricos

fn append_line(path: &Path, line: &str) -> std::io::Result<u64> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")?;
    Ok(f.metadata()?.len())
}

/// refills.jsonl: memória curta de refills (a janela mostra e conta).
pub fn record_event(paths: &Paths, kind: &str, mut data: Map<String, Value>) {
    data.insert("t".into(), Value::from(now()));
    data.insert("kind".into(), Value::from(kind));
    let line = Value::Object(data).to_string();
    if let Ok(size) = append_line(&paths.refills_file, &line) {
        if size > 512 * 1024 {
            if let Ok(text) = std::fs::read_to_string(&paths.refills_file) {
                let lines: Vec<&str> = text.lines().collect();
                let keep = &lines[lines.len().saturating_sub(2000)..];
                let _ = std::fs::write(&paths.refills_file, keep.join("\n") + "\n");
            }
        }
    }
}

/// (ok, falhas) registrados a partir de t0 — o teste de estresse mostra.
pub fn refills_since(paths: &Paths, t0: f64) -> (i64, i64) {
    let (mut ok, mut fail) = (0, 0);
    for ev in jsonl_tail(&paths.refills_file, 64 * 1024) {
        if get_f64(&ev, "t").unwrap_or(0.0) >= t0 {
            match ev.get("kind").and_then(Value::as_str) {
                Some("refill_ok") => ok += 1,
                Some("refill_fail") => fail += 1,
                _ => {}
            }
        }
    }
    (ok, fail)
}

pub fn record_volume(paths: &Paths, gb: f64) {
    let line = json!({"t": now().round() as i64, "gb": (gb * 1000.0).round() / 1000.0}).to_string();
    if let Ok(size) = append_line(&paths.volume_file, &line) {
        if size > 1024 * 1024 {
            prune_volume(paths, 7.0 * 24.0 * 3600.0);
        }
    }
}

pub fn prune_volume(paths: &Paths, keep_seconds: f64) {
    let limit = now() - keep_seconds;
    let Ok(text) = std::fs::read_to_string(&paths.volume_file) else { return };
    let mut keep: Vec<&str> = text
        .lines()
        .filter(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|v| v.get("t").and_then(Value::as_f64))
                .is_some_and(|t| t >= limit)
        })
        .collect();
    // Mesmo dentro da janela, nunca deixa crescer sem fim (checagem a cada 15 s).
    if keep.len() > 25000 {
        keep.drain(..keep.len() - 25000);
    }
    let tmp = paths.volume_file.with_extension("jsonl.tmp");
    let body = if keep.is_empty() { String::new() } else { keep.join("\n") + "\n" };
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &paths.volume_file);
    }
}

/// Objetos das últimas linhas do arquivo (até `max_bytes` do fim).
pub fn jsonl_tail(path: &Path, max_bytes: u64) -> Vec<Map<String, Value>> {
    let Ok(mut f) = std::fs::File::open(path) else { return vec![] };
    let size = f.seek(SeekFrom::End(0)).unwrap_or(0);
    let start = size.saturating_sub(max_bytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return vec![];
    }
    let mut buf = vec![];
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // cortada pelo meio
    }
    lines
        .filter_map(|l| match serde_json::from_str::<Value>(l) {
            Ok(Value::Object(m)) => Some(m),
            _ => None,
        })
        .collect()
}

pub fn history(paths: &Paths, hours: i64) -> History {
    let hours = hours.clamp(1, 7 * 24);
    let cut = now() - hours as f64 * 3600.0;
    let mut vol: Vec<[f64; 2]> = jsonl_tail(&paths.volume_file, 2 * 1024 * 1024)
        .iter()
        .filter_map(|d| {
            let t = d.get("t").and_then(Value::as_f64)?;
            let gb = d.get("gb").and_then(Value::as_f64)?;
            (t >= cut).then_some([t, gb])
        })
        .collect();
    // Muitas amostras (checagem a cada 15 s numa semana) pesam e não mudam o
    // desenho: afina pra ~1500 pontos mantendo os vales (o que importa).
    if vol.len() > 1500 {
        let step = vol.len() as f64 / 1500.0;
        let mut thin = Vec::with_capacity(1600);
        let mut i = 0.0f64;
        while (i as usize) < vol.len() {
            let a = i as usize;
            let b = ((i + step) as usize).max(a + 1).min(vol.len());
            if let Some(p) = vol[a..b].iter().min_by(|x, y| x[1].total_cmp(&y[1])) {
                thin.push(*p);
            }
            i += step;
        }
        vol = thin;
    }
    let mut events: Vec<Map<String, Value>> = jsonl_tail(&paths.refills_file, 512 * 1024)
        .into_iter()
        .filter(|d| matches!(d.get("kind").and_then(Value::as_str), Some("refill_ok" | "refill_fail")))
        .collect();
    events.sort_by(|a, b| get_f64(a, "t").unwrap_or(0.0).total_cmp(&get_f64(b, "t").unwrap_or(0.0)));
    let is_ok = |e: &Map<String, Value>| e.get("kind").and_then(Value::as_str) == Some("refill_ok");
    let t_of = |e: &Map<String, Value>| get_f64(e, "t").unwrap_or(0.0);
    let in_range: Vec<&Map<String, Value>> = events.iter().filter(|e| t_of(e) >= cut).collect();
    let midnight = local_midnight();
    let oks_today = events.iter().filter(|e| is_ok(e) && t_of(e) >= midnight).count();
    let lat: Vec<f64> = in_range.iter().filter(|e| is_ok(e)).filter_map(|e| get_f64(e, "latency")).collect();
    let refills: Vec<RefillItem> = in_range
        .iter()
        .map(|e| RefillItem {
            t: t_of(e),
            ok: is_ok(e),
            before: get_f64(e, "before"),
            after: get_f64(e, "after"),
            why: e.get("why").and_then(Value::as_str).unwrap_or("").to_string(),
            latencia: get_f64(e, "latency"),
        })
        .collect();
    let refills = refills[refills.len().saturating_sub(200)..].to_vec();
    History {
        volume: vol,
        refills,
        resumo: Resumo {
            refills_hoje: oks_today as i64,
            refills_periodo: in_range.iter().filter(|e| is_ok(e)).count() as i64,
            falhas_periodo: in_range.iter().filter(|e| !is_ok(e)).count() as i64,
            latencia_media: (!lat.is_empty())
                .then(|| (lat.iter().sum::<f64>() / lat.len() as f64 * 10.0).round() / 10.0),
            horas: hours,
        },
    }
}
