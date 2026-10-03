"""Cliente HTTP da API da Lidl Connect — só biblioteca padrão.

Medido contra a API de verdade (out/2026): com a conexão HTTPS mantida aberta, a
primeira chamada leva ~1 s (aperto de mão TLS) e as seguintes ~130 ms. Por isso
a conexão é reaproveitada entre checagens, e volume + cooldown vêm numa query só
(`painel`): quando o refill precisa sair, a mutation vai na hora, sem mais uma
ida e volta pra conferir o cooldown.

Três coisas que não são adivinháveis:
  * a mutation de refill registra a reserva e AINDA devolve HTTP 500 genérico
    (o botão do site toma o mesmo 500) — ela nunca é repetida às cegas;
  * o refresh token ROTACIONA: cada renovação mata o anterior;
  * a conta aceita uma sessão por vez.
"""
import base64
import http.client
import json
import os
import secrets
import socket
import ssl
import time
from urllib.parse import urlparse

API_ORIGIN = "https://api.lidl-connect.de"
PORTAL_ORIGIN = "https://kundenkonto.lidl-connect.de"
PORTAL_URL = f"{PORTAL_ORIGIN}/mein-lidl-connect/uebersicht.html"
# Credenciais públicas do cliente OAuth do portal (estão no bundle JS do site).
CLIENT_ID = "lidl"
CLIENT_SECRET = "lidl"
REFILL_OPTION_ID = "CCS_92061"   # tariffoption do botão "Refill aktivieren"

USER_AGENT = ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
              "Chrome/148.0.0.0 Safari/537.36")

_UNITS = "consumptionsForUnit { type left max unit __typename }"
_DISABLED = "disabledTariffoptions { disabledTariffoptions { name tariffoptionId } }"
PAINEL_QUERY = (f"query painel {{ consumptions {{ {_UNITS} __typename }} "
                f"tariffoptions {{ {_DISABLED} }} }}")
CONSUMPTIONS_QUERY = f"query consumptions {{ consumptions {{ {_UNITS} __typename }} }}"
TARIFFOPTIONS_QUERY = f"query tariffOptions {{ tariffoptions {{ {_DISABLED} }} }}"
# Idêntica à do app (só `success`): campo a mais é risco de erro sem ganho.
REFILL_MUTATION = (
    "mutation bookTariffOptionsDirect"
    "($bookTariffoptionsDirectInput: BookTariffoptionsDirectInput!) "
    "{ bookTariffoptionsDirect(bookTariffoptionsDirectInput: "
    "$bookTariffoptionsDirectInput) { success __typename } }"
)
REFILL_VARIABLES = {"bookTariffoptionsDirectInput": {
    "bookTariffoptions": [{"tariffoptionId": REFILL_OPTION_ID}]}}

# Erros de conexão "velha" (o servidor fechou a keep-alive enquanto dormíamos).
_STALE = (http.client.RemoteDisconnected, http.client.CannotSendRequest,
          http.client.ResponseNotReady, ConnectionResetError, BrokenPipeError,
          ConnectionAbortedError, ssl.SSLEOFError)


def jwt_exp(token):
    """Validade lida do próprio JWT (não valida assinatura). 0.0 se não der."""
    if not token or token.count(".") != 2:
        return 0.0
    try:
        part = token.split(".")[1]
        raw = base64.urlsafe_b64decode(part + "=" * (-len(part) % 4))
        return float(json.loads(raw).get("exp") or 0)
    except Exception:
        return 0.0


def _js_hash(text):
    """O hash do bundle do site (vai no header X-Transaction)."""
    acc = 0
    for ch in text:
        acc = (acc * 31 + ord(ch)) & 0xFFFFFFFF
        if acc >= 0x80000000:
            acc -= 0x100000000
    return acc + 2147483647


def x_transaction(label, access_token):
    ident = _js_hash(access_token) if access_token else "anonymous"
    return f"{label}-{secrets.token_hex(4)}-{ident}"


class Response:
    def __init__(self, status, text, ms=0):
        self.status = status   # -1 = nem houve resposta (rede, timeout, DNS)
        self.text = text or ""
        self.ms = ms

    def json(self):
        try:
            return json.loads(self.text)
        except ValueError:
            return None

    def snippet(self, n=160):
        return self.text[:n].replace("\n", " ")


class LidlApi:
    def __init__(self, origin=None, timeout=20):
        u = urlparse(origin or os.environ.get("TASKER_API_ORIGIN") or API_ORIGIN)
        self.https = u.scheme == "https"
        self.host = u.hostname
        self.port = u.port
        self.timeout = timeout
        self._conn = None
        self._last_use = 0.0

    def close(self):
        if self._conn is not None:
            try:
                self._conn.close()
            except Exception:
                pass
        self._conn = None

    def _connection(self):
        if self._conn is None:
            if self.https:
                self._conn = http.client.HTTPSConnection(
                    self.host, self.port, timeout=self.timeout,
                    context=ssl.create_default_context())
            else:
                self._conn = http.client.HTTPConnection(self.host, self.port,
                                                        timeout=self.timeout)
        return self._conn

    def post(self, path, payload, headers=None, idempotent=True):
        """POST JSON. Leitura repete uma vez se a conexão estava velha.

        A mutation (idempotent=False) nunca repete: se a conexão está parada há
        mais de alguns segundos ela é refeita ANTES de enviar, e qualquer falha
        depois de enviar vira "não sei" pra quem chamou decidir pelo efeito.
        """
        body = json.dumps(payload).encode()
        hdrs = {
            "content-type": "application/json",
            "accept": "application/json, text/plain, */*",
            "accept-language": "de-DE",
            "origin": PORTAL_ORIGIN,
            "referer": PORTAL_ORIGIN + "/",
            "user-agent": USER_AGENT,
        }
        hdrs.update(headers or {})
        if not idempotent and time.time() - self._last_use > 5:
            self.close()
        attempts = 2 if idempotent else 1
        for attempt in range(attempts):
            started = time.time()
            try:
                conn = self._connection()
                conn.request("POST", path, body, hdrs)
                resp = conn.getresponse()
                text = resp.read().decode("utf-8", "replace")
                self._last_use = time.time()
                if (resp.getheader("connection") or "").lower() == "close":
                    self.close()
                return Response(resp.status, text, int((time.time() - started) * 1000))
            except _STALE as e:
                self.close()
                if attempt + 1 < attempts:
                    continue
                return Response(-1, f"{type(e).__name__}: {e}")
            except (OSError, socket.timeout, http.client.HTTPException) as e:
                self.close()
                return Response(-1, f"{type(e).__name__}: {e}")
        return Response(-1, "sem resposta")

    # -- token ---------------------------------------------------------------

    def refresh(self, refresh_token, access_token=None):
        return self.post("/api/token", {
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "refresh_token": refresh_token,
        }, {"X-Transaction": x_transaction("refresh", access_token)})

    def password_login(self, msisdn, password):
        # Mesmo corpo do site quando não há captcha.
        return self.post("/api/token", {
            "grant_type": "password",
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "username": msisdn,
            "password": password,
            "captcha_code": None,
            "captcha_token": None,
        }, {"X-Transaction": x_transaction("login", None)}, idempotent=False)

    # -- GraphQL -------------------------------------------------------------

    def graphql(self, operation, query, token, variables=None, mutation=False):
        """O app manda X-Transaction nas queries mas NÃO na mutation — igual aqui."""
        headers = {"authorization": f"Bearer {token}", "accept": "*/*"}
        if not mutation:
            headers["X-Transaction"] = x_transaction(operation, token)
        return self.post("/api/graphql", {"operationName": operation,
                                          "variables": variables or {},
                                          "query": query},
                         headers, idempotent=not mutation)

    def painel(self, token):
        return self.graphql("painel", PAINEL_QUERY, token)

    def book_refill(self, token):
        return self.graphql("bookTariffOptionsDirect", REFILL_MUTATION, token,
                            REFILL_VARIABLES, mutation=True)


def parse_painel(data):
    """data do GraphQL -> (unidades, cooldown).

    unidades = {"refill": {...}, "data": {...}} ou None se a parte de consumo não
    veio. cooldown = True/False, ou None se a parte de tarifas não veio (não
    saber o cooldown NUNCA pode virar "pode reservar").
    """
    if not isinstance(data, dict):
        return None, None
    units = None
    try:
        units = {}
        for unit in data["consumptions"]["consumptionsForUnit"] or []:
            if unit.get("type") == "REFILLABLE_DATA":
                units["refill"] = unit
            elif unit.get("type") == "DATA":
                units["data"] = unit
    except (KeyError, TypeError, AttributeError):
        units = None
    cooldown = None
    try:
        disabled = data["tariffoptions"]["disabledTariffoptions"]["disabledTariffoptions"]
        cooldown = any(o.get("tariffoptionId") == REFILL_OPTION_ID for o in disabled or [])
    except (KeyError, TypeError, AttributeError):
        cooldown = None
    return units, cooldown


def as_float(value):
    try:
        return float(value)
    except (TypeError, ValueError):
        return None
