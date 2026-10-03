"""Uma API da Lidl de mentira, pra testar offline.

Imita o que importa da de verdade: tokens JWT com validade, refresh token que
ROTACIONA (o usado morre), login por senha (com captcha opcional), a consulta
combinada `painel`, e a mutation de refill que devolve 500 genérico mesmo
quando a reserva entra — o volume só sobe algumas leituras depois.
"""
import base64
import http.server
import json
import secrets
import threading
import time


def make_jwt(exp):
    def b64(d):
        return base64.urlsafe_b64encode(json.dumps(d).encode()).decode().rstrip("=")
    return f"{b64({'alg': 'none'})}.{b64({'exp': exp, 'sub': 'teste'})}.assinatura"


class FakeLidl:
    def __init__(self):
        self.lock = threading.Lock()
        self.left = 1.0
        self.max = 1.0
        self.plan_left = 0
        self.cooldown = False
        self.token_ttl = 3600
        self.access = set()
        self.refresh = set()
        self.password = ("01761234567", "certa")
        self.captcha = False
        self.calls = []              # (operação, status)
        self.connections = 0
        self.fail_reads = []         # status a devolver nas próximas leituras
        self.refill_mode = "500"     # "500" | "success" | "noop" | "drop"
        self.rise_after_reads = 1    # leituras depois da mutation até o volume subir
        self._rise_in = None
        self.reject_combined = False
        self.omit_tariffs = False
        self.httpd = None

    # -- controle pelos testes
    def issue(self, ttl=None):
        a = make_jwt(time.time() + (self.token_ttl if ttl is None else ttl))
        r = "rt-" + secrets.token_hex(6)
        self.access.add(a)
        self.refresh.add(r)
        return a, r

    def count(self, op):
        return sum(1 for o, _ in self.calls if o == op)

    def start(self):
        fake = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"   # keep-alive, igual à API de verdade

            def log_message(self, *a):
                pass

            def setup(self):
                super().setup()
                with fake.lock:
                    fake.connections += 1

            def _send(self, status, obj):
                body = json.dumps(obj).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                req = json.loads(self.rfile.read(n) or b"{}")
                with fake.lock:
                    status, obj, op = fake.handle(self.path, req, self.headers)
                    fake.calls.append((op, status))
                if status is None:   # "drop": derruba a conexão sem responder
                    self.close_connection = True
                    self.connection.close()
                    return
                self._send(status, obj)

        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.httpd.daemon_threads = True
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()
        return f"http://127.0.0.1:{self.httpd.server_address[1]}"

    def stop(self):
        if self.httpd:
            self.httpd.shutdown()
            self.httpd.server_close()

    # -- a "API"
    def handle(self, path, req, headers):
        if path == "/api/token":
            return self._token(req)
        if path != "/api/graphql":
            return 404, {}, "404"
        auth = headers.get("authorization", "")
        token = auth[7:] if auth.lower().startswith("bearer ") else ""
        query = req.get("query", "")
        op = req.get("operationName")
        if token not in self.access or _exp(token) < time.time():
            return 401, {"message": "Unauthenticated"}, op
        if "bookTariffoptionsDirect" in query:
            return self._book(op)
        if self.fail_reads:
            st = self.fail_reads.pop(0)
            return st, {"errors": [{"message": "Ein Fehler ist aufgetreten"}]}, op
        if "consumptions" in query and "tariffoptions" in query and self.reject_combined:
            return 400, {"errors": [{"message": "validation"}]}, op
        data = {}
        if "consumptions" in query:
            self._tick_rise()
            data["consumptions"] = {"consumptionsForUnit": [
                {"type": "DATA", "left": self.plan_left, "max": 25, "unit": "GB"},
                {"type": "REFILLABLE_DATA", "left": round(self.left, 2), "max": self.max,
                 "unit": "GB"}]}
        if "tariffoptions" in query and not self.omit_tariffs:
            disabled = [{"name": "Refill", "tariffoptionId": "CCS_92061"}] if self.cooldown else []
            data["tariffoptions"] = {"disabledTariffoptions": {"disabledTariffoptions": disabled}}
        return 200, {"data": data}, op

    def _tick_rise(self):
        if self._rise_in is None:
            return
        self._rise_in -= 1
        if self._rise_in <= 0:
            self.left = self.max
            self._rise_in = None

    def _book(self, op):
        if self.refill_mode == "drop":
            self._rise_in = self.rise_after_reads
            return None, None, op
        if self.refill_mode == "success":
            self.left = self.max
            return 200, {"data": {"bookTariffoptionsDirect": {"success": True}}}, op
        if self.refill_mode == "500":
            self._rise_in = self.rise_after_reads
        return 500, {"errors": [{"message": "Ein Fehler ist aufgetreten. Bitte wende "
                                            "dich an die LIDL Connect Kundenbetreuung"}]}, op

    def _token(self, req):
        grant = req.get("grant_type")
        if grant == "refresh_token":
            rt = req.get("refresh_token")
            if rt not in self.refresh:
                return 400, {"error": "invalid_grant",
                             "message": "The refresh token is invalid."}, "refresh"
            self.refresh.discard(rt)   # rotaciona: o usado morre
            a, r = self.issue()
            return 200, {"access_token": a, "refresh_token": r,
                         "expires_in": self.token_ttl}, "refresh"
        if grant == "password":
            if self.captcha:
                return 400, {"error": "captcha_required",
                             "message": "captcha required"}, "password"
            if (req.get("username"), req.get("password")) != self.password:
                return 401, {"error": "invalid_credentials",
                             "message": "Benutzername oder Passwort falsch"}, "password"
            a, r = self.issue()
            return 200, {"access_token": a, "refresh_token": r,
                         "expires_in": self.token_ttl}, "password"
        return 400, {"error": "unsupported_grant_type"}, "token"


def _exp(token):
    try:
        part = token.split(".")[1]
        return json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))["exp"]
    except Exception:
        return 0
