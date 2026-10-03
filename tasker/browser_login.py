"""Login pelo navegador: abre o Chrome/Edge do sistema e captura o token.

Pra quando o login por senha não serve (a Lidl pediu captcha/segundo fator) ou
você prefere não guardar senha. O navegador abre num perfil TEMPORÁRIO, você
loga no site normalmente, e o Tasker lê a sessão da API do sessionStorage
(onde o app do site a guarda) pelo protocolo DevTools. Assim que o token
aparece, o navegador fecha e o perfil é apagado — fechar logo importa: o app do
site, se ficasse aberto, renovaria o token por conta própria e derrubaria o
nosso (o refresh token rotaciona).

Sem Playwright e sem baixar Chromium: o protocolo DevTools é HTTP + WebSocket,
e o WebSocket mínimo que isso exige está aqui embaixo.
"""
import base64
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time
import urllib.request
from urllib.parse import urlparse

from .lidl import PORTAL_URL
from .system import find_browser
from .store import IS_WINDOWS

# A sessão vive em sessionStorage['portal-session'] (vuex-persistedstate). O
# caminho dentro do JSON muda entre builds do site, então procura o objeto que
# tem accessToken. Às vezes o valor vem JSON dentro de string.
JS_READ_TOKENS = r"""(() => {
  const raw = sessionStorage.getItem('portal-session');
  if (!raw) return null;
  let obj; try { obj = JSON.parse(raw); } catch (e) { return null; }
  if (typeof obj === 'string') { try { obj = JSON.parse(obj); } catch (e) { return null; } }
  const seen = new Set();
  const walk = (n) => {
    if (!n || typeof n !== 'object' || seen.has(n)) return null;
    seen.add(n);
    if (typeof n.accessToken === 'string' && n.accessToken) return n;
    for (const k of Object.keys(n)) { const h = walk(n[k]); if (h) return h; }
    return null;
  };
  const t = walk(obj);
  return t ? JSON.stringify({access: t.accessToken, refresh: t.refreshToken || null,
                             expiresAtTS: t.expiresAtTS || null}) : null;
})()"""


# ------------------------------------------------------------ WebSocket mínimo

class WebSocket:
    """Cliente RFC 6455 só com o necessário pro DevTools (texto, sem extensões).

    Não manda Origin: o Chrome recusa conexão DevTools com Origin desconhecido,
    e sem o cabeçalho ela é aceita.
    """

    def __init__(self, url, timeout=10):
        u = urlparse(url)
        self.sock = socket.create_connection((u.hostname, u.port or 80), timeout=timeout)
        key = base64.b64encode(os.urandom(16)).decode()
        path = u.path + (f"?{u.query}" if u.query else "")
        req = (f"GET {path} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\n"
               "Upgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
        self.sock.sendall(req.encode())
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = self.sock.recv(1024)
            if not chunk:
                raise ConnectionError("WebSocket fechou no aperto de mão")
            head += chunk
        if b" 101 " not in head.split(b"\r\n", 1)[0]:
            raise ConnectionError(head.split(b"\r\n", 1)[0].decode(errors="replace"))
        self._buf = head.split(b"\r\n\r\n", 1)[1]

    def _read(self, n):
        while len(self._buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("WebSocket fechou")
            self._buf += chunk
        out, self._buf = self._buf[:n], self._buf[n:]
        return out

    def _send_frame(self, opcode, payload):
        header = bytes([0x80 | opcode])
        n = len(payload)
        if n < 126:
            header += bytes([0x80 | n])
        elif n < 65536:
            header += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            header += bytes([0x80 | 127]) + struct.pack(">Q", n)
        mask = os.urandom(4)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(header + mask + masked)

    def send(self, text):
        self._send_frame(0x1, text.encode())

    def recv(self):
        message = b""
        while True:
            b1, b2 = self._read(2)
            opcode, n = b1 & 0x0F, b2 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._read(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._read(8))[0]
            mask = self._read(4) if b2 & 0x80 else None
            payload = self._read(n)
            if mask:
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
            if opcode == 0x8:
                raise ConnectionError("WebSocket fechado pelo navegador")
            if opcode == 0x9:
                self._send_frame(0xA, payload)   # ping -> pong
                continue
            if opcode == 0xA:
                continue
            message += payload
            if b1 & 0x80:
                return message.decode("utf-8", "replace")

    def close(self):
        try:
            self._send_frame(0x8, b"")
        except OSError:
            pass
        try:
            self.sock.close()
        except OSError:
            pass


def cdp_call(ws_url, method, params=None, timeout=10):
    ws = WebSocket(ws_url, timeout=timeout)
    try:
        ws.send(json.dumps({"id": 1, "method": method, "params": params or {}}))
        while True:
            msg = json.loads(ws.recv())
            if msg.get("id") == 1:
                return msg
    finally:
        ws.close()


def _http_json(url, timeout=5):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read().decode())


# ------------------------------------------------------------------- o login

class BrowserLogin:
    """Uma tentativa de login pelo navegador, em thread. `state` vira:
    'abrindo' -> 'aguardando' -> 'ok' | 'erro' | 'cancelado'."""

    TIMEOUT = 15 * 60
    extra_args = ()   # os testes usam ("--headless=new",)

    def __init__(self, log, on_tokens, url=PORTAL_URL):
        self.log = log
        self.on_tokens = on_tokens
        self.url = url
        self.state = "parado"
        self.message = ""
        self._cancel = threading.Event()
        self._thread = None
        self._port = None

    def info(self):
        return {"estado": self.state, "msg": self.message}

    def active(self):
        return bool(self._thread and self._thread.is_alive())

    def start(self):
        if self.active():
            return "O login pelo navegador já está aberto."
        exe = find_browser()
        if not exe:
            self.state, self.message = "erro", ("Não achei Chrome, Edge nem Chromium "
                                                "instalado. Use o login com número e senha.")
            return self.message
        self._cancel.clear()
        self.state, self.message = "abrindo", "Abrindo o navegador…"
        self._thread = threading.Thread(target=self._run, args=(exe,), daemon=True)
        self._thread.start()
        return "Abrindo o navegador: faça login na Lidl lá. Ele fecha sozinho quando der certo."

    def cancel(self):
        self._cancel.set()

    def _run(self, exe):
        profile = tempfile.mkdtemp(prefix="tasker-login-")
        proc = None
        self._port = None
        try:
            proc = subprocess.Popen(
                [exe, f"--user-data-dir={profile}", "--remote-debugging-port=0",
                 "--no-first-run", "--no-default-browser-check", "--disable-sync",
                 "--window-size=1100,900", *self.extra_args, "--new-window", self.url],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL)
            port = self._port = self._wait_port(profile, proc)
            if not port:
                raise RuntimeError("o navegador não abriu a porta de depuração")
            self.state, self.message = "aguardando", "Esperando você fazer login na janela…"
            self.log("Login pelo navegador: janela aberta, esperando o login.")
            tokens = self._wait_tokens(port, proc)
            if tokens:
                self.on_tokens(tokens)
                self.state, self.message = "ok", "Login capturado — o navegador foi fechado."
            elif self._cancel.is_set():
                self.state, self.message = "cancelado", "Login pelo navegador cancelado."
            elif proc.poll() is not None:
                self.state, self.message = "cancelado", "A janela foi fechada antes do login."
            else:
                self.state, self.message = "erro", "Passou de 15 min sem login — fechei a janela."
            self.log(f"Login pelo navegador: {self.message}")
        except Exception as e:
            self.state, self.message = "erro", f"Login pelo navegador falhou: {e}"
            self.log(self.message)
        finally:
            self._close(proc, profile)

    def _wait_port(self, profile, proc):
        marker = os.path.join(profile, "DevToolsActivePort")
        deadline = time.time() + 20
        while time.time() < deadline and not self._cancel.is_set():
            if proc.poll() is not None:
                return None
            try:
                with open(marker, encoding="utf-8") as f:
                    first = f.readline().strip()
                if first.isdigit():
                    return int(first)
            except OSError:
                pass
            time.sleep(0.2)
        return None

    def _wait_tokens(self, port, proc):
        deadline = time.time() + self.TIMEOUT
        host = urlparse(self.url).hostname or ""
        while time.time() < deadline and not self._cancel.is_set() and proc.poll() is None:
            try:
                pages = [t for t in _http_json(f"http://127.0.0.1:{port}/json/list")
                         if t.get("type") == "page" and t.get("webSocketDebuggerUrl")
                         and (host in t.get("url", "") or "lidl-connect.de" in t.get("url", ""))]
            except Exception:
                pages = []
            for page in pages:
                tokens = self._read_page(page["webSocketDebuggerUrl"])
                if tokens:
                    return tokens
            self._cancel.wait(1.5)
        return None

    @staticmethod
    def _read_page(ws_url):
        try:
            res = cdp_call(ws_url, "Runtime.evaluate",
                           {"expression": JS_READ_TOKENS, "returnByValue": True})
            value = ((res.get("result") or {}).get("result") or {}).get("value")
            data = json.loads(value) if value else None
        except Exception:
            return None
        if not data or not data.get("access"):
            return None
        exp_ms = data.get("expiresAtTS")
        if exp_ms and float(exp_ms) / 1000 < time.time() + 60:
            return None   # resto de sessão velha; espera o login de verdade
        return data

    def _close(self, proc, profile):
        if proc is not None and proc.poll() is None and self._port:
            # Fechar pelo DevTools derruba o navegador inteiro (no Windows o
            # terminate só mataria o processo principal).
            try:
                ver = _http_json(f"http://127.0.0.1:{self._port}/json/version")
                cdp_call(ver["webSocketDebuggerUrl"], "Browser.close", timeout=5)
            except Exception:
                pass
            try:
                proc.wait(5)
            except subprocess.TimeoutExpired:
                pass
        if proc is not None and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(8)
            except subprocess.TimeoutExpired:
                proc.kill()
                try:
                    proc.wait(5)
                except subprocess.TimeoutExpired:
                    pass
        # No Windows o perfil pode ficar preso por um instante depois do fim.
        for _ in range(10):
            shutil.rmtree(profile, ignore_errors=True)
            if not os.path.exists(profile):
                break
            time.sleep(0.5 if IS_WINDOWS else 0.2)
