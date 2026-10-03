#!/usr/bin/env python3
"""Testes do site/tasker.php — offline, contra um servidor que imita a API da Lidl.

    python3 tests/test_php.py        (precisa do php-cli)

Roda o tasker.php como CGI de verdade (php -f com QUERY_STRING), apontando as URLs
da API pro servidor falso. Cobre a porta de entrada por chave, a rotação do refresh
token, o login por senha com as travas, e o fluxo de refill em duas etapas
(reserva agora, confirmação na chamada seguinte).
"""
import base64
import json
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

SITE = Path(__file__).resolve().parent   # a própria pasta do site
PORT = 8733
KEY = "chave-de-teste-bem-comprida-123"

if not shutil.which("php"):
    print("php-cli não encontrado — instale com: sudo apt install php-cli")
    raise SystemExit(0)

ok = fail = 0


def check(label, got, want):
    global ok, fail
    if got == want:
        ok += 1
        print(f"  ok   {label}")
    else:
        fail += 1
        print(f"  FALHOU {label}: got={got!r} want={want!r}")


def contains(label, haystack, needle):
    check(label, needle.lower() in (haystack or "").lower(), True)


def jwt(seconds_from_now):
    payload = base64.urlsafe_b64encode(
        json.dumps({"exp": time.time() + seconds_from_now}).encode()).decode().rstrip("=")
    return f"hdr.{payload}.sig"


# ------------------------------------------------------- servidor da API falsa

class Fake:
    def __init__(self):
        self.reset()

    def reset(self):
        self.refresh_valid = {"rt-1"}
        self.rotation = iter([f"rt-{i}" for i in range(2, 40)])
        self.refill_left = 0.4
        self.refill_max = 1.0
        self.cooldown = False
        self.book_status = 200
        self.book_effect = True
        self.requests = []
        self.login_ok = ("1525", "boa")
        self.login_error = "senha errada"
        self.login_extra = {}
        self.ntfy = []


F = Fake()


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj, raw=False):
        body = obj if raw else json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(n).decode()
        if self.path.startswith("/ntfy"):
            F.ntfy.append(raw)
            return self._send(200, {"ok": True})
        payload = json.loads(raw or "{}")
        F.requests.append((self.path, payload))

        if self.path == "/api/token":
            if payload.get("grant_type") == "password":
                user, pwd = F.login_ok
                if payload.get("username") != user or payload.get("password") != pwd:
                    return self._send(401, {"message": F.login_error})
                novo = next(F.rotation)
                F.refresh_valid = {novo}
                resp = {"access_token": jwt(3600), "refresh_token": novo,
                        "expires_in": 3600}
                resp.update(F.login_extra)
                return self._send(200, resp)
            if payload.get("refresh_token") not in F.refresh_valid:
                return self._send(400, {"error": "invalid_grant",
                                        "hint": "Token has been revoked"})
            novo = next(F.rotation)
            F.refresh_valid = {novo}
            return self._send(200, {"access_token": jwt(3600), "refresh_token": novo,
                                    "expires_in": 3600, "token_type": "Bearer"})

        if self.path == "/api/graphql":
            if not (self.headers.get("authorization") or "").startswith("Bearer "):
                return self._send(401, {"message": "sem token"})
            op = payload.get("operationName")
            if op == "consumptions":
                return self._send(200, {"data": {"consumptions": {"consumptionsForUnit": [
                    {"type": "DATA", "left": 0, "max": 25, "unit": "GB"},
                    {"type": "REFILLABLE_DATA", "left": F.refill_left,
                     "max": F.refill_max, "unit": "GB"}]}}})
            if op == "tariffOptions":
                disabled = ([{"name": "Refill", "tariffoptionId": "CCS_92061"}]
                            if F.cooldown else [])
                return self._send(200, {"data": {"tariffoptions": {
                    "disabledTariffoptions": {"disabledTariffoptions": disabled}}}})
            if op == "bookTariffOptionsDirect":
                if F.book_effect:
                    F.refill_left = F.refill_max
                    F.cooldown = True
                if F.book_status == 500:
                    return self._send(500, {"errors": [{"message": "Ein Fehler"}]})
                return self._send(200, {"data": {"bookTariffoptionsDirect":
                                                 {"success": True}}})
        self._send(404, {})


srv = HTTPServer(("127.0.0.1", PORT), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()

# ------------------------------------------------- cópia do site apontada pro fake

TMP = Path(tempfile.mkdtemp(prefix="tasker-php-"))
WEB = TMP / "htdocs"
WEB.mkdir(parents=True)

src = (SITE / "tasker.php").read_text(encoding="utf-8")
src = src.replace("const API_ORIGIN   = 'https://api.lidl-connect.de';",
                  f"const API_ORIGIN   = 'http://127.0.0.1:{PORT}';")
(WEB / "tasker.php").write_text(src, encoding="utf-8")


def write_config(**kw):
    cfg = {"key": KEY, "refresh_token": "", "msisdn": "", "password": "",
           "gb_threshold": 0.85, "max_without_refill": 780,
           "ntfy_topic": f"http://127.0.0.1:{PORT}/ntfy", "verbose": True}
    cfg.update(kw)
    items = ",\n".join(
        f"    {k!r} => " + (json.dumps(v) if not isinstance(v, str) else f"'{v}'")
        for k, v in cfg.items())
    (WEB / "tasker-config.php").write_text(
        "<?php\nreturn [\n" + items.replace("'", "'") + "\n];\n", encoding="utf-8")


def state_path():
    hits = list((TMP / "tasker-data").glob("state*.json"))
    hits += list((WEB / "tasker-data").glob("state*.json"))
    return hits[0] if hits else None


def read_state():
    p = state_path()
    return json.loads(p.read_text()) if p else {}


def write_state(data):
    p = state_path()
    if p is None:
        (TMP / "tasker-data").mkdir(exist_ok=True)
        p = TMP / "tasker-data" / "state.json"
    p.write_text(json.dumps(data))


# Servidor embutido do PHP: a CLI (`php -f`) não popula $_GET a partir de
# QUERY_STRING, então testar por HTTP é o único jeito de exercitar o caminho real.
# Porta LIVRE, não fixa: uma rodada interrompida deixa um `php -S` zumbi de pé, e
# com porta fixa a rodada seguinte conversaria com o zumbi (docroot morto,
# respostas vazias) — aconteceu de verdade. O atexit mata o servidor até quando
# uma asserção estoura no meio.
import atexit
import socket


def _porta_livre(inicial=8734):
    for porta in range(inicial, inicial + 40):
        with socket.socket() as s:
            try:
                s.bind(("127.0.0.1", porta))
                return porta
            except OSError:
                continue
    raise SystemExit("nenhuma porta livre pro servidor PHP de teste")


WEB_PORT = _porta_livre()
php_srv = subprocess.Popen(
    ["php", "-S", f"127.0.0.1:{WEB_PORT}", "-t", str(WEB)],
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
atexit.register(php_srv.kill)
for _ in range(50):
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{WEB_PORT}/", timeout=1)
        break
    except urllib.error.HTTPError:
        break
    except Exception:
        time.sleep(0.1)


def run(key=KEY):
    """Chama o tasker.php por HTTP, como o web-cron faria."""
    url = f"http://127.0.0.1:{WEB_PORT}/tasker.php?key={urllib.parse.quote(key)}"
    try:
        with urllib.request.urlopen(url, timeout=90) as r:
            return r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.read().decode("utf-8", "replace")


# --------------------------------------------------------------------- testes

print("porta de entrada (chave)")
write_config(refresh_token="rt-1")
F.reset()
body = run(key="errada")
contains("chave errada -> 404", body, "Not Found")
check("não chamou a API", F.requests, [])
body = run(key="")
contains("sem chave -> 404", body, "Not Found")

write_config(key="curta", refresh_token="rt-1")
contains("chave curta é recusada na config", run(key="curta"), "pelo menos 16")
write_config(refresh_token="rt-1")

print("primeiro ciclo (semente + rotação + refill)")
F.reset()
body = run()
contains("usou a semente", body, "semente")
contains("renovou o token", body, "access token renovado")
contains("leu o volume", body, "refill disponível: 0.40/1 GB")
contains("plano no log", body, "plano: 0 GB")
contains("primeira execução força refill", body, "primeira execução")
contains("terminou ok", body, "OK")
st = read_state()
check("rotacionou e gravou", st.get("refresh"), "rt-2")
check("guardou o anterior", st.get("refresh_prev"), "rt-1")
check("access gravado", bool(st.get("access")), True)
check("marcou o refill", st.get("last_refill", 0) > time.time() - 60, True)
check("estado fora do htdocs", str(state_path()).startswith(str(TMP / "tasker-data")), True)

print("segundo ciclo (volume ok e refill recente: não faz nada)")
body = run()
contains("nada a fazer agora", body, "sem refill agora")
check("não reservou de novo",
      len([r for r in F.requests if r[1].get("operationName")
           == "bookTariffOptionsDirect"]), 1)

print("balde cheio com timeout vencido: não reserva, só reseta o timer")
st = read_state()
st["last_refill"] = time.time() - 3600      # timeout vencido
write_state(st)
F.refill_left = F.refill_max                # e o balde está cheio
body = run()
contains("percebeu que está cheio", body, "cheio")
check("mesmo forçado, não reservou",
      len([r for r in F.requests if r[1].get("operationName")
           == "bookTariffOptionsDirect"]), 1)
check("resetou o timer", read_state().get("last_refill", 0) > time.time() - 60, True)

print("refill em duas etapas (o 500 mentiroso da API)")
F.reset()
F.book_status = 500          # a API erra...
F.book_effect = True         # ...mas a reserva entra
write_state({"refresh": "rt-1", "last_refill": time.time() - 3600})
body = run()
contains("500 tratado como normal", body, "500 genérico")
contains("marcou pendente", body, "pendente")
st = read_state()
check("pendência gravada", "pending_refill" in st, True)
check("não marcou last_refill ainda", st.get("last_refill", 0) < time.time() - 600, True)
# a chamada seguinte confirma pelo efeito
body = run()
contains("confirmou na chamada seguinte", body, "reserva anterior")
st = read_state()
check("pendência resolvida", "pending_refill" in st, False)
check("agora marcou o refill", st.get("last_refill", 0) > time.time() - 60, True)
check("zerou falhas", st.get("refill_fails"), 0)

print("reserva que realmente falhou")
F.reset()
F.book_status = 500
F.book_effect = False        # nem entra
write_state({"refresh": "rt-1", "last_refill": time.time() - 3600})
run()
check("pendência criada", "pending_refill" in read_state(), True)
st = read_state()
st["pending_refill"]["at"] -= 300      # envelhece a pendência
write_state(st)
body = run()
contains("concluiu que não entrou", body, "não surtiu efeito")
check("contou falha", read_state().get("refill_fails"), 1)

print("cooldown")
F.reset()
F.cooldown = True
write_state({"refresh": "rt-1", "last_refill": time.time() - 3600})
body = run()
contains("respeitou o cooldown", body, "cooldown")
check("não reservou", [r for r in F.requests
                       if r[1].get("operationName") == "bookTariffOptionsDirect"], [])

print("token revogado -> login por senha")
F.reset()
write_config(refresh_token="", msisdn="1525", password="boa")
write_state({"refresh": "morto", "last_refill": time.time()})
body = run()
contains("tentou a senha", body, "login por senha")
contains("logou", body, "login por senha OK")
st = read_state()
check("guardou token novo", bool(st.get("access")), True)
check("zerou a trava", st.get("login_fails"), 0)

print("senha errada: backoff e proteção da conta")
F.reset()
write_config(refresh_token="", msisdn="1525", password="errada")
write_state({"refresh": "morto", "last_refill": time.time()})
body = run()
contains("falhou", body, "login por senha falhou")
st = read_state()
check("1 falha", st.get("login_fails"), 1)
d = st.get("login_blocked_until", 0) - time.time()
check("bloqueou ~5 min", 280 < d < 320, True)
body = run()
contains("dentro do bloqueio não tenta", body, "em espera")
check("não somou falha", read_state().get("login_fails"), 1)
st = read_state(); st["login_blocked_until"] = 0; write_state(st)
run()
check("2 falhas", read_state().get("login_fails"), 2)
check("backoff dobrou",
      580 < read_state().get("login_blocked_until", 0) - time.time() < 620, True)
st = read_state(); st["login_blocked_until"] = 0; write_state(st)
run()
check("3 falhas = limite", read_state().get("login_fails"), 3)
check("segurou por 6 h",
      read_state().get("login_blocked_until", 0) - time.time() > 5 * 3600, True)
check("avisou pelo ntfy", any("falhou 3 vezes" in n for n in F.ntfy), True)

print("captcha e segundo fator: para de tentar")
F.reset()
F.login_error = "Bitte Captcha eingeben"
write_config(refresh_token="", msisdn="1525", password="errada")
write_state({"refresh": "morto", "last_refill": time.time()})
body = run()
contains("detectou captcha", body, "bloqueado")
st = read_state()
check("não contou como falha comum", st.get("login_fails"), 0)
check("bloqueio longo", st.get("login_blocked_until", 0) - time.time() > 5 * 3600, True)

F.reset()
F.login_extra = {"needs_multifactor_authentication": True, "mfa_authenticated": False}
write_config(refresh_token="", msisdn="1525", password="boa")
write_state({"refresh": "morto", "last_refill": time.time()})
body = run()
contains("detectou segundo fator", body, "segundo fator")
check("bloqueio longo por MFA",
      read_state().get("login_blocked_until", 0) - time.time() > 5 * 3600, True)

print("sem credencial nenhuma")
F.reset()
write_config(refresh_token="", msisdn="", password="")
p = state_path()
if p:
    p.unlink()
contains("avisa que falta configurar", run(), "sem credencial")

print("proteção do diretório de estado dentro do htdocs")
# força o caso "só dá pra gravar dentro do htdocs"
F.reset()
write_config(refresh_token="rt-1", state_dir=str(WEB / "tasker-data"))
shutil.rmtree(TMP / "tasker-data", ignore_errors=True)
run()
ht = WEB / "tasker-data" / ".htaccess"
check("criou .htaccess", ht.is_file(), True)
contains("nega acesso", ht.read_text(), "denied")
check("index.html vazio criado", (WEB / "tasker-data" / "index.html").is_file(), True)
inside = list((WEB / "tasker-data").glob("state-*.json"))
check("nome do estado é imprevisível", len(inside) == 1 and len(inside[0].name) > 30, True)

print("proteção do diretório de estado (sempre, esteja onde estiver)")
sp = state_path()
check("nome do estado é derivado da chave, não 'state.json'",
      sp is not None and sp.name != "state.json" and sp.name.startswith("state-"),
      True)
check(".htaccess de negação está lá", (sp.parent / ".htaccess").exists(), True)
contains_ht = (sp.parent / ".htaccess").read_text()
check("com Require all denied", "Require all denied" in contains_ht, True)
check("e index.html anti-listagem", (sp.parent / "index.html").exists(), True)
# migração: um state.json antigo é renomeado pro nome disfarçado
velho_arq = sp.parent / "state.json"
novo_nome = sp
conteudo = novo_nome.read_text()
novo_nome.rename(velho_arq)                 # simula instalação antiga
body = run()
check("state.json antigo migra pro nome novo", novo_nome.exists(), True)
check("sem perder o conteúdo (refresh preservado)",
      json.loads(novo_nome.read_text()).get("refresh") is not None, True)

srv.shutdown()
php_srv.terminate()
php_srv.wait(timeout=10)
shutil.rmtree(TMP, ignore_errors=True)
print(f"\n{ok} ok, {fail} falha(s)")
raise SystemExit(1 if fail else 0)
