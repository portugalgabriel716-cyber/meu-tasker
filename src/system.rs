//! Integração com o sistema: atalhos, iniciar com o sistema, processos soltos e
//! o navegador (só pro login pelo site da Lidl).
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::store::run_quiet;
use crate::util::{env_get, Env};

pub const APP_NAME: &str = "Tasker";
pub const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
pub const ICON_ICO: &[u8] = include_bytes!("../assets/icon.ico");

pub fn exe_path() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).unwrap_or_else(|| PathBuf::from("tasker"))
}

/// Procura um programa no PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) { vec!["".into(), ".exe".into()] } else { vec!["".into()] };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let p = dir.join(format!("{name}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

const LINUX_BROWSERS: &[&str] = &[
    "google-chrome", "google-chrome-stable", "chromium", "chromium-browser",
    "microsoft-edge", "microsoft-edge-stable", "brave-browser",
];

/// Executável de um navegador da família Chromium, ou None.
pub fn find_browser(env: &Env) -> Option<PathBuf> {
    if let Some(over) = env_get(env, "TASKER_BROWSER") {
        let p = PathBuf::from(over);
        return if p.exists() { Some(p) } else { which(over) };
    }
    if cfg!(windows) {
        let roots: Vec<String> = ["PROGRAMFILES(X86)", "PROGRAMFILES", "LOCALAPPDATA"]
            .iter()
            .filter_map(|k| env.get(*k).cloned())
            .collect();
        let rels = [
            r"Microsoft\Edge\Application\msedge.exe",
            r"Google\Chrome\Application\chrome.exe",
            r"Chromium\Application\chrome.exe",
            r"BraveSoftware\Brave-Browser\Application\brave.exe",
        ];
        for rel in rels {
            for root in &roots {
                let p = Path::new(root).join(rel);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        return which("msedge").or_else(|| which("chrome"));
    }
    if cfg!(target_os = "macos") {
        for app in ["Google Chrome", "Microsoft Edge", "Chromium", "Brave Browser"] {
            let p = PathBuf::from(format!("/Applications/{app}.app/Contents/MacOS/{app}"));
            if p.exists() {
                return Some(p);
            }
        }
        return None;
    }
    LINUX_BROWSERS.iter().find_map(|b| which(b))
}

/// Processo solto deste: fechar um não fecha o outro.
pub fn spawn_detached(exe: &Path, args: &[&str]) -> std::io::Result<Child> {
    let mut c = Command::new(exe);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        c.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    c.spawn()
}

fn home(env: &Env) -> PathBuf {
    env_get(env, "HOME")
        .or_else(|| env_get(env, "USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn xdg_config(env: &Env) -> PathBuf {
    env_get(env, "XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home(env).join(".config"))
}

fn xdg_data(env: &Env) -> PathBuf {
    env_get(env, "XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home(env).join(".local/share"))
}

/// Ícone em disco (atalhos e notificações precisam de um arquivo).
pub fn icon_path() -> PathBuf {
    icon_path_in(&crate::util::process_env())
}

pub fn icon_path_in(env: &Env) -> PathBuf {
    let (dir, name, bytes) = if cfg!(windows) {
        let base = env_get(env, "LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home(env));
        (base.join("Tasker"), "tasker.ico", ICON_ICO)
    } else {
        (xdg_data(env).join("icons"), "tasker.png", ICON_PNG)
    };
    let path = dir.join(name);
    if std::fs::read(&path).ok().as_deref() != Some(bytes) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&path, bytes);
    }
    path
}

// -------------------------------------------------------------------- Linux

fn desktop_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        if matches!(ch, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

pub fn desktop_entry(env: &Env, exe: &Path, args: &str, autostart: bool) -> String {
    let mut exec = desktop_quote(&exe.to_string_lossy());
    if !args.is_empty() {
        exec = format!("{exec} {args}");
    }
    let dir = exe.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut lines = vec![
        "[Desktop Entry]".to_string(),
        "Type=Application".into(),
        format!("Name={APP_NAME}"),
        "Comment=Refill automático da Lidl Connect".into(),
        format!("Exec={exec}"),
        format!("Path={dir}"),
        format!("Icon={}", icon_path_in(env).to_string_lossy()),
        "Terminal=false".into(),
        "Categories=Network;Utility;".into(),
        "StartupNotify=false".into(),
        "StartupWMClass=tasker".into(),
    ];
    if autostart {
        lines.push("X-GNOME-Autostart-enabled=true".into());
    }
    lines.join("\n") + "\n"
}

fn linux_desktop_dir(env: &Env) -> Option<PathBuf> {
    // O HOME é o do ambiente recebido (nos testes, uma pasta temporária), nunca
    // o do processo: senão um teste mexeria na área de trabalho de verdade.
    let home = PathBuf::from(env_get(env, "HOME")?);
    let home_s = home.to_string_lossy().into_owned();
    let cfg_s = xdg_config(env).to_string_lossy().into_owned();
    let envs = [("HOME", home_s.as_str()), ("XDG_CONFIG_HOME", cfg_s.as_str())];
    if let Some(out) = run_quiet("xdg-user-dir", &["DESKTOP"], Duration::from_secs(5), &envs) {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.is_dir() && p != home && p.starts_with(&home) {
                return Some(p);
            }
        }
    }
    ["Desktop", "Área de trabalho", "Área de Trabalho"]
        .iter()
        .map(|n| home.join(n))
        .find(|p| p.is_dir())
}

pub fn autostart_file(env: &Env) -> PathBuf {
    xdg_config(env).join("autostart").join("tasker.desktop")
}

fn menu_file(env: &Env) -> PathBuf {
    xdg_data(env).join("applications").join("tasker.desktop")
}

fn write_entry(target: &Path, text: &str) -> bool {
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(target, text).is_err() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755));
    }
    // Nemo/Nautilus só executam .desktop da área de trabalho marcado como
    // confiável; sem isso o clique abre o arquivo como texto.
    run_quiet("gio", &["set", &target.to_string_lossy(), "metadata::trusted", "true"], Duration::from_secs(5), &[]);
    true
}

// ------------------------------------------------------------------ Windows

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
const LNK_PS: &str = r#"
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:T_LNK)
$s.TargetPath = $env:T_TARGET
$s.Arguments = ''
$s.WorkingDirectory = $env:T_DIR
$s.IconLocation = $env:T_ICON
$s.Description = 'Refill automático da Lidl Connect'
$s.Save()
"#;

#[cfg(windows)]
fn windows_shortcut(env: &Env, lnk: &Path) -> bool {
    let exe = exe_path();
    let dir = exe.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let icon = icon_path_in(env);
    let res = run_quiet(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", LNK_PS],
        Duration::from_secs(30),
        &[("T_LNK", &lnk.to_string_lossy()), ("T_TARGET", &exe.to_string_lossy()),
          ("T_DIR", &dir), ("T_ICON", &icon.to_string_lossy())],
    );
    res.is_some_and(|r| r.status.success())
}

#[cfg(windows)]
fn windows_known_folder(name: &str) -> Option<PathBuf> {
    let out = run_quiet(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &format!("[Environment]::GetFolderPath('{name}')")],
        Duration::from_secs(20),
        &[],
    )?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then(|| PathBuf::from(s))
}

#[cfg(windows)]
fn run_value() -> String {
    format!("\"{}\" --fundo", exe_path().to_string_lossy())
}

// ---------------------------------------------------------------- públicas

pub fn autostart_enabled(env: &Env) -> bool {
    #[cfg(windows)]
    {
        let _ = env;
        use winreg::enums::HKEY_CURRENT_USER;
        winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|k| k.get_value::<String, _>(APP_NAME))
            .is_ok()
    }
    #[cfg(not(windows))]
    {
        autostart_file(env).exists()
    }
}

/// Sobe junto com a sessão, sem abrir janela (--fundo). Devolve mensagem.
pub fn set_autostart(env: &Env, enabled: bool) -> std::io::Result<String> {
    #[cfg(windows)]
    {
        let _ = env;
        use winreg::enums::HKEY_CURRENT_USER;
        let (key, _) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
        if enabled {
            key.set_value(APP_NAME, &run_value())?;
        } else {
            let _ = key.delete_value(APP_NAME);
        }
    }
    #[cfg(not(windows))]
    {
        let f = autostart_file(env);
        if enabled {
            if !write_entry(&f, &desktop_entry(env, &exe_path(), "--fundo", true)) {
                return Err(std::io::Error::other(format!("não consegui gravar {}", f.display())));
            }
        } else {
            let _ = std::fs::remove_file(&f);
        }
    }
    Ok(if enabled {
        "O Tasker vai subir sozinho quando você entrar no computador (em segundo plano; o atalho abre a janela).".into()
    } else {
        "O Tasker não sobe mais sozinho com o sistema.".into()
    })
}

/// Atalho na área de trabalho e no menu. Devolve a lista de onde criou.
pub fn create_shortcuts(env: &Env) -> Vec<String> {
    let mut made = vec![];
    #[cfg(windows)]
    {
        for folder in ["Desktop", "Programs"] {
            if let Some(base) = windows_known_folder(folder) {
                let lnk = base.join(format!("{APP_NAME}.lnk"));
                if windows_shortcut(env, &lnk) {
                    made.push(lnk.to_string_lossy().into_owned());
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let entry = desktop_entry(env, &exe_path(), "", false);
        let mut targets = vec![menu_file(env)];
        if let Some(desk) = linux_desktop_dir(env) {
            targets.push(desk.join("Tasker.desktop"));
        }
        for t in targets {
            if write_entry(&t, &entry) {
                made.push(t.to_string_lossy().into_owned());
            }
        }
        if let Some(apps) = menu_file(env).parent() {
            run_quiet("update-desktop-database", &[&apps.to_string_lossy()], Duration::from_secs(10), &[]);
        }
    }
    made
}

/// Linha de comando de um atalho velho: a versão em Python, ou um executável que
/// não existe mais. Atalho que abre outro Tasker que existe fica como está.
pub fn command_is_stale(cmd: &str) -> bool {
    if cmd.contains(".pyw") || cmd.contains("Tasker.py") {
        return true;
    }
    let cmd = cmd.trim();
    let first = if let Some(rest) = cmd.strip_prefix('"') {
        rest.split('"').next().unwrap_or("").replace("\\\\", "\\")
    } else {
        cmd.split_whitespace().next().unwrap_or("").to_string()
    };
    !first.is_empty() && !Path::new(&first).exists()
}

#[cfg_attr(windows, allow(dead_code))]
fn entry_is_stale(text: &str) -> bool {
    text.lines().find_map(|l| l.strip_prefix("Exec=")).is_some_and(command_is_stale)
}

#[cfg(windows)]
fn windows_lnk_is_stale(lnk: &Path) -> bool {
    let ps = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:T_LNK); \
              Write-Output ('\"' + $s.TargetPath + '\" ' + $s.Arguments)";
    run_quiet("powershell", &["-NoProfile", "-NonInteractive", "-Command", ps], Duration::from_secs(20),
              &[("T_LNK", &lnk.to_string_lossy())])
        .is_some_and(|o| command_is_stale(String::from_utf8_lossy(&o.stdout).trim()))
}

/// Atalhos que já existem mas estão velhos (ver `command_is_stale`) passam a
/// abrir este executável. Devolve quais mudou.
pub fn refresh_entries(env: &Env) -> Vec<String> {
    let mut fixed = vec![];
    #[cfg(not(windows))]
    {
        let exe = exe_path();
        let mut items = vec![(autostart_file(env), desktop_entry(env, &exe, "--fundo", true))];
        let entry = desktop_entry(env, &exe, "", false);
        items.push((menu_file(env), entry.clone()));
        if let Some(desk) = linux_desktop_dir(env) {
            items.push((desk.join("Tasker.desktop"), entry));
        }
        for (path, want) in items {
            if let Ok(have) = std::fs::read_to_string(&path) {
                if entry_is_stale(&have) && write_entry(&path, &want) {
                    fixed.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
        if let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_ALL_ACCESS) {
            if let Ok(v) = key.get_value::<String, _>(APP_NAME) {
                if command_is_stale(&v) && key.set_value(APP_NAME, &run_value()).is_ok() {
                    fixed.push("registro (iniciar com o Windows)".into());
                }
            }
        }
        for folder in ["Desktop", "Programs"] {
            if let Some(base) = windows_known_folder(folder) {
                let lnk = base.join(format!("{APP_NAME}.lnk"));
                if lnk.exists() && windows_lnk_is_stale(&lnk) && windows_shortcut(env, &lnk) {
                    fixed.push(lnk.to_string_lossy().into_owned());
                }
            }
        }
    }
    fixed
}
