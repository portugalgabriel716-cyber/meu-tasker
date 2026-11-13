//! Testes do Tasker — 100% offline, contra a API de mentira (common/mod.rs).
//!
//!     cargo test
//!
//! Nada aqui encosta nos seus arquivos de verdade: cada teste usa uma pasta de
//! dados temporária e um ambiente próprio (HOME/XDG apontam pra pastas
//! temporárias), e a sentinela no fim confere que dados/ não mudou.
mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::{now, FakeLidl, TempDir};
use serde_json::{json, Value};
use tasker::config::{self, Cfg};
use tasker::daemon::App;
use tasker::engine::Monitor;
use tasker::history::{self, clear_pause, pause_state, record_event, set_pause};
use tasker::lidl::{jwt_exp, parse_painel, LidlApi};
use tasker::logs::Logger;
use tasker::notify::Recorder;
use tasker::paths::Paths;
use tasker::store::{self, Json, StateStore};
use tasker::util::Env;

struct T {
    dir: TempDir,
    paths: Arc<Paths>,
    env: Env,
    fake: FakeLidl,
    rec: Arc<Recorder>,
}

fn obj(v: Value) -> Json {
    v.as_object().cloned().unwrap()
}

impl T {
    fn new(tag: &str) -> T {
        T::with_env(tag, &[])
    }

    fn with_env(tag: &str, extra: &[(&str, &str)]) -> T {
        let dir = TempDir::new(tag);
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("HOME".into(), dir.0.join("home").to_string_lossy().into());
        env.insert("XDG_CONFIG_HOME".into(), dir.0.join("xdg-config").to_string_lossy().into());
        env.insert("XDG_DATA_HOME".into(), dir.0.join("xdg-data").to_string_lossy().into());
        env.insert("TASKER_BROWSER".into(), "/nao/existe".into());
        for (k, v) in extra {
            env.insert(k.to_string(), v.to_string());
        }
        let env: Env = Arc::new(env);
        let paths = Arc::new(Paths::with_data_dir(dir.0.join("dados"), &env));
        paths.ensure_data_dir();
        T { dir, paths, env, fake: FakeLidl::start(), rec: Arc::new(Recorder::default()) }
    }

    fn api(&self) -> LidlApi {
        LidlApi::new(Some(&self.fake.origin), 5)
    }

    fn seed_tokens(&self) -> (String, String) {
        let (a, r) = self.fake.lock().issue(None);
        StateStore::new(&self.paths)
            .update(&obj(json!({"access": a, "refresh": r, "expires_at": jwt_exp(&a)})))
            .unwrap();
        (a, r)
    }

    fn monitor(&self, tokens: bool, f: impl FnOnce(&mut Cfg)) -> Arc<Monitor> {
        if tokens {
            self.seed_tokens();
        }
        let m = Monitor::new(self.paths.clone(), self.env.clone(), Arc::new(Logger::silent()), Some(self.api()));
        m.set_notifier(self.rec.clone());
        m.update_cfg(f);
        m.load_state();
        m
    }

    fn disk(&self) -> Json {
        StateStore::new(&self.paths).load()
    }

    fn msgs(&self) -> Vec<String> {
        self.rec.msgs.lock().unwrap().clone()
    }

    fn books(&self) -> usize {
        self.fake.lock().count("bookTariffOptionsDirect")
    }
}

fn st_f64(m: &Monitor, k: &str) -> f64 {
    store::get_f64(&m.lock().st, k).unwrap_or(0.0)
}

fn set_st(m: &Monitor, k: &str, v: Value) {
    m.lock().st.insert(k.into(), v);
}

fn age_pending(m: &Monitor, secs: f64) {
    let mut g = m.lock();
    let p = g.st.get_mut("pending_refill").and_then(Value::as_object_mut).expect("sem pendência");
    let at = p["at"].as_f64().unwrap();
    p.insert("at".into(), json!(at - secs));
}

// ======================================================================= config

#[test]
fn config_defaults_and_precedence() {
    let t = T::new("cfg");
    assert_eq!(config::load(&t.paths, &t.env).check_interval, 60.0);
    store::write_json(&t.paths.config_file, &json!({"check_interval": 40, "gb_threshold": "0,4",
                                                    "headless": true, "keepalive_interval": 600}), false).unwrap();
    let (vals, src) = config::load_with_sources(&t.paths, &t.env);
    assert_eq!(vals["check_interval"], config::Val::Int(40));
    assert_eq!(src["check_interval"], "arquivo");
    assert_eq!(vals["gb_threshold"], config::Val::Float(0.4));
    assert!(!vals.contains_key("headless")); // ajuste da v1, morto
    let env: Env = Arc::new([("TASKER_CHECK_INTERVAL".to_string(), "120".to_string())].into());
    let (vals, src) = config::load_with_sources(&t.paths, &env);
    assert_eq!((vals["check_interval"].clone(), src["check_interval"]), (config::Val::Int(120), "ambiente"));
}

#[test]
fn config_invalid_values_fall_back() {
    let t = T::new("cfg2");
    store::write_json(&t.paths.config_file, &json!({"check_interval": 1, "gb_threshold": "abc"}), false).unwrap();
    let c = config::load(&t.paths, &t.env);
    assert_eq!(c.check_interval, 60.0);
    assert_eq!(c.gb_threshold, 0.85);
    std::fs::write(&t.paths.config_file, "{quebrado").unwrap();
    assert_eq!(config::load(&t.paths, &t.env).check_interval, 60.0);
}

#[test]
fn config_display_units() {
    let spec = config::spec("max_without_refill").unwrap();
    assert_eq!(config::to_display(spec, &config::Val::Int(840)), json!(14));
    assert_eq!(config::from_display(spec, &json!("14")), Ok(config::Val::Int(840)));
    assert!(config::from_display(spec, &json!("1")).is_err());
    assert_eq!(config::from_display(spec, &json!("13,5")), Ok(config::Val::Int(810)));
    let topic = config::spec("ntfy_topic").unwrap();
    assert_eq!(config::from_display(topic, &json!("https://n.tld/x")), Ok(config::Val::Str("https://n.tld/x".into())));
    assert!(config::from_display(topic, &json!("com espaço")).is_err());
    let gb = config::spec("gb_threshold").unwrap();
    assert_eq!(config::from_display(gb, &json!("9")).unwrap_err(), "máximo é 5 GB");
}

#[test]
fn config_save_drops_obsolete_keys() {
    let t = T::new("cfg3");
    store::write_json(&t.paths.config_file, &json!({"headless": false, "check_interval": 40}), false).unwrap();
    let app = App::new(t.paths.clone(), t.env.clone(), Arc::new(Logger::silent()), Some(t.api()));
    let r = app.save_settings(&obj(json!({"gb_threshold": "0,5"})));
    assert_eq!(r.ok, Some(true), "{r:?}");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&t.paths.config_file).unwrap()).unwrap();
    assert_eq!(saved, json!({"check_interval": 40, "gb_threshold": 0.5}));
    let r = app.save_settings(&obj(json!({"gb_threshold": "9"})));
    assert_eq!(r.ok, Some(false));
    assert!(r.erros.unwrap().contains_key("gb_threshold"));
}

// ===================================================================== migração

#[test]
fn migration_moves_v1_files_without_overwriting() {
    let t = T::new("mig");
    let old = t.dir.0.join("raiz-v1");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("session_tokens.json"), r#"{"refresh": "rt-velho"}"#).unwrap();
    std::fs::write(old.join("config.json"), r#"{"check_interval": 40}"#).unwrap();
    std::fs::write(old.join("refill_history.jsonl"), "").unwrap();
    std::fs::write(old.join("tasker.log"), "x\n").unwrap();
    store::write_json(&t.paths.config_file, &json!({"check_interval": 99}), false).unwrap(); // já existe: fica
    let moved = t.paths.migrate_legacy(Some(&old));
    assert!(moved.contains(&"session_tokens.json".to_string()));
    assert!(!moved.contains(&"config.json".to_string()));
    assert_eq!(t.disk()["refresh"], "rt-velho");
    assert!(old.join("config.json").exists());
    assert!(t.paths.migrate_legacy(Some(&old)).is_empty());
    assert!(t.paths.migrate_legacy(None).is_empty()); // pasta trocada: nada a migrar
}

#[test]
fn v2_python_files_are_read_as_is() {
    // Arquivos exatamente como a versão 2 (Python) grava: indent=1, sem ASCII escapado.
    let t = T::new("v2");
    std::fs::write(&t.paths.config_file, "{\n \"check_interval\": 40,\n \"gb_threshold\": 0.4,\n \"stress_url\": \"\"\n}").unwrap();
    let c = config::load(&t.paths, &t.env);
    assert_eq!((c.check_interval, c.gb_threshold), (40.0, 0.4));
    std::fs::write(&t.paths.refills_file,
                   "{\"before\": 0.42, \"after\": null, \"why\": \"API confirmou\", \"latency\": 1.9, \"t\": 1, \"kind\": \"refill_ok\"}\n").unwrap();
    assert_eq!(history::refills_since(&t.paths, 0.0), (1, 0));
}

// ========================================================================== API

#[test]
fn api_parse_painel() {
    let (u, cd) = parse_painel(Some(&json!({"consumptions": {"consumptionsForUnit": [
        {"type": "REFILLABLE_DATA", "left": 0.3, "max": 1}]},
        "tariffoptions": {"disabledTariffoptions": {"disabledTariffoptions": [{"tariffoptionId": "CCS_92061"}]}}})));
    assert_eq!(u.unwrap().refill.unwrap()["left"], 0.3);
    assert_eq!(cd, Some(true));
    let (u, cd) = parse_painel(Some(&json!({"consumptions": {"consumptionsForUnit": []}})));
    let u = u.unwrap();
    assert!(u.refill.is_none() && u.data.is_none());
    assert_eq!(cd, None);
    let (u, cd) = parse_painel(None);
    assert!(u.is_none() && cd.is_none());
}

#[test]
fn api_jwt_exp() {
    assert_eq!(jwt_exp(&common::make_jwt(123.0)), 123.0);
    assert_eq!(jwt_exp("lixo"), 0.0);
}

#[test]
fn api_x_transaction_hash_matches_site() {
    // Mesmo valor do hash em JavaScript do site (e da versão em Python).
    assert_eq!(tasker::lidl::js_hash("abc"), 2147580001);
    assert_eq!(tasker::lidl::js_hash(&"x".repeat(20)), 2992887295);
    assert_eq!(tasker::lidl::js_hash("eyJhbGciOi.payload.sig"), 3719333657);
}

#[test]
fn keepalive_reuses_connection() {
    let t = T::new("ka");
    let m = t.monitor(true, |_| {});
    for _ in 0..4 {
        m.cycle();
    }
    assert_eq!(t.fake.lock().count("painel"), 4);
    assert_eq!(t.fake.lock().connections, 1, "devia reaproveitar a conexão");
}

#[test]
fn reconnects_when_server_drops() {
    let t = T::new("drop");
    let m = t.monitor(true, |_| {});
    m.cycle();
    t.fake.lock().drop_next = true; // o servidor fecha a keep-alive enquanto dormíamos
    m.cycle();
    assert_eq!(t.fake.lock().count("painel"), 2);
    assert!(m.status().erro.is_none(), "{:?}", m.status().erro);
}

// ======================================================================= refill

#[test]
fn reads_volume_in_one_request() {
    let t = T::new("one");
    t.fake.lock().left = 0.9;
    let m = t.monitor(true, |c| c.gb_threshold = 0.4);
    m.cycle();
    assert_eq!(t.fake.lock().ops(), vec!["painel"]);
    assert_eq!(m.status().volume.unwrap().left, 0.9);
    assert_eq!(m.status().cooldown, Some(false));
    assert!(t.paths.volume_file.exists());
}

#[test]
fn low_volume_books_once_and_confirms_fast() {
    let t = T::new("low");
    {
        let mut f = t.fake.lock();
        f.left = 0.3;
        f.rise_after_reads = 2;
    }
    let m = t.monitor(true, |c| {
        c.gb_threshold = 0.4;
        c.fast_interval = 7.0;
    });
    let (delay, why) = m.cycle();
    assert_eq!(t.books(), 1);
    assert_eq!((delay, why.as_str()), (7.0, "confirmando o refill"));
    assert_eq!(t.disk()["pending_refill"]["before"], 0.3); // persistida ANTES de confirmar
    let (delay, _) = m.cycle(); // ainda não subiu
    assert_eq!(delay, 7.0);
    assert_eq!(t.books(), 1);
    m.cycle(); // subiu
    assert!(t.disk()["pending_refill"].is_null());
    let text = std::fs::read_to_string(&t.paths.refills_file).unwrap();
    let ev: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(ev["kind"], "refill_ok");
    assert_eq!(ev["why"], "volume subiu");
    assert!(ev["latency"].is_number());
    assert_eq!(t.books(), 1);
}

#[test]
fn pending_survives_restart() {
    let t = T::new("pend");
    {
        let mut f = t.fake.lock();
        f.left = 0.3;
        f.rise_after_reads = 99;
    }
    let m = t.monitor(true, |c| c.gb_threshold = 0.4);
    m.cycle();
    let m2 = t.monitor(false, |c| c.gb_threshold = 0.4);
    assert!(!m2.lock().st["pending_refill"].is_null());
    m2.cycle();
    assert_eq!(t.books(), 1, "reservou duas vezes");
}

#[test]
fn no_effect_counts_failure_and_backs_off() {
    let t = T::new("noop");
    {
        let mut f = t.fake.lock();
        f.left = 0.3;
        f.refill_mode = "noop".into();
    }
    let m = t.monitor(true, |c| {
        c.gb_threshold = 0.4;
        c.pending_grace = 30.0;
        c.check_interval = 60.0;
    });
    m.cycle();
    age_pending(&m, 31.0); // passou o prazo
    m.cycle();
    let d = t.disk();
    assert_eq!(d["refill_fails"], 1);
    assert!(d["next_refill_try"].as_f64().unwrap() > now() + 100.0);
    m.cycle(); // em espera: não reserva de novo
    assert_eq!(t.books(), 1);
}

#[test]
fn fourth_failure_notifies() {
    let t = T::new("4x");
    {
        let mut f = t.fake.lock();
        f.left = 0.3;
        f.refill_mode = "noop".into();
    }
    let m = t.monitor(true, |c| {
        c.gb_threshold = 0.4;
        c.pending_grace = 30.0;
    });
    for _ in 0..4 {
        set_st(&m, "next_refill_try", json!(0));
        m.cycle();
        age_pending(&m, 31.0);
        m.cycle();
    }
    assert_eq!(t.disk()["refill_fails"], 4);
    assert!(t.msgs().iter().any(|x| x.contains("4 vezes")));
}

#[test]
fn unknown_cooldown_never_books() {
    let t = T::new("cdx");
    {
        let mut f = t.fake.lock();
        f.left = 0.2;
        f.omit_tariffs = true;
    }
    let m = t.monitor(true, |c| {
        c.gb_threshold = 0.4;
        c.fast_interval = 9.0;
    });
    let (delay, why) = m.cycle();
    assert_eq!(t.books(), 0);
    assert_eq!((delay, why.as_str()), (9.0, "cooldown ilegível"));
}

#[test]
fn cooldown_waits() {
    let t = T::new("cd");
    {
        let mut f = t.fake.lock();
        f.left = 0.2;
        f.cooldown = true;
    }
    let m = t.monitor(true, |c| c.gb_threshold = 0.4);
    m.cycle();
    assert_eq!(t.books(), 0);
}

#[test]
fn cooldown_after_booking_confirms() {
    let t = T::new("cd2");
    {
        let mut f = t.fake.lock();
        f.left = 0.3;
        f.rise_after_reads = 99;
    }
    let m = t.monitor(true, |c| c.gb_threshold = 0.4);
    m.cycle();
    t.fake.lock().cooldown = true;
    m.cycle();
    assert!(t.disk()["pending_refill"].is_null());
}

#[test]
fn full_bucket_forced_only_resets_clock() {
    let t = T::new("full");
    let m = t.monitor(true, |c| c.max_without_refill = 300.0);
    set_st(&m, "last_refill", json!(now() - 400.0));
    m.cycle();
    assert_eq!(t.books(), 0);
    assert!(t.disk()["last_refill"].as_f64().unwrap() > now() - 5.0);
}

#[test]
fn forced_refill_when_not_full() {
    let t = T::new("forced");
    t.fake.lock().left = 0.95;
    let m = t.monitor(true, |c| {
        c.max_without_refill = 300.0;
        c.gb_threshold = 0.4;
    });
    set_st(&m, "last_refill", json!(now() - 400.0));
    m.cycle();
    assert_eq!(t.books(), 1);
}

#[test]
fn api_success_is_trusted() {
    let t = T::new("ok");
    {
        let mut f = t.fake.lock();
        f.left = 0.1;
        f.refill_mode = "success".into();
    }
    let m = t.monitor(true, |c| c.gb_threshold = 0.4);
    m.cycle();
    assert!(t.disk()["pending_refill"].is_null());
    assert_eq!(m.status().ultimo_refill.unwrap().why, "API confirmou");
}

#[test]
fn dropped_mutation_is_not_repeated() {
    let t = T::new("mdrop");
    {
        let mut f = t.fake.lock();
        f.left = 0.1;
        f.refill_mode = "drop".into();
    }
    let m = t.monitor(true, |c| c.gb_threshold = 0.4);
    m.cycle();
    assert_eq!(t.books(), 1);
    assert!(!t.disk()["pending_refill"].is_null());
    m.cycle();
    assert_eq!(t.books(), 1);
}

#[test]
fn transient_500_on_read_is_retried() {
    let t = T::new("500");
    t.fake.lock().fail_reads = vec![500];
    let m = t.monitor(true, |_| {});
    m.cycle();
    assert!(m.status().erro.is_none());
    assert_eq!(t.fake.lock().count("painel"), 2);
}

#[test]
fn persistent_read_errors_back_off() {
    let t = T::new("500x");
    t.fake.lock().fail_reads = vec![500; 10];
    let m = t.monitor(true, |c| c.check_interval = 40.0);
    let (d1, _) = m.cycle();
    let (d2, _) = m.cycle();
    assert_eq!((d1, d2), (40.0, 80.0));
    assert!(m.status().erro.is_some());
}

#[test]
fn combined_rejected_falls_back() {
    let t = T::new("comb");
    {
        let mut f = t.fake.lock();
        f.reject_combined = true;
        f.left = 0.9;
    }
    let m = t.monitor(true, |_| {});
    m.cycle();
    assert!(!m.lock().combined);
    assert_eq!(m.status().volume.unwrap().left, 0.9);
    assert_eq!(m.status().cooldown, Some(false));
}

#[test]
fn pause_is_silent() {
    let t = T::new("pause");
    let m = t.monitor(true, |_| {});
    set_pause(&t.paths, 30.0);
    m.start();
    std::thread::sleep(Duration::from_millis(800));
    assert!(t.fake.lock().calls.is_empty());
    assert_eq!(m.snapshot().fase, "pausado");
    clear_pause(&t.paths);
    m.poke();
    for _ in 0..40 {
        if !t.fake.lock().calls.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    m.stop(Duration::from_secs(10));
    assert!(!t.fake.lock().calls.is_empty());
    assert!(!pause_state(&t.paths).0);
    assert!(!m.running());
}

#[test]
fn expired_pause_is_removed() {
    let t = T::new("pause2");
    store::write_json(&t.paths.pause_file, &json!({"until": now() - 1.0}), false).unwrap();
    assert_eq!(pause_state(&t.paths), (false, 0.0));
    assert!(!t.paths.pause_file.exists());
}

// ======================================================================= tokens

#[test]
fn refresh_rotates_and_persists_first() {
    let t = T::new("rot");
    let (a, r) = t.fake.lock().issue(Some(60.0)); // vence dentro da margem de 5 min
    StateStore::new(&t.paths).update(&obj(json!({"access": a, "refresh": r, "expires_at": jwt_exp(&a)}))).unwrap();
    let m = t.monitor(false, |_| {});
    m.cycle();
    let d = t.disk();
    let new_rt = d["refresh"].as_str().unwrap();
    assert_ne!(new_rt, r);
    assert!(t.fake.lock().refresh.contains(new_rt));
    assert!(!t.fake.lock().refresh.contains(&r));
    assert!(d["expires_at"].as_f64().unwrap() > now() + 3000.0);
}

#[test]
fn http_401_refreshes_and_retries() {
    let t = T::new("401");
    let m = t.monitor(true, |_| {});
    t.fake.lock().access.clear(); // revogado no servidor
    m.cycle();
    assert!(m.status().erro.is_none());
    assert_eq!(t.fake.lock().count("refresh"), 1);
}

#[test]
fn dead_session_without_credentials_asks_login_once() {
    let t = T::new("dead");
    let m = t.monitor(true, |_| {});
    {
        let mut f = t.fake.lock();
        f.access.clear();
        f.refresh.clear();
    }
    m.cycle();
    assert!(m.status().precisa_login.is_some());
    m.cycle();
    m.cycle();
    assert_eq!(t.msgs().iter().filter(|x| x.to_lowercase().contains("login")).count(), 1);
    assert_eq!(t.fake.lock().count("refresh"), 1, "marretou o refresh morto");
}

#[test]
fn dead_session_relogs_with_password() {
    let t = T::new("relog");
    let (u, p) = t.fake.lock().password.clone();
    store::save_credentials(&t.paths, &u, &p).unwrap();
    let m = t.monitor(true, |_| {});
    {
        let mut f = t.fake.lock();
        f.access.clear();
        f.refresh.clear();
    }
    m.cycle();
    assert!(m.status().precisa_login.is_none());
    assert_eq!(t.fake.lock().count("password"), 1);
    assert_eq!(t.fake.lock().count("painel"), 2);
}

#[test]
fn seed_from_env() {
    let mut t = T::new("seed");
    let (_, r) = t.fake.lock().issue(None);
    let mut env: HashMap<String, String> = (*t.env).clone();
    env.insert("TASKER_REFRESH_TOKEN".into(), r);
    t.env = Arc::new(env);
    let m = t.monitor(false, |_| {});
    m.cycle();
    assert!(m.status().erro.is_none());
    let rt = t.disk()["refresh"].as_str().unwrap().to_string();
    assert!(t.fake.lock().refresh.contains(&rt));
}

#[test]
fn v1_state_file_is_compatible() {
    let t = T::new("v1");
    let (a, r) = t.fake.lock().issue(None);
    store::write_json(&t.paths.state_file, &json!({
        "access": a, "refresh": r, "expires_at": jwt_exp(&a), "saved_at": 1,
        "pending_refill": null, "last_refill": now() - 60.0, "refill_fails": 0, "next_refill_try": 0}), true).unwrap();
    let m = t.monitor(false, |_| {});
    m.cycle();
    assert!(m.status().erro.is_none());
    assert_eq!(t.fake.lock().count("refresh"), 0);
}

#[test]
fn disk_adoption() {
    let t = T::new("adopt");
    let m = t.monitor(true, |_| {});
    let (a, r) = t.fake.lock().issue(None);
    StateStore::new(&t.paths).update(&obj(json!({"access": a, "refresh": r, "expires_at": jwt_exp(&a) + 100.0}))).unwrap();
    set_st(&m, "expires_at", json!(now() + 10.0)); // o meu está vencendo
    m.cycle();
    assert_eq!(m.lock().st["access"], a);
    assert_eq!(t.fake.lock().count("refresh"), 0);
}

#[test]
fn private_files_are_owner_only() {
    let t = T::new("perm");
    t.seed_tokens();
    store::save_credentials(&t.paths, "1", "2").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for f in [&t.paths.state_file, &t.paths.credentials_file] {
            assert_eq!(std::fs::metadata(f).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}

// ================================================================ login por senha

#[test]
fn wrong_password_backs_off() {
    let t = T::new("pw");
    let m = t.monitor(true, |_| {});
    let creds = |p: &str| Some(("01761234567".to_string(), p.to_string()));
    let (ok, msg) = m.password_login(true, creds("errada"));
    assert!(!ok);
    assert!(msg.contains("falsch"), "{msg}");
    let d = t.disk();
    assert_eq!(d["login_fails"], 1);
    assert!(d["login_blocked_until"].as_f64().unwrap() > now() + 200.0);
    let (ok, _) = m.password_login(true, creds("certa"));
    assert!(!ok); // ainda na espera
    assert_eq!(t.fake.lock().count("password"), 1);
    let (ok, _) = m.password_login(false, creds("certa"));
    assert!(ok); // manual não respeita a espera
    assert_eq!(t.disk()["login_fails"], 0);
}

#[test]
fn max_fails_blocks_long_and_notifies() {
    let t = T::new("pwmax");
    let m = t.monitor(true, |c| c.login_max_fails = 2);
    for _ in 0..2 {
        set_st(&m, "login_blocked_until", json!(0));
        StateStore::new(&t.paths).update(&obj(json!({"login_blocked_until": 0}))).unwrap();
        m.password_login(true, Some(("x".into(), "y".into())));
    }
    assert!(t.disk()["login_blocked_until"].as_f64().unwrap() > now() + 5.0 * 3600.0);
    assert!(t.msgs().iter().any(|x| x.contains("2 vezes")));
}

#[test]
fn captcha_stops_automatic_login() {
    let t = T::new("captcha");
    t.fake.lock().captcha = true;
    let m = t.monitor(true, |_| {});
    let (ok, msg) = m.password_login(true, Some(("01761234567".into(), "certa".into())));
    assert!(!ok);
    assert!(msg.to_lowercase().contains("captcha"));
    assert!(t.disk()["login_blocked_until"].as_f64().unwrap() > now() + 5.0 * 3600.0);
}

#[test]
fn server_error_does_not_count() {
    let t = T::new("pw500");
    t.seed_tokens();
    let m = Monitor::new(t.paths.clone(), t.env.clone(), Arc::new(Logger::silent()),
                         Some(LidlApi::new(Some("http://127.0.0.1:9"), 2))); // porta fechada
    m.load_state();
    let (ok, _) = m.password_login(true, Some(("x".into(), "y".into())));
    assert!(!ok);
    assert_eq!(store::get_f64(&t.disk(), "login_fails").unwrap_or(0.0), 0.0);
}

#[test]
fn set_credentials_reset_is_seen() {
    let t = T::new("pwreset");
    let m = t.monitor(true, |_| {});
    m.password_login(true, Some(("01761234567".into(), "errada".into())));
    StateStore::new(&t.paths).update(&obj(json!({"login_fails": 0, "login_blocked_until": 0}))).unwrap(); // --set-credentials
    let (ok, _) = m.password_login(true, Some(("01761234567".into(), "certa".into())));
    assert!(ok);
}

// =================================================================== adaptativo

fn samples(m: &Monitor, pts: &[(f64, f64)]) {
    let mut g = m.lock();
    g.samples.clear();
    g.samples.extend(pts.iter().copied());
}

#[test]
fn fast_consumption_shortens_interval() {
    let t = T::new("ad");
    let m = t.monitor(true, |c| {
        c.check_interval = 60.0;
        c.fast_interval = 15.0;
        c.gb_threshold = 0.4;
        c.max_without_refill = 3600.0;
    });
    let n = now();
    samples(&m, &[(n - 120.0, 0.9), (n - 60.0, 0.8), (n, 0.7)]); // 0.1 GB/min
    // cruza 0.4 em ~180 s -> olha em ~90 s, limitado aos 60 normais
    assert_eq!(m.next_delay_for(Some(0.7)).0, 60.0);
    samples(&m, &[(n - 120.0, 1.0), (n - 60.0, 0.75), (n, 0.5)]); // 0.25 GB/min
    let (delay, why) = m.next_delay_for(Some(0.5));
    assert!((delay - 15.0).abs() < 0.5, "{delay}"); // ~12 s -> piso de 15
    assert!(why.contains("adaptativo"));
}

#[test]
fn idle_keeps_normal_interval() {
    let t = T::new("idle");
    let m = t.monitor(true, |c| {
        c.check_interval = 60.0;
        c.max_without_refill = 3600.0;
    });
    let n = now();
    samples(&m, &[(n - 300.0, 0.9), (n, 0.9)]);
    assert_eq!(m.next_delay_for(Some(0.9)).0, 60.0);
}

#[test]
fn adaptive_disabled() {
    let t = T::new("adoff");
    let m = t.monitor(true, |c| {
        c.check_interval = 60.0;
        c.adaptive = false;
        c.max_without_refill = 3600.0;
    });
    let n = now();
    samples(&m, &[(n - 120.0, 1.0), (n, 0.5)]);
    assert_eq!(m.next_delay_for(Some(0.5)).0, 60.0);
}

#[test]
fn wakes_for_forced_refill() {
    let t = T::new("wake");
    let m = t.monitor(true, |c| {
        c.check_interval = 60.0;
        c.max_without_refill = 300.0;
    });
    set_st(&m, "last_refill", json!(now() - 280.0));
    let (delay, why) = m.next_delay_for(Some(0.9));
    assert!(delay < 25.0);
    assert_eq!(why, "hora do refill forçado");
    assert!(st_f64(&m, "last_refill") > 0.0);
}

// ========================================================================== log

#[test]
fn log_since_and_reset() {
    let log = Logger::silent();
    log.log("a");
    log.log("b");
    let r = log.since(0);
    assert_eq!(r.linhas.len(), 2);
    log.log("c");
    let r2 = log.since(r.seq);
    assert_eq!(r2.linhas.len(), 1);
    assert!(!r2.reset);
    assert!(log.since(9999).reset);
}

#[test]
fn log_once() {
    let log = Logger::silent();
    log.once("k", "x", 300.0);
    log.once("k", "x", 300.0);
    log.once("k", "y", 300.0);
    assert_eq!(log.since(0).linhas.len(), 2);
}

#[test]
fn log_preload_and_file() {
    let t = T::new("log");
    let log = Logger::new(Some(t.paths.log_file.clone()), false, false);
    for i in 0..5 {
        log.log(format!("linha {i}"));
    }
    let again = Logger::new(Some(t.paths.log_file.clone()), false, false);
    again.preload(3);
    let lines = again.since(0).linhas;
    assert_eq!(lines.len(), 3);
    assert!(lines[2].ends_with("linha 4"));
}

// ================================================================ serviço local

fn service(t: &T) -> (Arc<App>, tasker::client::Client, std::thread::JoinHandle<()>) {
    let app = App::new(t.paths.clone(), t.env.clone(), Arc::new(Logger::silent()), Some(t.api()));
    app.monitor.set_notifier(t.rec.clone());
    let a = app.clone();
    let h = std::thread::spawn(move || a.run_service(0).unwrap());
    for _ in 0..100 {
        if let Some(c) = tasker::client::Client::find(&t.paths) {
            if c.alive() {
                return (app, c, h);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("o serviço não subiu");
}

fn raw_get(port: u16, path: &str, key: Option<&str>, host: Option<&str>) -> u16 {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build();
    let mut req = agent.get(&format!("http://127.0.0.1:{port}{path}"));
    if let Some(k) = key {
        req = req.set("X-Tasker-Key", k);
    }
    if let Some(h) = host {
        req = req.set("Host", h);
    }
    match req.call() {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(c, _)) => c,
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn service_auth_state_settings_and_actions() {
    let t = T::new("svc");
    t.seed_tokens();
    let (app, c, h) = service(&t);
    let port = app.port();
    assert_eq!(raw_get(port, "/api/estado", None, None), 403);
    assert_eq!(raw_get(port, "/api/estado", Some("errada"), None), 403);
    assert_eq!(raw_get(port, "/api/estado", Some(&app.key), Some("evil.example")), 403);
    assert_eq!(raw_get(port, "/api/estado", Some(&app.key), None), 200);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&t.paths.ui_file).unwrap().permissions().mode() & 0o777, 0o600);
    }

    let st: tasker::types::AppState = c.get("/api/estado").unwrap();
    assert!(st.monitor.ligado);
    let s: tasker::types::SettingsPayload = c.get("/api/ajustes").unwrap();
    assert_eq!(s.campos.len(), config::SETTINGS.len());
    let r: tasker::types::ActionResult = c.post("/api/ajustes", &json!({"valores": {"check_interval": "5"}})).unwrap();
    assert_eq!(r.ok, Some(false));
    let r: tasker::types::ActionResult = c.post("/api/ajustes", &json!({"valores": {"check_interval": "45"}})).unwrap();
    assert_eq!(r.ok, Some(true));
    assert_eq!(app.monitor.cfg().check_interval, 45.0); // vale na hora
    let _: Value = c.post("/api/acao", &json!({"acao": "pausar", "minutos": 30})).unwrap();
    assert!(pause_state(&t.paths).0);
    let _: Value = c.post("/api/acao", &json!({"acao": "retomar"})).unwrap();
    assert!(!pause_state(&t.paths).0);
    let r: Value = c.post("/api/acao", &json!({"acao": "inexistente"})).unwrap();
    assert!(r.get("erro").is_some());
    let _: Value = c.post("/api/acao", &json!({"acao": "focar"})).unwrap();
    let st: tasker::types::AppState = c.get("/api/estado").unwrap();
    assert_eq!(st.sistema.foco, 1);

    let r: tasker::types::ActionResult = c.post("/api/acao", &json!({"acao": "login_senha", "msisdn": "01761234567",
                                                                       "senha": "certa", "lembrar": true})).unwrap();
    assert_eq!(r.ok, Some(true), "{r:?}");
    assert!(t.paths.credentials_file.exists());
    let r: tasker::types::ActionResult = c.post("/api/acao", &json!({"acao": "login_senha", "msisdn": "x", "senha": "y"})).unwrap();
    assert_eq!(r.ok, Some(false));

    let log: tasker::logs::LogChunk = c.get("/api/log?desde=0").unwrap();
    assert!(!log.linhas.is_empty());
    let _: Value = c.post("/api/acao", &json!({"acao": "sair"})).unwrap();
    h.join().unwrap();
    assert!(!t.paths.ui_file.exists());
    assert!(!app.monitor.running());
}

#[test]
fn history_thins_and_summarizes() {
    let t = T::new("hist");
    let n = now();
    let mut text = String::new();
    for i in 0..3000 {
        text += &format!("{}\n", json!({"t": n - 3000.0 + i as f64, "gb": 0.5 + (i % 10) as f64 / 100.0}));
    }
    std::fs::write(&t.paths.volume_file, text).unwrap();
    record_event(&t.paths, "refill_ok", obj(json!({"before": 0.4, "after": 1.0, "why": "volume subiu", "latency": 12})));
    let h = history::history(&t.paths, 24);
    assert!(h.volume.len() <= 1510, "{}", h.volume.len());
    assert_eq!(h.resumo.refills_hoje, 1);
    assert_eq!(h.resumo.latencia_media, Some(12.0));
}

// ============================================================ linha de comando

fn cli(t: &T, args: &[&str]) -> std::process::Output {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_tasker"));
    c.args(args);
    for (k, _) in std::env::vars() {
        if k.starts_with("TASKER_") {
            c.env_remove(k);
        }
    }
    for (k, v) in t.env.iter() {
        c.env(k, v);
    }
    c.env("TASKER_DATA_DIR", &t.paths.data_dir)
        .env("TASKER_API_ORIGIN", &t.fake.origin)
        .env("TASKER_NOTIFY", "0");
    c.output().unwrap()
}

fn out(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr)
}

#[test]
fn cli_once_and_status() {
    let t = T::new("cli");
    t.seed_tokens();
    t.fake.lock().left = 0.9;
    let o = cli(&t, &["--once"]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("Volume de refill: 0.90"), "{}", out(&o));
    assert!(t.paths.status_file.exists());
    let o = cli(&t, &["--status"]);
    assert!(out(&o).contains("refill: 0.9/1.0"), "{}", out(&o));
}

#[test]
fn cli_running_instance_handoff_and_clean_stop() {
    let t = T::new("cli2");
    t.seed_tokens();
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_tasker"));
    for (k, v) in t.env.iter() {
        c.env(k, v);
    }
    let mut proc = c
        .args(["--fundo", "--porta", "0"])
        .env("TASKER_DATA_DIR", &t.paths.data_dir)
        .env("TASKER_API_ORIGIN", &t.fake.origin)
        .env("TASKER_NOTIFY", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..100 {
        if t.paths.ui_file.exists() && t.paths.status_file.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(store::instance_running(&t.paths.lock_file).0);
    assert!(out(&cli(&t, &["--once"])).contains("Checando agora"));
    assert!(cli(&t, &["--status"]).status.success());
    assert_eq!(out(&cli(&t, &["--fundo"])).trim(), "O Tasker já está rodando.");
    std::fs::write(&t.paths.stop_file, "parar").unwrap();
    let mut ok = false;
    for _ in 0..200 {
        if let Some(s) = proc.try_wait().unwrap() {
            ok = s.success();
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if proc.try_wait().unwrap().is_none() {
        let _ = proc.kill();
    }
    assert!(ok, "o serviço não encerrou limpo");
    assert!(!t.paths.ui_file.exists());
    assert!(!store::instance_running(&t.paths.lock_file).0);
}

#[test]
fn cli_bootstrap_and_credentials() {
    let t = T::new("cli3");
    assert_eq!(cli(&t, &["--bootstrap"]).status.code(), Some(1));
    StateStore::new(&t.paths).update(&obj(json!({"refresh": "rt-xyz"}))).unwrap();
    assert!(out(&cli(&t, &["--bootstrap"])).contains("rt-xyz"));
    store::save_credentials(&t.paths, "1", "2").unwrap();
    assert!(out(&cli(&t, &["--forget-credentials"])).contains("apagados"));
    assert!(!t.paths.credentials_file.exists());
    assert_eq!(cli(&t, &["--nada"]).status.code(), Some(2));
}

// ====================================================================== sistema

#[cfg(target_os = "linux")]
#[test]
fn autostart_and_shortcut_linux() {
    let t = T::new("sys");
    assert!(!tasker::system::autostart_enabled(&t.env));
    tasker::system::set_autostart(&t.env, true).unwrap();
    let f = tasker::system::autostart_file(&t.env);
    assert!(f.starts_with(&t.dir.0));
    assert!(std::fs::read_to_string(&f).unwrap().contains("--fundo"));
    assert!(tasker::system::autostart_enabled(&t.env));
    tasker::system::set_autostart(&t.env, false).unwrap();
    assert!(!f.exists());
    let made = tasker::system::create_shortcuts(&t.env);
    assert_eq!(made.len(), 1, "{made:?}"); // sem área de trabalho no HOME de teste: só o menu
    assert!(made[0].starts_with(&*t.dir.0.to_string_lossy()));
    let exe = tasker::system::exe_path();
    assert!(std::fs::read_to_string(&made[0]).unwrap().contains(&*exe.to_string_lossy()));
}

#[cfg(target_os = "linux")]
#[test]
fn stale_python_entries_are_refreshed() {
    let t = T::new("sys2");
    let f = tasker::system::autostart_file(&t.env);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, "[Desktop Entry]\nExec=\"/usr/bin/python3\" \"/x/Tasker.pyw\" --fundo\n").unwrap();
    let fixed = tasker::system::refresh_entries(&t.env);
    assert_eq!(fixed.len(), 1);
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("--fundo") && !text.contains("Tasker.pyw"));
    // Atalho bom (aponta pra um executável que existe) não é mexido.
    std::fs::write(&f, "[Desktop Entry]\nExec=\"/bin/sh\" --fundo\n").unwrap();
    assert!(tasker::system::refresh_entries(&t.env).is_empty());
    assert!(tasker::system::command_is_stale("\"/nao/existe/tasker\" --fundo"));
    // Pela linha de comando: conserta sem tocar em sessão nem subir serviço.
    std::fs::write(&f, "[Desktop Entry]\nExec=\"/usr/bin/python3\" \"/x/Tasker.pyw\"\n").unwrap();
    assert!(out(&cli(&t, &["--atualizar-atalhos"])).contains("Atualizado"));
    assert!(!std::fs::read_to_string(&f).unwrap().contains("Tasker.pyw"));
    assert!(!t.paths.state_file.exists());
}

// ======================================================== login pelo navegador

#[test]
fn browser_login_captures_tokens_via_devtools() {
    let Some(browser) = tasker::system::find_browser(&Arc::new(HashMap::new())) else {
        eprintln!("sem Chrome/Edge/Chromium: pulando");
        return;
    };
    let exp_ms = ((now() + 3600.0) * 1000.0) as i64;
    // Valor embrulhado duas vezes (JSON dentro de string), como o site às vezes faz.
    let sess = json!({"authentication": {"authenticationToken": {
        "accessToken": "tok-a", "refreshToken": "tok-r", "expiresAtTS": exp_ms}}});
    let twice = Value::from(Value::from(sess.to_string()).to_string()).to_string();
    let page = format!("<!doctype html><title>login</title><script>setTimeout(function(){{\
                        sessionStorage.setItem('portal-session', {twice})}}, 800);</script>");
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let server = Arc::new(server);
    let s2 = server.clone();
    std::thread::spawn(move || {
        for req in s2.incoming_requests() {
            let resp = tiny_http::Response::from_string(page.clone())
                .with_header(tiny_http::Header::from_bytes("Content-Type", "text/html").unwrap());
            let _ = req.respond(resp);
        }
    });
    let got = Arc::new(std::sync::Mutex::new(vec![]));
    let g2 = got.clone();
    let env: Env = Arc::new([("TASKER_BROWSER".to_string(), browser.to_string_lossy().into_owned())].into());
    let bl = tasker::browser_login::BrowserLogin::new(Arc::new(Logger::silent()), env,
                                                      Arc::new(move |t| g2.lock().unwrap().push(t)),
                                                      &format!("http://127.0.0.1:{port}/login"));
    *bl.extra_args.lock().unwrap() = vec!["--headless=new".into()];
    bl.start();
    for _ in 0..400 {
        if !bl.active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bl.cancel();
    server.unblock();
    assert_eq!(bl.info().estado, "ok", "{}", bl.info().msg);
    let got = got.lock().unwrap();
    assert_eq!(got[0].access, "tok-a");
    assert_eq!(got[0].refresh.as_deref(), Some("tok-r"));
}

// ============================================================ teste de estresse

/// Servidor de "teste de velocidade" local: arquivos de 8 MB de zeros.
fn big_file_server() -> (String, Arc<tiny_http::Server>) {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
    let s2 = server.clone();
    std::thread::spawn(move || {
        for req in s2.incoming_requests() {
            std::thread::spawn(move || {
                if req.url().starts_with("/quebrado") {
                    let _ = req.respond(tiny_http::Response::empty(500));
                    return;
                }
                let size = 8 * 1024 * 1024;
                let reader = std::io::Read::take(std::io::repeat(0), size as u64);
                let _ = req.respond(tiny_http::Response::new(200.into(), vec![], reader, Some(size), None));
            });
        }
    });
    (url, server)
}

fn wait_stopped(st: &tasker::stress::StressTest) {
    for _ in 0..400 {
        if st.state() == "parado" {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("o teste de estresse não parou");
}

#[test]
fn stress_stops_at_gb_limit() {
    let (url, srv) = big_file_server();
    let log = Arc::new(Logger::silent());
    let st = tasker::stress::StressTest::new(log.clone(), Arc::new(|| Some(0.7)));
    st.start(3, 0.05, 600.0, &format!("{url}/10GB.bin"));
    assert_eq!(st.start(1, 1.0, 1.0, ""), "O teste já está rodando.");
    wait_stopped(&st);
    srv.unblock();
    let info = st.info();
    assert!(info.bytes >= 50_000_000, "{}", info.bytes);
    assert!(info.motivo.contains("limite de 0.05 GB"), "{}", info.motivo);
    assert_eq!(info.volume_minimo, Some(0.7));
    assert!(info.pico_mbps + info.media_mbps > 0.0);
    assert!(log.since(0).linhas.iter().any(|l| l.contains("Teste de estresse terminou")));
}

#[test]
fn stress_manual_stop_and_bad_mirror() {
    let (url, srv) = big_file_server();
    let st = tasker::stress::StressTest::new(Arc::new(Logger::silent()), Arc::new(|| None));
    *st.mirrors.lock().unwrap() = vec![format!("{url}/quebrado"), format!("{url}/ok.bin")]; // um fora do ar
    st.start(2, 100.0, 600.0, "");
    for _ in 0..100 {
        if st.info().bytes > 4_000_000 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(st.stop("parado por você").contains("Parando"));
    wait_stopped(&st);
    srv.unblock();
    assert!(st.info().bytes > 4_000_000);
    assert_eq!(st.info().motivo, "parado por você");
    assert_eq!(st.stop("x"), "O teste não está rodando.");
}

#[test]
fn stress_action_and_refill_count() {
    let (url, srv) = big_file_server();
    let t = T::new("stress");
    let app = App::new(t.paths.clone(), t.env.clone(), Arc::new(Logger::silent()), Some(t.api()));
    app.monitor.update_cfg(|c| {
        c.stress_url = format!("{url}/x.bin");
        c.stress_max_gb = 0.02;
        c.stress_streams = 2;
    });
    record_event(&t.paths, "refill_ok", obj(json!({"before": 0.5, "after": 1.0}))); // antes: não conta
    std::thread::sleep(Duration::from_millis(50));
    assert!(app.action(&json!({"acao": "estresse_iniciar"})).text().contains("iniciado"));
    record_event(&t.paths, "refill_ok", obj(json!({"before": 0.4, "after": 1.0})));
    wait_stopped(&app.stress);
    srv.unblock();
    let st = app.state().estresse;
    assert_eq!((st.refills, st.falhas), (Some(1), Some(0)));
    assert_eq!(st.cfg_gb, 0.02);
}

// ======================================================================= fim

#[test]
fn zz_real_data_untouched() {
    // Nenhum teste usa a pasta dados/ do projeto: tudo vai pra temporárias.
    let real = tasker::paths::base_dir().join("dados");
    for name in ["sessao.json", "config.json", "credenciais.json", "refills.jsonl", "volume.jsonl"] {
        if let Ok(text) = std::fs::read_to_string(real.join(name)) {
            for mark in ["rt-", "assinatura", "tok-a", "01761234567", "rt-xyz"] {
                assert!(!text.contains(mark), "{name} tem rastro de teste ({mark})!");
            }
        }
    }
}
