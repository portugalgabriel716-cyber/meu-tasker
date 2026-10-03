"""Arquivos com segredo: gravação atômica, permissão só do dono e locks de SO."""
import json
import os
import subprocess
import time

from . import paths

IS_WINDOWS = os.name == "nt"
if IS_WINDOWS:
    import msvcrt
else:
    import fcntl

# Sem isso cada ajudante (icacls, taskkill, powershell) pisca um console.
NO_WINDOW = 0x08000000 if IS_WINDOWS else 0
LOCK_BYTE = 1024   # Windows trava um byte longe de onde o PID é escrito


def run_quiet(cmd, timeout=10, **kwargs):
    """subprocess.run que nunca levanta. None = programa não existe/estourou."""
    try:
        return subprocess.run(cmd, capture_output=True, timeout=timeout,
                              creationflags=NO_WINDOW, **kwargs)
    except Exception:
        return None


def protect_file(path):
    """Legível só pelo dono. No Windows o chmod não protege: quem faz é o icacls."""
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass
    if IS_WINDOWS:
        user = os.environ.get("USERNAME") or ""
        if user:
            domain = os.environ.get("USERDOMAIN") or ""
            who = f"{domain}\\{user}" if domain else user
            run_quiet(["icacls", str(path), "/inheritance:r", "/grant:r", f"{who}:F"])


_EMPTY = object()


def read_json(path, default=_EMPTY):
    """dict do arquivo; `default` (padrão {}) se faltar, estiver quebrado ou não for dict."""
    if default is _EMPTY:
        default = {}
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, dict) else default
    except (OSError, ValueError):
        return default


def write_json(path, data, private=False):
    """Grava num temporário (com PID no nome) e troca: nunca fica meio arquivo.

    Arquivo privado NASCE 0600 — nunca existe um instante com senha/token
    legível por outros usuários.
    """
    paths.ensure_data_dir()
    tmp = path.with_name(f"{path.name}.tmp{os.getpid()}")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600 if private else 0o644)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=1)
        if private:
            protect_file(tmp)   # a permissão viaja junto no replace
        os.replace(tmp, path)
    except OSError:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


class FileLock:
    """Lock bloqueante de SO em volta de ler-mesclar-gravar. Best-effort."""

    def __init__(self, path):
        self.path = path
        self.handle = None

    def __enter__(self):
        try:
            paths.ensure_data_dir()
            self.handle = open(self.path, "a+")
            if IS_WINDOWS:
                for _ in range(100):
                    try:
                        self.handle.seek(0)
                        msvcrt.locking(self.handle.fileno(), msvcrt.LK_NBLCK, 1)
                        break
                    except OSError:
                        time.sleep(0.1)
            else:
                fcntl.flock(self.handle, fcntl.LOCK_EX)
        except OSError:
            pass
        return self

    def __exit__(self, *exc):
        try:
            if self.handle:
                if IS_WINDOWS:
                    self.handle.seek(0)
                    msvcrt.locking(self.handle.fileno(), msvcrt.LK_UNLCK, 1)
                self.handle.close()
        except OSError:
            pass
        return False


class StateStore:
    """sessao.json: tokens + pendência de refill + contadores do login.

    Mesmo formato do session_tokens.json da versão 1 e do cloud_state.json — os
    dois migram sem conversão.
    """

    def __init__(self, path=None):
        self.path = path or paths.STATE_FILE

    def load(self):
        return read_json(self.path, {})

    def update(self, patch):
        with FileLock(self.path.with_name(self.path.name + ".lk")):
            data = self.load()
            data.update(patch)
            data["saved_at"] = time.time()
            write_json(self.path, data, private=True)
            return data


# ----------------------------------------------------------------- credenciais

def load_credentials():
    """(número, senha) do ambiente ou do arquivo; (None, None) se não houver."""
    msisdn = os.environ.get("TASKER_MSISDN", "").strip()
    password = os.environ.get("TASKER_PASSWORD", "")
    if msisdn and password:
        return msisdn, password
    data = read_json(paths.CREDENTIALS_FILE, {})
    msisdn = (data.get("msisdn") or "").strip()
    password = data.get("password") or ""
    return (msisdn, password) if msisdn and password else (None, None)


def save_credentials(msisdn, password):
    write_json(paths.CREDENTIALS_FILE, {"msisdn": msisdn.strip(), "password": password},
               private=True)


def forget_credentials():
    try:
        paths.CREDENTIALS_FILE.unlink()
        return True
    except FileNotFoundError:
        return False


def has_credentials():
    return load_credentials()[0] is not None


# ---------------------------------------------------------- instância única

def acquire_instance_lock():
    """Handle do lock, ou None se outro processo já tem. Cai sozinho se ele morrer."""
    paths.ensure_data_dir()
    handle = open(paths.LOCK_FILE, "a+")   # "a" pra não truncar o PID de quem tem
    try:
        if IS_WINDOWS:
            handle.seek(LOCK_BYTE)
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        handle.close()
        return None
    handle.seek(0)
    handle.truncate()
    handle.write(f"{os.getpid()}\n")
    handle.flush()
    return handle


def instance_running():
    """(rodando?, pid). Testa o lock de verdade — arquivo esquecido não conta."""
    try:
        h = open(paths.LOCK_FILE, "a+")
    except OSError:
        return False, None
    try:
        try:
            if IS_WINDOWS:
                h.seek(LOCK_BYTE)
                msvcrt.locking(h.fileno(), msvcrt.LK_NBLCK, 1)
                msvcrt.locking(h.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(h, fcntl.LOCK_EX | fcntl.LOCK_NB)
                fcntl.flock(h, fcntl.LOCK_UN)
        except OSError:
            h.seek(0)
            try:
                pid = int((h.read(32) or "0").strip() or 0)
            except ValueError:
                pid = 0
            return True, pid or None
        return False, None
    finally:
        h.close()
