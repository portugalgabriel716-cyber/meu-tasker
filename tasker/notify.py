"""Avisos: notificação do sistema (Linux/Windows) + ntfy.sh + webhook.

Tudo em thread e best-effort: aviso é conveniência e nunca trava o monitor. Só
passam por aqui as mensagens importantes (login caiu, refill falhando).
"""
import json
import os
import subprocess
import threading
import urllib.request

from .store import IS_WINDOWS, run_quiet

NTFY_BASE = os.environ.get("TASKER_NTFY_BASE", "https://ntfy.sh")

# Toast nativo do Windows 10/11 via WinRT. O AppID do próprio PowerShell é o
# atalho que dispensa registrar um AppUserModelID. A mensagem vai por variável
# de ambiente pra não precisar escapar aspas.
_TOAST_PS = r"""
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
"""


def _desktop_windows(msg):
    res = run_quiet(["powershell", "-NoProfile", "-NonInteractive",
                     "-ExecutionPolicy", "Bypass", "-Command", _TOAST_PS],
                    timeout=25, env=dict(os.environ, TASKER_NOTIFY_MSG=msg))
    if res is not None and res.returncode == 0:
        return
    # Toast desligado por política: cai pra caixa de diálogo — estes avisos
    # (os que pedem login) não podem sumir.
    try:
        import ctypes
        ctypes.windll.user32.MessageBoxW(None, msg, "Tasker", 0x30 | 0x10000 | 0x40000)
    except Exception:
        pass


def _desktop_linux(msg):
    try:
        subprocess.run(["notify-send", "-u", "critical", "-a", "Tasker", "Tasker", msg],
                       capture_output=True, timeout=5)
    except Exception:
        pass


def _post(url, data, headers):
    try:
        req = urllib.request.Request(url, data=data, headers=headers)
        urllib.request.urlopen(req, timeout=15).read()
    except Exception:
        pass


class Notifier:
    def __init__(self, settings_getter, log=None):
        self._settings = settings_getter
        self.log = log

    def send(self, msg):
        cfg = self._settings()
        if self.log:
            self.log(f"Aviso: {msg}")
        jobs = []
        if cfg.get("notify"):
            jobs.append((_desktop_windows if IS_WINDOWS else _desktop_linux, (msg,)))
        topic = (cfg.get("ntfy_topic") or "").strip()
        if topic:
            url = topic if topic.startswith("http") else f"{NTFY_BASE}/{topic}"
            jobs.append((_post, (url, msg.encode("utf-8"),
                                 {"Title": "Tasker", "Priority": "high",
                                  "Content-Type": "text/plain; charset=utf-8"})))
        hook = (cfg.get("webhook") or "").strip()
        if hook:
            jobs.append((_post, (hook, json.dumps({"text": f"Tasker: {msg}"}).encode(),
                                 {"Content-Type": "application/json"})))
        for fn, args in jobs:
            threading.Thread(target=fn, args=args, daemon=True).start()
        return len(jobs)
