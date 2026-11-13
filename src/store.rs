//! Arquivos com segredo: gravação atômica, permissão só do dono e locks de SO.
//!
//! Os locks são compatíveis com os da versão 2 (Python): `flock` no Linux e um
//! byte travado longe do PID no Windows. Assim a versão nova e a antiga nunca
//! giram a mesma sessão ao mesmo tempo.
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::paths::Paths;
use crate::util::{env_get, now, Env};

pub const IS_WINDOWS: bool = cfg!(windows);
/// Windows trava um byte longe de onde o PID é escrito.
#[cfg(windows)]
const LOCK_BYTE: u32 = 1024;

pub type Json = Map<String, Value>;

// --------------------------------------------------------------- processos

/// Comando que nunca levanta erro: None = programa não existe ou estourou o tempo.
/// No Windows não pisca console.
pub fn run_quiet(cmd: &str, args: &[&str], timeout: Duration, envs: &[(&str, &str)]) -> Option<Output> {
    let mut c = Command::new(cmd);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in envs {
        c.env(k, v);
    }
    no_window(&mut c);
    let mut child = c.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(30)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(windows)]
pub fn no_window(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
pub fn no_window(_c: &mut Command) {}

// ---------------------------------------------------------------- arquivos

/// Legível só pelo dono. No Windows o chmod não protege: quem faz é o icacls.
pub fn protect_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        let user = std::env::var("USERNAME").unwrap_or_default();
        if !user.is_empty() {
            let domain = std::env::var("USERDOMAIN").unwrap_or_default();
            let who = if domain.is_empty() { user } else { format!("{domain}\\{user}") };
            let p = path.to_string_lossy().into_owned();
            run_quiet(
                "icacls",
                &[&p, "/inheritance:r", "/grant:r", &format!("{who}:F")],
                Duration::from_secs(10),
                &[],
            );
        }
    }
}

/// dict do arquivo; None se faltar, estiver quebrado ou não for objeto.
pub fn read_json(path: &Path) -> Option<Json> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

pub fn read_json_or_empty(path: &Path) -> Json {
    read_json(path).unwrap_or_default()
}

/// Grava num temporário (com PID no nome) e troca: nunca fica meio arquivo.
///
/// Arquivo privado NASCE 0600 — nunca existe um instante com senha/token
/// legível por outros usuários.
pub fn write_json(path: &Path, data: &Value, private: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp{}", std::process::id()));
    let result = (|| {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(if private { 0o600 } else { 0o644 });
        }
        let mut f = opts.open(&tmp)?;
        let text = serde_json::to_string_pretty(data).map_err(std::io::Error::other)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        if private {
            protect_file(&tmp); // a permissão viaja junto na troca
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

// ------------------------------------------------------------------- locks

#[cfg(unix)]
fn os_lock(f: &File, blocking: bool) -> bool {
    use std::os::unix::io::AsRawFd;
    let op = libc::LOCK_EX | if blocking { 0 } else { libc::LOCK_NB };
    unsafe { libc::flock(f.as_raw_fd(), op) == 0 }
}

#[cfg(unix)]
fn os_unlock(f: &File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::flock(f.as_raw_fd(), libc::LOCK_UN);
    }
}

#[cfg(windows)]
fn win_lock(f: &File, offset: u32, blocking: bool) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;
    unsafe {
        let mut ov: OVERLAPPED = std::mem::zeroed();
        ov.Anonymous.Anonymous.Offset = offset;
        let flags = LOCKFILE_EXCLUSIVE_LOCK | if blocking { 0 } else { LOCKFILE_FAIL_IMMEDIATELY };
        LockFileEx(f.as_raw_handle() as _, flags, 0, 1, 0, &mut ov) != 0
    }
}

#[cfg(windows)]
fn win_unlock(f: &File, offset: u32) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;
    unsafe {
        let mut ov: OVERLAPPED = std::mem::zeroed();
        ov.Anonymous.Anonymous.Offset = offset;
        UnlockFileEx(f.as_raw_handle() as _, 0, 1, 0, &mut ov);
    }
}

fn open_lock_file(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // "append" pra não truncar o PID de quem tem o lock.
    OpenOptions::new().read(true).append(true).create(true).open(path)
}

/// Lock bloqueante de SO em volta de ler-mesclar-gravar. Best-effort.
pub struct FileLock {
    file: Option<File>,
}

impl FileLock {
    pub fn acquire(path: &Path) -> FileLock {
        let file = open_lock_file(path).ok();
        if let Some(f) = &file {
            #[cfg(unix)]
            os_lock(f, true);
            #[cfg(windows)]
            win_lock(f, 0, true);
        }
        FileLock { file }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Some(f) = &self.file {
            #[cfg(unix)]
            os_unlock(f);
            #[cfg(windows)]
            win_unlock(f, 0);
        }
    }
}

/// Lock de instância única. Cai sozinho se o processo morrer.
pub struct InstanceLock {
    _file: File,
}

/// O lock, ou None se outro processo já tem.
pub fn acquire_instance_lock(path: &Path) -> Option<InstanceLock> {
    let file = open_lock_file(path).ok()?;
    #[cfg(unix)]
    let ok = os_lock(&file, false);
    #[cfg(windows)]
    let ok = win_lock(&file, LOCK_BYTE, false);
    if !ok {
        return None;
    }
    // Truncar e escrever o PID (o arquivo foi aberto em append: reabre pra truncar).
    if let Ok(mut w) = OpenOptions::new().write(true).open(path) {
        let _ = w.set_len(0);
        let _ = w.seek(SeekFrom::Start(0));
        let _ = writeln!(w, "{}", std::process::id());
    }
    Some(InstanceLock { _file: file })
}

/// (rodando?, pid). Testa o lock de verdade — arquivo esquecido não conta.
pub fn instance_running(path: &Path) -> (bool, Option<u32>) {
    let Ok(mut f) = open_lock_file(path) else {
        return (false, None);
    };
    #[cfg(unix)]
    let free = if os_lock(&f, false) {
        os_unlock(&f);
        true
    } else {
        false
    };
    #[cfg(windows)]
    let free = if win_lock(&f, LOCK_BYTE, false) {
        win_unlock(&f, LOCK_BYTE);
        true
    } else {
        false
    };
    if free {
        return (false, None);
    }
    let mut text = String::new();
    let _ = f.seek(SeekFrom::Start(0));
    let _ = Read::by_ref(&mut f).take(32).read_to_string(&mut text);
    (true, text.trim().parse().ok())
}

// ----------------------------------------------------------------- sessão

/// `sessao.json`: tokens + pendência de refill + contadores do login.
///
/// Mesmo formato do `session_tokens.json` da versão 1, do `cloud_state.json` e
/// da versão 2: os três valem sem conversão. Chaves desconhecidas são mantidas.
#[derive(Debug, Clone)]
pub struct StateStore {
    pub path: PathBuf,
}

impl StateStore {
    pub fn new(paths: &Paths) -> StateStore {
        StateStore { path: paths.state_file.clone() }
    }

    pub fn load(&self) -> Json {
        read_json_or_empty(&self.path)
    }

    pub fn update(&self, patch: &Json) -> std::io::Result<Json> {
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let _guard = FileLock::acquire(&self.path.with_file_name(format!("{name}.lk")));
        let mut data = self.load();
        for (k, v) in patch {
            data.insert(k.clone(), v.clone());
        }
        data.insert("saved_at".into(), Value::from(now()));
        write_json(&self.path, &Value::Object(data.clone()), true)?;
        Ok(data)
    }
}

// ------------------------------------------------------------- credenciais

/// (número, senha) do ambiente ou do arquivo.
pub fn load_credentials(paths: &Paths, env: &Env) -> Option<(String, String)> {
    let msisdn = env_get(env, "TASKER_MSISDN").unwrap_or("");
    let password = env.get("TASKER_PASSWORD").map(String::as_str).unwrap_or("");
    if !msisdn.is_empty() && !password.is_empty() {
        return Some((msisdn.to_string(), password.to_string()));
    }
    let data = read_json(&paths.credentials_file)?;
    let msisdn = data.get("msisdn").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let password = data.get("password").and_then(Value::as_str).unwrap_or("").to_string();
    (!msisdn.is_empty() && !password.is_empty()).then_some((msisdn, password))
}

pub fn save_credentials(paths: &Paths, msisdn: &str, password: &str) -> std::io::Result<()> {
    write_json(
        &paths.credentials_file,
        &serde_json::json!({"msisdn": msisdn.trim(), "password": password}),
        true,
    )
}

pub fn forget_credentials(paths: &Paths) -> bool {
    std::fs::remove_file(&paths.credentials_file).is_ok()
}

pub fn has_credentials(paths: &Paths, env: &Env) -> bool {
    load_credentials(paths, env).is_some()
}

// ----------------------------------------------------------------- leitura

pub fn get_f64(m: &Json, key: &str) -> Option<f64> {
    match m.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(_) | Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

pub fn get_str<'a>(m: &'a Json, key: &str) -> Option<&'a str> {
    m.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
