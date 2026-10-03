"""Integração com o sistema: janela, atalhos e iniciar com o sistema.

A janela é o Chrome/Edge que já está instalado, em modo app (`--app=URL`): sem
abas nem barra de endereço, parece programa. No Windows o Edge sempre existe;
no Linux serve Chrome, Chromium, Edge ou Brave. Sem nenhum deles, abre no
navegador padrão. Nada disso precisa instalar biblioteca.
"""
import os
import shutil
import subprocess
import sys
import webbrowser
from pathlib import Path

from . import paths
from .store import IS_WINDOWS, run_quiet

APP_NAME = "Tasker"
LAUNCHER = paths.BASE_DIR / "Tasker.pyw"
ICON_PNG = paths.UI_DIR / "icon.png"
ICON_ICO = paths.UI_DIR / "icon.ico"

_LINUX_BROWSERS = ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser",
                   "microsoft-edge", "microsoft-edge-stable", "brave-browser")


def find_browser():
    """Executável de um navegador da família Chromium, ou None."""
    override = os.environ.get("TASKER_BROWSER")
    if override:
        return override if Path(override).exists() or shutil.which(override) else None
    if IS_WINDOWS:
        roots = [os.environ.get(k) for k in ("PROGRAMFILES(X86)", "PROGRAMFILES", "LOCALAPPDATA")]
        rels = (r"Microsoft\Edge\Application\msedge.exe",
                r"Google\Chrome\Application\chrome.exe",
                r"Chromium\Application\chrome.exe",
                r"BraveSoftware\Brave-Browser\Application\brave.exe")
        for rel in rels:
            for root in roots:
                if root and Path(root, rel).exists():
                    return str(Path(root, rel))
        return shutil.which("msedge") or shutil.which("chrome")
    if sys.platform == "darwin":
        for app in ("Google Chrome", "Microsoft Edge", "Chromium", "Brave Browser"):
            exe = Path(f"/Applications/{app}.app/Contents/MacOS/{app}")
            if exe.exists():
                return str(exe)
        return None
    for name in _LINUX_BROWSERS:
        exe = shutil.which(name)
        if exe:
            return exe
    return None


def detached_popen(cmd, **kwargs):
    """Processo solto deste: fechar o Tasker não fecha a janela, e vice-versa."""
    kw = {"stdin": subprocess.DEVNULL, "stdout": subprocess.DEVNULL,
          "stderr": subprocess.DEVNULL}
    if IS_WINDOWS:
        kw["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP | 0x00000008  # DETACHED
    else:
        kw["start_new_session"] = True
    kw.update(kwargs)
    return subprocess.Popen(cmd, **kw)


def open_window(url):
    """Abre a tela. True se foi em janela de app; False se caiu no navegador padrão."""
    exe = find_browser()
    if exe:
        try:
            detached_popen([exe, f"--app={url}", "--window-size=1180,880"])
            return True
        except OSError:
            pass
    webbrowser.open(url)
    return False


def pythonw():
    """No Windows, pythonw.exe (sem console) quando existir."""
    exe = Path(sys.executable)
    if IS_WINDOWS:
        alt = exe.with_name("pythonw.exe")
        if alt.exists():
            return str(alt)
    return str(exe)


# -------------------------------------------------------------------- Linux

def _desktop_entry(args=""):
    exec_line = f'"{sys.executable}" "{LAUNCHER}"' + (f" {args}" if args else "")
    return "\n".join([
        "[Desktop Entry]",
        "Type=Application",
        f"Name={APP_NAME}",
        "Comment=Refill automático da Lidl Connect",
        f"Exec={exec_line}",
        f"Path={paths.BASE_DIR}",
        f"Icon={ICON_PNG}",
        "Terminal=false",
        "Categories=Network;Utility;",
        "StartupNotify=false",
        "",
    ])


def _linux_desktop_dir():
    res = run_quiet(["xdg-user-dir", "DESKTOP"], timeout=5)
    if res is not None and res.returncode == 0:
        out = res.stdout.decode().strip()
        if out and Path(out).is_dir():
            return Path(out)
    for name in ("Desktop", "Área de trabalho", "Área de Trabalho"):
        p = Path.home() / name
        if p.is_dir():
            return p
    return None


def _autostart_file():
    base = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config")
    return base / "autostart" / "tasker.desktop"


# ------------------------------------------------------------------ Windows

_RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"

_LNK_PS = r"""
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:T_LNK)
$s.TargetPath = $env:T_TARGET
$s.Arguments = $env:T_ARGS
$s.WorkingDirectory = $env:T_DIR
$s.IconLocation = $env:T_ICON
$s.Description = 'Refill automático da Lidl Connect'
$s.Save()
"""


def _windows_shortcut(lnk):
    env = dict(os.environ, T_LNK=str(lnk), T_TARGET=pythonw(), T_ARGS=f'"{LAUNCHER}"',
               T_DIR=str(paths.BASE_DIR), T_ICON=str(ICON_ICO))
    res = run_quiet(["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy",
                     "Bypass", "-Command", _LNK_PS], timeout=30, env=env)
    return res is not None and res.returncode == 0


def _windows_known_folder(name):
    res = run_quiet(["powershell", "-NoProfile", "-NonInteractive", "-Command",
                     f"[Environment]::GetFolderPath('{name}')"], timeout=20)
    if res is not None and res.returncode == 0:
        out = res.stdout.decode(errors="replace").strip()
        if out:
            return Path(out)
    return None


# ---------------------------------------------------------------- públicas

def autostart_enabled():
    if IS_WINDOWS:
        try:
            import winreg
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, _RUN_KEY) as k:
                winreg.QueryValueEx(k, APP_NAME)
            return True
        except OSError:
            return False
    return _autostart_file().exists()


def set_autostart(enabled):
    """Sobe junto com a sessão, sem abrir janela (--fundo). Devolve mensagem."""
    if IS_WINDOWS:
        import winreg
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER, _RUN_KEY) as k:
            if enabled:
                winreg.SetValueEx(k, APP_NAME, 0, winreg.REG_SZ,
                                  f'"{pythonw()}" "{LAUNCHER}" --fundo')
            else:
                try:
                    winreg.DeleteValue(k, APP_NAME)
                except OSError:
                    pass
    else:
        f = _autostart_file()
        if enabled:
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_text(_desktop_entry("--fundo").replace(
                "StartupNotify=false", "StartupNotify=false\nX-GNOME-Autostart-enabled=true"),
                encoding="utf-8")
        else:
            try:
                f.unlink()
            except FileNotFoundError:
                pass
    return ("O Tasker vai subir sozinho quando você entrar no computador "
            "(em segundo plano; o atalho abre a janela)." if enabled
            else "O Tasker não sobe mais sozinho com o sistema.")


def create_shortcuts():
    """Atalho na área de trabalho e no menu. Devolve a lista de onde criou."""
    made = []
    if IS_WINDOWS:
        for folder in ("Desktop", "Programs"):
            base = _windows_known_folder(folder)
            if base and _windows_shortcut(base / f"{APP_NAME}.lnk"):
                made.append(str(base / f"{APP_NAME}.lnk"))
        return made
    entry = _desktop_entry()
    apps = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share") / "applications"
    targets = [apps / "tasker.desktop"]
    desk = _linux_desktop_dir()
    if desk:
        targets.append(desk / "Tasker.desktop")
    for target in targets:
        try:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(entry, encoding="utf-8")
            target.chmod(0o755)
            # Nemo/Nautilus só executam .desktop da área de trabalho marcado
            # como confiável; sem isso o clique abre o arquivo como texto.
            run_quiet(["gio", "set", str(target), "metadata::trusted", "true"], timeout=5)
            made.append(str(target))
        except OSError:
            continue
    run_quiet(["update-desktop-database", str(apps)], timeout=10)
    return made
