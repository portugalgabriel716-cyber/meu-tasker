//! Teste de estresse do download: puxa o máximo que a rede der, pra ver o refill
//! aguentando consumo pesado.
//!
//! Baixa arquivos grandes de servidores feitos pra teste de velocidade, em várias
//! conexões paralelas, e joga os bytes fora na memória (nada vai pro disco: sem
//! desgaste de SSD e quase sem CPU). As conexões são espalhadas entre os
//! servidores, e um servidor que falha fica de molho um minuto.
//!
//! Para sozinho no limite de GB ou de tempo — consumir volume é o objetivo, mas
//! um teste esquecido ligado não pode comer o mês inteiro.
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::logs::Logger;
use crate::types::StressInfo;
use crate::util::now;

pub const MIRRORS: &[&str] = &[
    "https://nbg1-speed.hetzner.com/10GB.bin", // Nuremberg
    "https://fsn1-speed.hetzner.com/10GB.bin", // Falkenstein
    "https://proof.ovh.net/files/10Gb.dat",    // França
    "https://hel1-speed.hetzner.com/10GB.bin", // Helsinque
    "http://speedtest.tele2.net/10GB.zip",     // Suécia
];
const CHUNK: usize = 256 * 1024;
const USER_AGENT: &str = "Tasker-stress/3 (+teste de download)";

#[derive(Default)]
struct Run {
    state: String,
    started: Option<f64>,
    ended: Option<f64>,
    limit_bytes: u64,
    limit_seconds: u64,
    peak_bps: f64,
    min_volume: Option<f64>,
    reason: String,
    urls: Vec<String>,
    /// (instante, bytes acumulados), últimos segundos.
    samples: Vec<(f64, u64)>,
    /// url -> instante até quando fica de molho.
    bench: HashMap<String, f64>,
}

pub struct StressTest {
    log: Arc<Logger>,
    volume: Arc<dyn Fn() -> Option<f64> + Send + Sync>,
    run: Mutex<Run>,
    stop: AtomicBool,
    bytes: AtomicU64,
    active: AtomicI64,
    errors: AtomicI64,
    /// Os testes trocam os espelhos por um servidor local.
    pub mirrors: Mutex<Vec<String>>,
}

impl StressTest {
    pub fn new(log: Arc<Logger>, volume: Arc<dyn Fn() -> Option<f64> + Send + Sync>) -> Arc<StressTest> {
        Arc::new(StressTest {
            log,
            volume,
            run: Mutex::new(Run { state: "parado".into(), ..Default::default() }),
            stop: AtomicBool::new(false),
            bytes: AtomicU64::new(0),
            active: AtomicI64::new(0),
            errors: AtomicI64::new(0),
            mirrors: Mutex::new(MIRRORS.iter().map(|s| s.to_string()).collect()),
        })
    }

    pub fn state(&self) -> String {
        self.run.lock().unwrap().state.clone()
    }

    pub fn running(&self) -> bool {
        matches!(self.state().as_str(), "rodando" | "parando")
    }

    pub fn start(self: &Arc<Self>, streams: usize, max_gb: f64, max_seconds: f64, custom_url: &str) -> String {
        let streams = streams.max(1);
        {
            let mut r = self.run.lock().unwrap();
            if matches!(r.state.as_str(), "rodando" | "parando") {
                return "O teste já está rodando.".into();
            }
            self.stop.store(false, Ordering::SeqCst);
            self.bytes.store(0, Ordering::SeqCst);
            self.errors.store(0, Ordering::SeqCst);
            *r = Run {
                state: "rodando".into(),
                started: Some(now()),
                limit_bytes: (max_gb * 1e9) as u64,
                limit_seconds: max_seconds as u64,
                urls: if custom_url.trim().is_empty() {
                    self.mirrors.lock().unwrap().clone()
                } else {
                    vec![custom_url.trim().to_string()]
                },
                samples: vec![(now(), 0)],
                ..Default::default()
            };
        }
        let workers: Vec<_> = (0..streams)
            .map(|i| {
                let me = Arc::clone(self);
                std::thread::Builder::new().name(format!("stress-{i}")).spawn(move || me.worker(i)).unwrap()
            })
            .collect();
        let me = Arc::clone(self);
        std::thread::Builder::new().name("stress-watch".into()).spawn(move || me.watch(workers)).unwrap();
        self.log.log(format!("Teste de estresse: começando com {streams} conexões (para em {} GB ou {} min).",
                             crate::util::fmt_g(max_gb), (max_seconds / 60.0) as i64));
        "Teste de estresse iniciado — acompanhe o volume caindo e o refill entrando.".into()
    }

    pub fn stop(&self, reason: &str) -> String {
        let mut r = self.run.lock().unwrap();
        if r.state != "rodando" {
            return "O teste não está rodando.".into();
        }
        r.reason = reason.into();
        r.state = "parando".into();
        self.stop.store(true, Ordering::SeqCst);
        "Parando o teste…".into()
    }

    fn pick(&self, index: usize, attempt: usize) -> String {
        let r = self.run.lock().unwrap();
        let t = now();
        let live: Vec<&String> = r.urls.iter().filter(|u| r.bench.get(*u).copied().unwrap_or(0.0) <= t).collect();
        let pool: Vec<&String> = if live.is_empty() { r.urls.iter().collect() } else { live };
        pool[(index + attempt) % pool.len()].clone()
    }

    fn worker(&self, index: usize) {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(15))
            .user_agent(USER_AGENT)
            .build();
        let mut buf = vec![0u8; CHUNK];
        let mut attempt = 0;
        while !self.stop.load(Ordering::SeqCst) {
            let url = self.pick(index, attempt);
            let result = agent.get(&url).set("Cache-Control", "no-cache").call();
            let err = match result {
                Ok(resp) => {
                    self.active.fetch_add(1, Ordering::SeqCst);
                    let mut reader = resp.into_reader();
                    let mut err = None;
                    while !self.stop.load(Ordering::SeqCst) {
                        match reader.read(&mut buf) {
                            Ok(0) => break, // arquivo acabou: pega outro
                            Ok(n) => {
                                self.bytes.fetch_add(n as u64, Ordering::Relaxed);
                            }
                            Err(e) => {
                                err = Some(e.to_string());
                                break;
                            }
                        }
                    }
                    self.active.fetch_sub(1, Ordering::SeqCst);
                    err
                }
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = err {
                let n = self.errors.fetch_add(1, Ordering::SeqCst) + 1;
                self.run.lock().unwrap().bench.insert(url.clone(), now() + 60.0);
                attempt += 1;
                if matches!(n, 1 | 10 | 50) {
                    let host = url.split('/').nth(2).unwrap_or(&url);
                    let short: String = e.chars().take(60).collect();
                    self.log.log(format!("Teste de estresse: {host} falhou ({short}) — sigo pelos outros."));
                }
                for _ in 0..10 {
                    if self.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn watch(&self, workers: Vec<std::thread::JoinHandle<()>>) {
        loop {
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(100));
                if self.stop.load(Ordering::SeqCst) {
                    break;
                }
            }
            if self.stop.load(Ordering::SeqCst) {
                break;
            }
            let t = now();
            let total = self.bytes.load(Ordering::Relaxed);
            let vol = (self.volume)();
            let stop_reason = {
                let mut r = self.run.lock().unwrap();
                r.samples.push((t, total));
                r.samples.retain(|s| s.0 >= t - 5.0);
                let speed = speed_of(&r.samples);
                if t - r.started.unwrap_or(t) > 5.0 {
                    r.peak_bps = r.peak_bps.max(speed);
                }
                if let Some(v) = vol {
                    if r.min_volume.is_none_or(|m| v < m) {
                        r.min_volume = Some(v);
                    }
                }
                if r.limit_bytes > 0 && total >= r.limit_bytes {
                    Some(format!("chegou no limite de {} GB", crate::util::fmt_g(r.limit_bytes as f64 / 1e9)))
                } else if r.limit_seconds > 0 && t - r.started.unwrap_or(t) >= r.limit_seconds as f64 {
                    Some(format!("chegou no limite de {} min", r.limit_seconds / 60))
                } else if self.errors.load(Ordering::SeqCst) > 30 && total == 0 {
                    Some("nenhum servidor de teste respondeu".into())
                } else {
                    None
                }
            };
            if let Some(reason) = stop_reason {
                self.stop(&reason);
            }
        }
        for w in workers {
            let _ = w.join(); // cada um acorda em até ~15 s (timeout de leitura)
        }
        let summary = {
            let mut r = self.run.lock().unwrap();
            r.ended = Some(now());
            format!("Teste de estresse terminou ({}): {}",
                    if r.reason.is_empty() { "parado" } else { &r.reason }, self.summary(&r))
        };
        // Loga antes de virar "parado": quem vê o fim já acha o resumo no log.
        self.log.log(summary);
        self.run.lock().unwrap().state = "parado".into();
    }

    fn summary(&self, r: &Run) -> String {
        let secs = elapsed(r);
        let bytes = self.bytes.load(Ordering::Relaxed) as f64;
        let avg = if secs > 0.0 { bytes * 8.0 / secs / 1e6 } else { 0.0 };
        let mut txt = format!("{:.2} GB em {} min {} s (média {avg:.0} Mbit/s, pico {:.0} Mbit/s)",
                              bytes / 1e9, (secs / 60.0) as i64, (secs % 60.0) as i64, r.peak_bps / 1e6);
        if let Some(m) = r.min_volume {
            txt += &format!(", volume mínimo {m:.2} GB");
        }
        txt + "."
    }

    pub fn info(&self) -> StressInfo {
        let r = self.run.lock().unwrap();
        let secs = elapsed(&r);
        let bytes = self.bytes.load(Ordering::Relaxed);
        let round1 = |x: f64| (x * 10.0).round() / 10.0;
        StressInfo {
            estado: r.state.clone(),
            inicio: r.started,
            fim: r.ended,
            segundos: round1(secs),
            bytes,
            limite_bytes: r.limit_bytes,
            limite_segundos: r.limit_seconds,
            mbps: if r.state == "rodando" { round1(speed_of(&r.samples) / 1e6) } else { 0.0 },
            media_mbps: if secs > 0.0 { round1(bytes as f64 * 8.0 / secs / 1e6) } else { 0.0 },
            pico_mbps: round1(r.peak_bps / 1e6),
            conexoes: self.active.load(Ordering::SeqCst),
            erros: self.errors.load(Ordering::SeqCst),
            volume_minimo: r.min_volume,
            motivo: r.reason.clone(),
            ..Default::default()
        }
    }
}

fn speed_of(s: &[(f64, u64)]) -> f64 {
    if s.len() < 2 || s[s.len() - 1].0 - s[0].0 <= 0.0 {
        return 0.0;
    }
    (s[s.len() - 1].1.saturating_sub(s[0].1)) as f64 * 8.0 / (s[s.len() - 1].0 - s[0].0)
}

fn elapsed(r: &Run) -> f64 {
    match r.started {
        Some(s) => r.ended.unwrap_or_else(now) - s,
        None => 0.0,
    }
}
