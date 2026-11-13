//! Ponto de entrada: a janela (padrão), o serviço, os modos de servidor e a linha
//! de comando.
//!
//!     tasker                     abre o Tasker (sobe o serviço se preciso + janela)
//!     tasker --fundo             só o serviço, sem janela (iniciar com o sistema)
//!     tasker --loop              só o motor, log no terminal (VM/Docker)
//!     tasker --once              uma checagem e sai (cron/GitHub Actions)
//!     tasker --status            mostra o volume e sai
//!     tasker --login             login pelo navegador e sai
//!     tasker --set-credentials | --forget-credentials
//!     tasker --test-notification | --bootstrap | --atualizar-atalhos
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tasker::client::Client;
use tasker::daemon::App;
use tasker::logs::Logger;
use tasker::notify::{Notifier, Notify};
use tasker::paths::Paths;
use tasker::store::{self, StateStore};
use tasker::types::MonitorSnap;
use tasker::util::{fmt_g, fmt_local, now, process_env, Env};
use tasker::{config, engine, system};

const HELP: &str = "\
Tasker — refill automático da Lidl Connect

uso: tasker [modo] [--porta N]

  (sem nada)            abre o Tasker (ou só a janela, se ele já estiver rodando)
  --fundo               sobe sem abrir a janela
  --loop                só o motor, sem tela (servidor/Docker)
  --once                uma checagem e sai (cron)
  --status              mostra o volume e sai
  --login               login pelo navegador e sai
  --set-credentials     guarda número e senha
  --forget-credentials  apaga número e senha
  --test-notification   manda um aviso de teste
  --bootstrap           mostra o refresh token (pra levar a sessão pra um servidor)
  --atualizar-atalhos   conserta atalhos que ainda abrem a versão antiga
  --porta N             porta do serviço local (padrão 8765)
  --version             mostra a versão
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Gui,
    Fundo,
    Loop,
    Once,
    Status,
    Login,
    SetCredentials,
    ForgetCredentials,
    TestNotification,
    Bootstrap,
    RefreshShortcuts,
}

static SIGNALED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    SIGNALED.store(true, Ordering::SeqCst);
}

fn install_signals(ignore_hup: bool) {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        if ignore_hup {
            // Aberto por um terminal: fechar o terminal não derruba o Tasker.
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
        }
    }
    #[cfg(not(unix))]
    let _ = ignore_hup;
}

/// Chama `f` quando chegar SIGTERM/SIGINT.
fn on_signals(f: impl Fn() + Send + 'static) {
    std::thread::spawn(move || loop {
        if SIGNALED.load(Ordering::SeqCst) {
            f();
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    });
}

#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn console() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

fn parse(args: &[String]) -> Result<(Mode, u16), String> {
    let mut mode = Mode::Gui;
    let mut port = 8765u16;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let m = match a.as_str() {
            "--fundo" => Mode::Fundo,
            "--loop" => Mode::Loop,
            "--once" => Mode::Once,
            "--status" => Mode::Status,
            "--login" => Mode::Login,
            "--set-credentials" => Mode::SetCredentials,
            "--forget-credentials" => Mode::ForgetCredentials,
            "--test-notification" => Mode::TestNotification,
            "--bootstrap" => Mode::Bootstrap,
            "--atualizar-atalhos" => Mode::RefreshShortcuts,
            "--porta" => {
                port = it.next().and_then(|p| p.parse().ok()).ok_or("--porta precisa de um número")?;
                continue;
            }
            "-h" | "--help" => return Err(String::new()),
            "--version" | "-V" => {
                println!("tasker {}", tasker::VERSION);
                std::process::exit(0);
            }
            other => return Err(format!("opção desconhecida: {other}")),
        };
        if mode != Mode::Gui {
            return Err("escolha um modo só".into());
        }
        mode = m;
    }
    Ok((mode, port))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if !args.is_empty() {
        attach_console();
    }
    let (mode, port) = match parse(&args) {
        Ok(x) => x,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("tasker: {e}\n");
            }
            print!("{HELP}");
            std::process::exit(if e.is_empty() { 0 } else { 2 });
        }
    };
    std::process::exit(run(mode, port));
}

fn run(mode: Mode, port: u16) -> i32 {
    let env = process_env();
    let paths = Arc::new(Paths::from_env(&env));
    let moved = paths.migrate_legacy(None);
    paths.ensure_data_dir();

    match mode {
        Mode::ForgetCredentials => {
            println!("{}", if store::forget_credentials(&paths) { "Número e senha apagados." } else { "Não havia nada guardado." });
            return 0;
        }
        Mode::SetCredentials => return set_credentials(&paths),
        Mode::TestNotification => {
            let (p, e) = (paths.clone(), env.clone());
            let n = Notifier::new(Arc::new(move || config::load(&p, &e)), None, tasker::util::env_get(&env, "TASKER_NTFY_BASE"))
                .send("Teste do Tasker: se você está lendo isto, os avisos funcionam.");
            if n > 0 {
                std::thread::sleep(Duration::from_secs(3)); // as threads de aviso precisam de um instante
            }
            println!("{}", if n > 0 { "Aviso de teste disparado." } else { "Os avisos estão desligados nos ajustes." });
            return 0;
        }
        Mode::Bootstrap => {
            let st = StateStore::new(&paths).load();
            let Some(rt) = store::get_str(&st, "refresh") else {
                println!("Não há sessão salva. Faça login no Tasker primeiro.");
                return 1;
            };
            println!("Refresh token (guarde como segredo TASKER_REFRESH_TOKEN no servidor):\n");
            println!("{rt}");
            println!("\nO refresh token ROTACIONA e a conta aceita UMA sessão: depois que o");
            println!("servidor assumir, feche o Tasker daqui (e vice-versa).");
            return 0;
        }
        Mode::RefreshShortcuts => {
            let fixed = system::refresh_entries(&env);
            if fixed.is_empty() {
                println!("Nenhum atalho velho.");
            }
            for f in fixed {
                println!("Atualizado: {f}");
            }
            return 0;
        }
        Mode::Gui => return open_gui(paths, env, port),
        _ => {}
    }

    let Some(_lock) = store::acquire_instance_lock(&paths.lock_file) else {
        return with_running_instance(mode, &paths);
    };
    let service = mode == Mode::Fundo;
    let log = Arc::new(Logger::new(Some(paths.log_file.clone()), console() || !service,
                                   matches!(mode, Mode::Loop | Mode::Once)));
    if service {
        log.preload(200);
    }
    if !moved.is_empty() {
        log.log(format!("Arquivos da versão 1 movidos pra {}/: {}.",
                        paths.data_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        moved.join(", ")));
    }
    let app = App::new(paths.clone(), env.clone(), log.clone(), None);
    match mode {
        Mode::Status => {
            app.monitor.load_state();
            match app.monitor.read_now() {
                Ok((units, cooldown)) => {
                    for (name, unit) in [("refill", &units.refill), ("data", &units.data)] {
                        if let Some(u) = unit {
                            let g = |k: &str| u.get(k).map(|v| v.to_string()).unwrap_or_else(|| "None".into());
                            println!("{name}: {}/{} {}", g("left"), g("max"), u.get("unit").and_then(|v| v.as_str()).unwrap_or(""));
                        }
                    }
                    println!("cooldown do refill: {}", cooldown.map(|c| if c { "True" } else { "False" }).unwrap_or("None"));
                    0
                }
                Err(e) => {
                    println!("Não consegui ler o volume: {e}");
                    1
                }
            }
        }
        Mode::Login => {
            println!("{}", app.login.start());
            while app.login.active() {
                std::thread::sleep(Duration::from_millis(500));
            }
            let info = app.login.info();
            println!("{}", info.msg);
            if info.estado == "ok" { 0 } else { 1 }
        }
        Mode::Once => {
            app.monitor.run(true);
            let snap = app.monitor.snapshot();
            if snap.erro.is_some() || snap.precisa_login.is_some() { 1 } else { 0 }
        }
        Mode::Loop => {
            install_signals(false);
            let m = app.monitor.clone();
            on_signals(move || m.stop(Duration::ZERO));
            let stop = Arc::new(AtomicBool::new(false));
            let m2 = app.monitor.clone();
            let watcher = engine::watch_control_files(app.monitor.clone(), Arc::new(move || m2.stop(Duration::ZERO)), stop.clone());
            app.monitor.run(false);
            stop.store(true, Ordering::SeqCst);
            let _ = watcher.join();
            0
        }
        Mode::Fundo => {
            install_signals(true);
            let a = app.clone();
            on_signals(move || a.shutdown());
            match app.run_service(port) {
                Ok(()) => 0,
                Err(e) => {
                    log.log(format!("Não consegui subir o serviço: {e}"));
                    1
                }
            }
        }
        _ => 0,
    }
}

/// Já tem um Tasker com esta sessão: conversa com ele em vez de brigar.
fn with_running_instance(mode: Mode, paths: &Paths) -> i32 {
    let client = Client::find(paths);
    let ask = |acao: &str| -> Option<String> {
        client.as_ref()?.post::<tasker::types::ActionResult>("/api/acao", &serde_json::json!({"acao": acao})).ok().map(|r| r.text())
    };
    match mode {
        Mode::Fundo => {
            println!("O Tasker já está rodando.");
            0
        }
        Mode::Once => {
            println!("{}", ask("checar").unwrap_or_else(|| "Já tem outra execução nesta sessão — saindo.".into()));
            0 // pro cron, isso não é erro
        }
        Mode::Status => {
            let snap: Option<MonitorSnap> = store::read_json(&paths.status_file)
                .and_then(|m| serde_json::from_value(serde_json::Value::Object(m)).ok());
            match &snap {
                Some(s) => println!("{}", fmt_status(s)),
                None => println!("O Tasker está rodando, mas ainda sem leitura."),
            }
            let last = snap.and_then(|s| s.ultima_checagem).unwrap_or(0.0);
            if now() - last < 15.0 * 60.0 { 0 } else { 1 } // healthcheck do Docker
        }
        Mode::Login => match ask("login_navegador") {
            Some(m) => {
                println!("{m}");
                0
            }
            None => {
                println!("O Tasker está rodando sem tela: pare ele antes.");
                1
            }
        },
        _ => {
            println!("Já tem um Tasker rodando com esta sessão — saindo.");
            1
        }
    }
}

fn fmt_status(s: &MonitorSnap) -> String {
    let mut lines = vec![];
    if let Some(v) = &s.volume {
        let mut l = format!("Refill: {:.2}/{} GB", v.left, fmt_g(v.total.unwrap_or(0.0)));
        if let Some(p) = v.plan_left {
            l += &format!(" | plano: {} GB", fmt_g(p));
        }
        lines.push(l);
    }
    if let Some(t) = s.ultima_checagem {
        lines.push(format!("Última checagem: {}", fmt_local(t, "%d/%m %H:%M:%S")));
    }
    lines.push(format!("Fase: {}", s.fase));
    if let Some(p) = &s.precisa_login {
        lines.push(p.clone());
    }
    if let Some(e) = &s.erro {
        lines.push(format!("Último erro: {e}"));
    }
    lines.join("\n")
}

/// Garante o serviço no ar e abre a janela.
fn open_gui(paths: Arc<Paths>, env: Env, port: u16) -> i32 {
    let (running, _) = store::instance_running(&paths.lock_file);
    let alive = |p: &Paths| Client::find(p).is_some_and(|c| c.alive());
    if !running {
        // O serviço é um processo à parte: fechar a janela não para o monitor.
        let port_s = port.to_string();
        if let Err(e) = system::spawn_detached(&system::exe_path(), &["--fundo", "--porta", &port_s]) {
            eprintln!("Não consegui subir o serviço: {e}");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !alive(&paths) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // Uma janela por vez: com uma já aberta, ela só vem pra frente.
    let Some(_win) = store::acquire_instance_lock(&paths.window_lock) else {
        if let Some(c) = Client::find(&paths) {
            let _ = c.post::<serde_json::Value>("/api/acao", &serde_json::json!({"acao": "focar"}));
        }
        return 0;
    };
    show_window(paths, env)
}

#[cfg(feature = "janela")]
fn show_window(paths: Arc<Paths>, env: Env) -> i32 {
    tasker::gui::run(paths, env)
}

#[cfg(not(feature = "janela"))]
fn show_window(_paths: Arc<Paths>, _env: Env) -> i32 {
    println!("Este Tasker foi compilado sem janela. O serviço está rodando; use --status, --once ou --fundo.");
    0
}

fn read_password(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    #[cfg(unix)]
    let restore = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) == 0 {
            let old = t;
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &t);
            Some(old)
        } else {
            None
        }
    };
    #[cfg(windows)]
    let restore = unsafe {
        use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE};
        let h = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT);
            Some(mode)
        } else {
            None
        }
    };
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    #[cfg(unix)]
    if let Some(old) = restore {
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &old);
        }
    }
    #[cfg(windows)]
    if let Some(mode) = restore {
        unsafe {
            use windows_sys::Win32::System::Console::{GetStdHandle, SetConsoleMode, STD_INPUT_HANDLE};
            SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), mode);
        }
    }
    println!();
    line.trim_end_matches(['\r', '\n']).to_string()
}

fn set_credentials(paths: &Paths) -> i32 {
    println!("Com número e senha guardados, o Tasker refaz o login sozinho quando a");
    println!("sessão cair. Ficam em {}, legível só pelo seu usuário.\n", paths.credentials_file.display());
    print!("Número (Mobilfunknummer): ");
    let _ = std::io::stdout().flush();
    let mut msisdn = String::new();
    let _ = std::io::stdin().read_line(&mut msisdn);
    let msisdn = msisdn.trim().to_string();
    let password = if msisdn.is_empty() { String::new() } else { read_password("Senha (não aparece): ") };
    if msisdn.is_empty() || password.is_empty() {
        println!("Cancelado.");
        return 1;
    }
    if let Err(e) = store::save_credentials(paths, &msisdn, &password) {
        println!("Não consegui guardar: {e}");
        return 1;
    }
    let _ = StateStore::new(paths).update(&serde_json::from_value(serde_json::json!({"login_fails": 0, "login_blocked_until": 0})).unwrap());
    println!("Guardado.");
    0
}
