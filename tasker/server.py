"""Servidor da tela: só em 127.0.0.1, com chave por execução.

Segurança (igual à versão 1): recusa Host que não seja local (DNS rebinding),
toda rota exige a chave e todo POST exige a chave no CABEÇALHO (site de fora
não consegue mandar cabeçalho próprio sem permissão de CORS, então não há
CSRF). A janela abre com um token de UM uso (`?boot=`): a URL vira argumento
do navegador, legível em /proc por outros usuários — com a chave de verdade
ali, qualquer um na máquina ganharia a tela.
"""
import http.server
import json
import mimetypes
import os
import secrets
import socket
import threading
import time
from urllib.parse import parse_qs, urlparse

from . import config, paths, store

STATIC = {"/app.js", "/style.css", "/icon.svg", "/icon.png"}


# ------------------------------------------------------------------ históricos

def _jsonl_tail(path, max_bytes=2 * 1024 * 1024):
    try:
        with open(path, "rb") as f:
            f.seek(0, 2)
            start = max(0, f.tell() - max_bytes)
            f.seek(start)
            raw = f.read().decode("utf-8", "replace")
    except OSError:
        return []
    lines = raw.splitlines()[1:] if start else raw.splitlines()
    out = []
    for line in lines:
        try:
            d = json.loads(line)
        except ValueError:
            continue
        if isinstance(d, dict):
            out.append(d)
    return out


def history(hours=24):
    hours = max(1, min(int(hours), 7 * 24))
    cut = time.time() - hours * 3600
    vol = [[d["t"], d["gb"]] for d in _jsonl_tail(paths.VOLUME_FILE)
           if isinstance(d.get("t"), (int, float)) and isinstance(d.get("gb"), (int, float))
           and d["t"] >= cut]
    # Muitas amostras (checagem a cada 15 s numa semana) pesam no navegador e não
    # mudam o desenho: afina pra ~1500 pontos mantendo os vales (o que importa).
    if len(vol) > 1500:
        step = len(vol) / 1500
        thin, i = [], 0.0
        while int(i) < len(vol):
            chunk = vol[int(i):int(i + step) or int(i) + 1]
            thin.append(min(chunk, key=lambda p: p[1]))
            i += step
        vol = thin
    events = [d for d in _jsonl_tail(paths.REFILLS_FILE, 512 * 1024)
              if d.get("kind") in ("refill_ok", "refill_fail")]
    events.sort(key=lambda d: d.get("t") or 0)
    in_range = [e for e in events if (e.get("t") or 0) >= cut]
    midnight = time.mktime(time.localtime()[:3] + (0, 0, 0, 0, 0, -1))
    oks_today = [e for e in events if e["kind"] == "refill_ok" and (e.get("t") or 0) >= midnight]
    lat = [e["latency"] for e in in_range
           if e["kind"] == "refill_ok" and isinstance(e.get("latency"), (int, float))]
    return {
        "volume": vol,
        "refills": [{"t": e.get("t"), "ok": e["kind"] == "refill_ok",
                     "before": e.get("before"), "after": e.get("after"),
                     "why": e.get("why") or "", "latencia": e.get("latency")}
                    for e in in_range][-200:],
        "resumo": {
            "refills_hoje": len(oks_today),
            "refills_periodo": sum(1 for e in in_range if e["kind"] == "refill_ok"),
            "falhas_periodo": sum(1 for e in in_range if e["kind"] == "refill_fail"),
            "latencia_media": round(sum(lat) / len(lat), 1) if lat else None,
            "horas": hours,
        },
    }


def settings_payload():
    values, sources = config.load_with_sources()
    fields = []
    for spec in config.SETTINGS:
        num = spec["type"] in ("int", "float")
        fields.append({
            "key": spec["key"], "label": spec["label"], "help": spec["help"],
            "group": spec["group"], "type": spec["type"], "env": spec["env"],
            "unit_label": config.UNIT_LABEL.get(spec.get("unit", ""), ""),
            "origem": sources[spec["key"]],
            "valor": config.to_display(spec, values[spec["key"]]),
            "padrao": config.to_display(spec, spec["default"]),
            "min": config.to_display(spec, spec["min"]) if num else None,
            "max": config.to_display(spec, spec["max"]) if num else None,
        })
    return {"grupos": config.GROUPS, "campos": fields, "avisos": config.warnings(values)}


def save_settings(sent):
    keep, errors = {}, {}
    for spec in config.SETTINGS:
        if spec["key"] in sent:
            try:
                keep[spec["key"]] = config.from_display(spec, sent[spec["key"]])
            except ValueError as e:
                errors[spec["key"]] = str(e)
    if errors:
        return False, "Tem campo fora do limite — nada foi gravado.", errors
    final = {k: v for k, v in config.read_file().items() if k in config.BY_KEY}
    final.update(keep)
    try:
        config.save(final)
    except (OSError, ValueError) as e:
        return False, f"Não consegui gravar: {e}", {}
    return True, "Ajustes gravados e já valendo.", {}


# --------------------------------------------------------------------- servidor

class UiServer:
    def __init__(self, app, port=8765):
        self.app = app
        self.key = secrets.token_urlsafe(24)
        self._boots = set()
        self._boot_lock = threading.Lock()
        self.port = free_port(port)
        self.httpd = _HTTPServer(("127.0.0.1", self.port), _Handler)
        self.httpd.ui = self

    def new_boot(self):
        token = secrets.token_urlsafe(16)
        with self._boot_lock:
            self._boots = {t for t in self._boots if t[1] > time.time()}
            self._boots.add((token, time.time() + 120))
        return token

    def use_boot(self, token):
        with self._boot_lock:
            for item in list(self._boots):
                if item[1] > time.time() and secrets.compare_digest(item[0], token):
                    self._boots.discard(item)
                    return True
        return False

    def boot_url(self):
        return f"http://127.0.0.1:{self.port}/?boot={self.new_boot()}"

    def publish(self):
        """dados/tela.json: como um segundo clique no atalho acha esta tela."""
        store.write_json(paths.UI_FILE, {"port": self.port, "key": self.key,
                                         "pid": os.getpid()}, private=True)

    def unpublish(self):
        try:
            paths.UI_FILE.unlink()
        except OSError:
            pass

    def serve(self):
        self.httpd.serve_forever(poll_interval=0.5)

    def shutdown(self):
        threading.Thread(target=self.httpd.shutdown, daemon=True).start()


class _HTTPServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = False


def free_port(first):
    for port in range(first, first + 30):
        with socket.socket() as s:
            try:
                s.bind(("127.0.0.1", port))
                return port
            except OSError:
                continue
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class _Handler(http.server.BaseHTTPRequestHandler):
    server_version = "Tasker"

    def log_message(self, *a):
        pass

    @property
    def ui(self):
        return self.server.ui

    def _local(self):
        host = (self.headers.get("Host") or "").rsplit(":", 1)[0].strip("[]")
        return host in ("127.0.0.1", "localhost", "::1", "")

    def _send(self, code, body, ctype, extra=None):
        data = body if isinstance(body, bytes) else body.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def _json(self, obj, code=200):
        self._send(code, json.dumps(obj, ensure_ascii=False), "application/json; charset=utf-8")

    def _text(self, msg, code):
        self._send(code, msg, "text/plain; charset=utf-8")

    def do_GET(self):
        if not self._local():
            return self._text("Só aceito conexão local.", 403)
        url = urlparse(self.path)
        q = parse_qs(url.query)
        if url.path == "/" and q.get("boot"):
            if self.ui.use_boot(q["boot"][0]):
                # A chave vai no fragmento (#): fica só no navegador — nunca volta
                # ao servidor, não entra em log nem em Referer, e o F5 continua
                # funcionando.
                self.send_response(302)
                self.send_header("Location", f"/#k={self.ui.key}")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return None
            return self._text("Este link de abertura já foi usado. Abra o Tasker pelo "
                              "atalho de novo.", 403)
        if url.path == "/":
            page = (paths.UI_DIR / "index.html").read_bytes()
            return self._send(200, page, "text/html; charset=utf-8",
                              {"Content-Security-Policy":
                               "default-src 'self'; img-src 'self' data:; "
                               "style-src 'self' 'unsafe-inline'; frame-ancestors 'none'"})
        if url.path in STATIC:
            f = paths.UI_DIR / url.path.lstrip("/")
            ctype = mimetypes.guess_type(f.name)[0] or "application/octet-stream"
            if f.suffix == ".js":
                ctype = "text/javascript"
            try:
                return self._send(200, f.read_bytes(), ctype + ("; charset=utf-8"
                                  if ctype.startswith("text") else ""))
            except OSError:
                return self._text("Não existe.", 404)
        if url.path == "/favicon.ico":
            return self._send(200, (paths.UI_DIR / "icon.png").read_bytes(), "image/png")
        if not secrets.compare_digest(self.headers.get("X-Tasker-Key", ""), self.ui.key):
            return self._text("Chave errada.", 403)
        app = self.ui.app
        if url.path == "/api/estado":
            return self._json(app.state())
        if url.path == "/api/ajustes":
            return self._json(settings_payload())
        if url.path == "/api/historico":
            try:
                hours = int(q.get("horas", ["24"])[0])
            except ValueError:
                hours = 24
            return self._json(history(hours))
        if url.path == "/api/log":
            try:
                since = int(q.get("desde", ["0"])[0])
            except ValueError:
                since = 0
            return self._json(app.log.since(since))
        return self._text("Não existe.", 404)

    def do_POST(self):
        if not self._local():
            return self._json({"erro": "Só aceito conexão local."}, 403)
        if not secrets.compare_digest(self.headers.get("X-Tasker-Key", ""), self.ui.key):
            return self._json({"erro": "Chave errada."}, 403)
        try:
            size = min(int(self.headers.get("Content-Length") or 0), 64 * 1024)
            body = json.loads(self.rfile.read(size) or b"{}")
            if not isinstance(body, dict):
                raise ValueError
        except (ValueError, OSError):
            return self._json({"erro": "Corpo inválido."}, 400)
        path = urlparse(self.path).path
        if path == "/api/boot":
            return self._json({"boot": self.ui.new_boot()})
        if path == "/api/ajustes":
            ok, msg, errors = save_settings(body.get("valores") or {})
            if ok:
                self.ui.app.monitor.reload_config()
            return self._json({"ok": ok, "msg": msg, "erros": errors})
        if path == "/api/acao":
            return self._json(self.ui.app.action(body))
        return self._json({"erro": "Não existe."}, 404)
