//! Avisos: notificação do sistema (Linux/Windows) + ntfy.sh + webhook.
//!
//! Tudo em thread e best-effort: aviso é conveniência e nunca trava o monitor. Só
//! passam por aqui as mensagens importantes (login caiu, refill falhando).
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::Cfg;
use crate::logs::Logger;
use crate::store::run_quiet;

pub trait Notify: Send + Sync {
    /// Dispara o aviso por todos os canais ligados. Devolve quantos.
    fn send(&self, msg: &str) -> usize;
}

/// Toast nativo do Windows 10/11 via WinRT. O AppID do próprio PowerShell é o
/// atalho que dispensa registrar um AppUserModelID. A mensagem vai por variável
/// de ambiente pra não precisar escapar aspas.
#[cfg(windows)]
const TOAST_PS: &str = r#"
$ErrorActionPreference = 'Stop'
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
$xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent(
    [Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$texts = $xml.GetElementsByTagName('text')
[void]$texts.Item(0).AppendChild($xml.CreateTextNode('Tasker'))
[void]$texts.Item(1).AppendChild($xml.CreateTextNode($env:TASKER_NOTIFY_MSG))
$app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show(
    [Windows.UI.Notifications.ToastNotification]::new($xml))
"#;

pub fn desktop(msg: &str) {
    #[cfg(windows)]
    {
        let res = run_quiet(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", TOAST_PS],
            Duration::from_secs(25),
            &[("TASKER_NOTIFY_MSG", msg)],
        );
        if res.is_some_and(|r| r.status.success()) {
            return;
        }
        // Toast desligado por política: cai pra caixa de diálogo — estes avisos
        // (os que pedem login) não podem sumir.
        use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (m, t) = (wide(msg), wide("Tasker"));
        unsafe {
            MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), 0x30 | 0x10000 | 0x40000);
        }
    }
    #[cfg(not(windows))]
    {
        let icon = crate::system::icon_path();
        let icon = icon.to_string_lossy();
        run_quiet(
            "notify-send",
            &["-u", "critical", "-a", "Tasker", "-i", &icon, "Tasker", msg],
            Duration::from_secs(5),
            &[],
        );
    }
}

fn post(url: &str, body: Vec<u8>, headers: &[(&str, &str)]) {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    let mut req = agent.post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let _ = req.send_bytes(&body);
}

pub struct Notifier {
    cfg: Arc<dyn Fn() -> Cfg + Send + Sync>,
    log: Option<Arc<Logger>>,
    ntfy_base: String,
}

impl Notifier {
    pub fn new(cfg: Arc<dyn Fn() -> Cfg + Send + Sync>, log: Option<Arc<Logger>>, ntfy_base: Option<&str>) -> Notifier {
        Notifier { cfg, log, ntfy_base: ntfy_base.unwrap_or("https://ntfy.sh").trim_end_matches('/').into() }
    }
}

impl Notify for Notifier {
    fn send(&self, msg: &str) -> usize {
        let cfg = (self.cfg)();
        if let Some(log) = &self.log {
            log.log(format!("Aviso: {msg}"));
        }
        let mut jobs: Vec<Box<dyn FnOnce() + Send>> = vec![];
        if cfg.notify {
            let m = msg.to_string();
            jobs.push(Box::new(move || desktop(&m)));
        }
        let topic = cfg.ntfy_topic.trim().to_string();
        if !topic.is_empty() {
            let url = if topic.starts_with("http") { topic } else { format!("{}/{topic}", self.ntfy_base) };
            let body = msg.as_bytes().to_vec();
            jobs.push(Box::new(move || {
                post(&url, body, &[("Title", "Tasker"), ("Priority", "high"),
                                   ("Content-Type", "text/plain; charset=utf-8")])
            }));
        }
        let hook = cfg.webhook.trim().to_string();
        if !hook.is_empty() {
            let body = serde_json::json!({"text": format!("Tasker: {msg}")}).to_string().into_bytes();
            jobs.push(Box::new(move || post(&hook, body, &[("Content-Type", "application/json")])));
        }
        let n = jobs.len();
        for job in jobs {
            std::thread::spawn(job);
        }
        n
    }
}

/// Guarda as mensagens em vez de avisar (testes).
#[derive(Default)]
pub struct Recorder {
    pub msgs: Mutex<Vec<String>>,
}

impl Notify for Recorder {
    fn send(&self, msg: &str) -> usize {
        self.msgs.lock().unwrap().push(msg.to_string());
        1
    }
}
