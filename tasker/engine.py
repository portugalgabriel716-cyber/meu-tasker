"""Motor: lê o volume e dispara o refill, só com HTTP.

Regras que valem desde a versão 1 e continuam aqui:
  * a mutation de refill NUNCA é repetida às cegas: a resposta (500 genérico)
    não prova nada, então a reserva vira "pendente" — persistida em disco, pra
    sobreviver a crash — e quem decide é o efeito (volume subiu ou a opção
    entrou em cooldown);
  * cooldown que não deu pra ler adia a reserva em vez de reservar às cegas;
  * o refresh token rotaciona: o novo é gravado ANTES de qualquer outra coisa;
  * login por senha tem trava persistida (errar senha trava a conta na Lidl).

Novidades da versão 2:
  * volume e cooldown vêm numa query só, pela mesma conexão HTTPS — o refill sai
    ~1-2 s mais cedo do que pelo navegador;
  * depois de pedir um refill, confirma em poucos segundos (intervalo rápido)
    em vez de esperar o ciclo inteiro;
  * checagem adaptativa: mede o ritmo de consumo e olha mais cedo quando o
    volume vai cruzar o limite antes da próxima checagem.
"""
import collections
import json
import os
import threading
import time
import traceback

from . import config, paths, store
from .lidl import (CONSUMPTIONS_QUERY, TARIFFOPTIONS_QUERY, LidlApi, as_float,
                   jwt_exp, parse_painel)

FATAL_LOGIN_WORDS = ("captcha", "multifactor", "locked", "blocked", "gesperrt", "lockout")
DEAD_REFRESH_WAIT = 30 * 60   # refresh recusado de vez: não marretar a API


# ------------------------------------------------------------ arquivos de controle

def pause_state():
    """(pausado?, until). until=0 = até retomarem. Pausa vencida é apagada."""
    data = store.read_json(paths.PAUSE_FILE, None)
    if data is None:
        return False, 0.0
    try:
        until = float(data.get("until") or 0)
    except (TypeError, ValueError):
        until = 0.0
    if until and time.time() >= until:
        try:
            paths.PAUSE_FILE.unlink()
        except OSError:
            pass
        return False, 0.0
    return True, until


def set_pause(minutes):
    until = time.time() + minutes * 60 if minutes else 0
    store.write_json(paths.PAUSE_FILE, {"until": until})
    return until


def clear_pause():
    try:
        paths.PAUSE_FILE.unlink()
        return True
    except FileNotFoundError:
        return False


# ------------------------------------------------------------------ históricos

def record_event(kind, **data):
    """refills.jsonl: memória curta de refills (a tela mostra e conta)."""
    data.update({"t": time.time(), "kind": kind})
    try:
        paths.ensure_data_dir()
        with open(paths.REFILLS_FILE, "a", encoding="utf-8") as f:
            f.write(json.dumps(data) + "\n")
        if paths.REFILLS_FILE.stat().st_size > 512 * 1024:
            lines = paths.REFILLS_FILE.read_text(encoding="utf-8").splitlines()[-2000:]
            paths.REFILLS_FILE.write_text("\n".join(lines) + "\n", encoding="utf-8")
    except OSError:
        pass


def record_volume(gb):
    try:
        paths.ensure_data_dir()
        with open(paths.VOLUME_FILE, "a", encoding="utf-8") as f:
            f.write(json.dumps({"t": round(time.time()), "gb": round(gb, 3)}) + "\n")
        if paths.VOLUME_FILE.stat().st_size > 1024 * 1024:
            prune_volume()
    except OSError:
        pass


def prune_volume(keep_seconds=7 * 24 * 3600):
    limit = time.time() - keep_seconds
    keep = []
    for line in paths.VOLUME_FILE.read_text(encoding="utf-8").splitlines():
        try:
            if float(json.loads(line).get("t") or 0) >= limit:
                keep.append(line)
        except (ValueError, TypeError, AttributeError):
            continue
    # Mesmo dentro da janela, nunca deixa crescer sem fim (checagem a cada 15 s).
    keep = keep[-25000:]
    tmp = paths.VOLUME_FILE.with_name(paths.VOLUME_FILE.name + ".tmp")
    tmp.write_text("\n".join(keep) + ("\n" if keep else ""), encoding="utf-8")
    tmp.replace(paths.VOLUME_FILE)


# ---------------------------------------------------------------------- motor

class Monitor:
    def __init__(self, log, notifier=None, api=None):
        self.log = log
        self.notifier = notifier
        self.cfg = config.load()
        self.api = api or LidlApi()
        self.store = store.StateStore()
        self.lock = threading.RLock()       # tokens + conexão: um de cada vez
        self._stop = threading.Event()
        self._wake = threading.Event()
        self._thread = None
        self.st = {}
        self.samples = collections.deque(maxlen=200)
        self.refresh_blocked_until = 0.0
        self.errors = 0
        self.combined = True
        self._last_login_notice = 0.0
        self._last_volume_log = (None, 0.0)
        self._status_lock = threading.Lock()
        self.status = {"fase": "parado", "volume": None, "cooldown": None,
                       "ritmo_gb_h": None, "ultima_checagem": None,
                       "proxima_checagem": None, "motivo_intervalo": "",
                       "precisa_login": None, "erro": None, "token_expira": None,
                       "pendente": None, "ultimo_refill": None, "falhas_seguidas": 0}

    # -- ciclo de vida -------------------------------------------------------

    def start(self):
        if self.running():
            return False
        self._stop.clear()
        self._thread = threading.Thread(target=self.run, name="monitor", daemon=True)
        self._thread.start()
        return True

    def stop(self, wait=25):
        self._stop.set()
        self._wake.set()
        if self._thread and self._thread is not threading.current_thread():
            self._thread.join(wait)

    def running(self):
        return bool(self._thread and self._thread.is_alive() and not self._stop.is_set())

    def poke(self):
        self._wake.set()

    def reload_config(self):
        self.cfg = config.load()
        self.poke()

    def set_status(self, **kw):
        with self._status_lock:
            self.status.update(kw)

    def snapshot(self):
        with self._status_lock:
            snap = dict(self.status)
        paused, until = pause_state()
        snap.update(ligado=self.running(), pausado=paused, pausa_ate=until or None,
                    credenciais=store.has_credentials(), agora=time.time())
        if paused and snap["ligado"]:
            snap["fase"] = "pausado"
        return snap

    def write_status_file(self):
        try:
            store.write_json(paths.STATUS_FILE, self.snapshot())
        except OSError:
            pass

    # -- estado em disco -----------------------------------------------------

    def load_state(self):
        with self.lock:
            self.st = self.store.load()
            seed = os.environ.get("TASKER_REFRESH_TOKEN", "").strip()
            if seed and not self.st.get("refresh"):
                self.log("Usando TASKER_REFRESH_TOKEN do ambiente como semente.")
                self.st["refresh"] = seed
            if not isinstance(self.st.get("pending_refill"), dict) or \
                    not self.st["pending_refill"].get("at"):
                self.st["pending_refill"] = None
            saved = as_float(self.st.get("last_refill")) or 0.0
            # Sem histórico, o relógio do refill forçado começa agora. Com um
            # last_refill de dias atrás, ele vence agora (não "há dias").
            self.st["last_refill"] = max(saved, time.time() - self.cfg["max_without_refill"]) \
                if saved else time.time()
            self.set_status(token_expira=as_float(self.st.get("expires_at")),
                            pendente=self.st["pending_refill"],
                            falhas_seguidas=int(self.st.get("refill_fails") or 0))

    def persist(self, **patch):
        self.st.update(patch)
        try:
            self.store.update(patch)
        except OSError as e:
            self.log(f"NÃO consegui gravar {paths.STATE_FILE.name}: {e}")
            if "refresh" in patch:
                self.notify(f"Não consegui gravar a sessão em disco ({e}). "
                            "Se o Tasker reiniciar agora, vai precisar de login.")

    def notify(self, msg):
        if self.notifier:
            self.notifier.send(msg)
        else:
            self.log(f"Aviso: {msg}")

    # -- tokens --------------------------------------------------------------

    def token_valid(self, margin=0):
        return bool(self.st.get("access")) and \
            (as_float(self.st.get("expires_at")) or 0) - margin > time.time()

    def _set_tokens(self, access, refresh, expires_in=None, expires_at=None):
        exp = jwt_exp(access) or as_float(expires_at) or \
            time.time() + float(expires_in or 3600)
        # Rotacionou: grava JÁ, antes de usar. Perder o refresh novo = login manual.
        self.persist(access=access, refresh=refresh or self.st.get("refresh"),
                     expires_at=exp)
        self.refresh_blocked_until = 0.0
        self.set_status(token_expira=exp, precisa_login=None)

    def adopt_disk(self):
        """Alguém gravou tokens mais novos no arquivo (login por outra via)."""
        disk = self.store.load()
        mine = as_float(self.st.get("expires_at")) or 0
        if disk.get("access") and (as_float(disk.get("expires_at")) or 0) > mine:
            self.st.update(access=disk["access"], refresh=disk.get("refresh"),
                           expires_at=disk.get("expires_at"))
            self.refresh_blocked_until = 0.0
            return True
        if disk.get("refresh") and disk["refresh"] != self.st.get("refresh"):
            self.st["refresh"] = disk["refresh"]
            self.refresh_blocked_until = 0.0
            return True
        return False

    def refresh(self, force=False):
        rt = self.st.get("refresh")
        if not rt or (not force and time.time() < self.refresh_blocked_until):
            return False
        r = self.api.refresh(rt, self.st.get("access"))
        data = r.json() or {}
        if r.status == 200 and isinstance(data, dict) and data.get("access_token"):
            self._set_tokens(data["access_token"], data.get("refresh_token") or rt,
                             data.get("expires_in"))
            self.log(f"Token renovado (vale {self._token_minutes()} min).")
            return True
        if r.status in (400, 401, 403):
            # Recusado de vez (revogado, ou você logou em outro lugar).
            self.refresh_blocked_until = time.time() + DEAD_REFRESH_WAIT
            self.log(f"Refresh token recusado (HTTP {r.status}): {r.snippet(120)}")
        else:
            self.refresh_blocked_until = time.time() + self.cfg["refresh_retry_delay"]
            self.log(f"Renovação do token falhou (HTTP {r.status}: {r.snippet(100)}) — "
                     f"tento de novo em {self.cfg['refresh_retry_delay']} s.")
        return False

    def _token_minutes(self):
        return max(0, int(((as_float(self.st.get("expires_at")) or 0) - time.time()) / 60))

    def ensure_token(self):
        margin = self.cfg["token_margin"]
        if self.token_valid(margin):
            return self.st["access"]
        if self.adopt_disk() and self.token_valid(margin):
            self.log("Peguei do disco um token mais novo.")
            return self.st["access"]
        if self.refresh():
            return self.st["access"]
        if self.token_valid(30):
            return self.st["access"]   # sem margem, mas ainda vale
        ok, _ = self.password_login(auto=True)
        if ok:
            return self.st["access"]
        self._need_login()
        return None

    def _need_login(self):
        has_creds = store.has_credentials()
        reason = ("A sessão da Lidl caiu e o login automático não resolveu agora."
                  if has_creds else "A sessão da Lidl caiu: faça login no Tasker.")
        self.set_status(precisa_login=reason, fase="sem sessão")
        if time.time() - self._last_login_notice > 3600:
            self._last_login_notice = time.time()
            self.notify(reason + " (abra o Tasker)")

    def password_login(self, auto=False, msisdn=None, password=None):
        """(ok, mensagem). auto=True respeita a trava anti-bloqueio de conta."""
        if msisdn is None:
            msisdn, password = store.load_credentials()
        if not msisdn or not password:
            return False, "Sem número/senha guardados."
        now = time.time()
        # A trava pode ter sido zerada por fora (--set-credentials): vale o disco.
        disk = self.store.load()
        for k in ("login_fails", "login_blocked_until"):
            if k in disk:
                self.st[k] = disk[k]
        fails = int(self.st.get("login_fails") or 0)
        blocked = as_float(self.st.get("login_blocked_until")) or 0
        if auto and now < blocked:
            self.log.once("login-espera", "Login automático em espera por "
                          f"{(blocked - now) / 60:.0f} min (proteção da conta).", 1800)
            return False, "Login automático em espera."

        self.log(f"Login por senha ({msisdn[:4]}…{', automático' if auto else ''}).")
        r = self.api.password_login(msisdn, password)
        data = r.json() if isinstance(r.json(), dict) else {}
        if r.status == 200 and data.get("access_token"):
            if data.get("needs_multifactor_authentication") and not data.get("mfa_authenticated"):
                self.persist(login_fails=0, login_blocked_until=now + self.cfg["login_block_long"])
                msg = "A Lidl pediu segundo fator — use \"Entrar pelo navegador\"."
                self.log(msg)
                if auto:
                    self.notify(msg)
                return False, msg
            self._set_tokens(data["access_token"], data.get("refresh_token"),
                             data.get("expires_in"))
            self.persist(login_fails=0, login_blocked_until=0)
            self.log(f"Login por senha OK (token vale {self._token_minutes()} min).")
            self.poke()
            return True, "Login feito."

        api_msg = (data.get("message") or data.get("error_description")
                   or data.get("error") or r.snippet(160))
        self.log(f"Login por senha falhou (HTTP {r.status}): {r.snippet(200)}")
        if r.status == -1 or r.status >= 500:
            # Rede ou erro do servidor não é senha errada: não conta na trava.
            return False, f"A Lidl não respondeu direito (HTTP {r.status}). Tente de novo."
        if any(w in r.text.lower() for w in FATAL_LOGIN_WORDS):
            self.persist(login_fails=0, login_blocked_until=now + self.cfg["login_block_long"])
            msg = ("A Lidl pediu captcha ou travou a conta — o login automático "
                   "parou. Use \"Entrar pelo navegador\".")
            if auto:
                self.notify(msg)
            return False, msg
        fails += 1
        delay = min(self.cfg["login_backoff"] * 2 ** (fails - 1), self.cfg["login_block_long"])
        if fails >= self.cfg["login_max_fails"]:
            delay = self.cfg["login_block_long"]
            if auto:
                self.notify(f"O login automático falhou {fails} vezes — parei por "
                            f"{delay / 3600:.0f} h pra não travar a conta. Confira a senha.")
        self.persist(login_fails=fails, login_blocked_until=now + delay)
        return False, f"Login recusado: {api_msg}"

    def adopt_tokens(self, access, refresh, expires_at=None, source="navegador"):
        """Tokens capturados por outra via (login pelo navegador)."""
        with self.lock:
            self._set_tokens(access, refresh, expires_at=expires_at)
            self.persist(login_fails=0, login_blocked_until=0)
            self.log(f"Sessão nova pelo {source} (token vale {self._token_minutes()} min).")
        self.poke()

    # -- leitura -------------------------------------------------------------

    def read(self, token):
        """(unidades, cooldown, erro). Volume e cooldown numa ida só."""
        r = None
        for attempt in range(2):
            if self.combined:
                r = self.api.painel(token)
            else:
                r = self.api.graphql("consumptions", CONSUMPTIONS_QUERY, token)
            if r.status == 401 and attempt == 0:
                self.log("A API respondeu 401 — renovando o token.")
                if not self.refresh(force=True):
                    self.persist(access=None)   # revogado: não adianta insistir nele
                    if not self.ensure_token():  # senha guardada, ou pede login
                        return None, None, "a Lidl recusou a sessão"
                token = self.st["access"]
                continue
            if r.status in (500, 502, 503, 504) and attempt == 0:
                # A API erra sozinha algumas vezes por hora nas leituras.
                if self._stop.wait(1.5):
                    return None, None, "encerrando"
                continue
            if r.status == 400 and self.combined and attempt == 0:
                self.combined = False
                self.log("A API recusou a consulta combinada — passo a usar duas.")
                continue
            break
        if r.status != 200:
            return None, None, f"HTTP {r.status} {r.snippet(120)}".strip()
        body = r.json()
        if not isinstance(body, dict):
            return None, None, "resposta não era JSON"
        if body.get("errors"):
            msgs = "; ".join(str(e.get("message"))[:90] for e in body["errors"][:2])
            self.log.once("gql-err", f"Erro da API: {msgs}", 600)
        units, cooldown = parse_painel(body.get("data"))
        if not self.combined and units is not None:
            r2 = self.api.graphql("tariffOptions", TARIFFOPTIONS_QUERY, token)
            if r2.status == 200:
                cooldown = parse_painel((r2.json() or {}).get("data"))[1]
        if units is None:
            return None, None, "a resposta não trouxe o consumo"
        return units, cooldown, None

    # -- o ciclo -------------------------------------------------------------

    def cycle(self):
        """Uma checagem completa. Devolve (segundos até a próxima, motivo)."""
        cfg = self.cfg
        self.set_status(fase="checando")
        token = self.ensure_token()
        if not token:
            return self._error_delay(login=True)

        units, cooldown, err = self.read(token)
        if err and self.status.get("precisa_login"):
            return self._error_delay(login=True)
        if err:
            self.errors += 1
            self.log(f"Não consegui ler o volume: {err}")
            self.set_status(erro=err, fase="erro")
            return self._error_delay()
        self.errors = 0

        now = time.time()
        refill, plan = units.get("refill"), units.get("data")
        left = total = None
        if not refill:
            self.log.once("sem-balde", "Nenhum balde REFILLABLE_DATA na conta (tarifa mudou?).", 3600)
        elif refill.get("unit") == "UNLIMITED":
            self.log.once("ilimitado", "Refill marcado como ilimitado — nada a fazer.", 3600)
        else:
            left, total = as_float(refill.get("left")), as_float(refill.get("max"))

        volume = None
        if left is not None:
            self.samples.append((now, left))
            record_volume(left)
            volume = {"left": left, "total": total,
                      "plan_left": as_float((plan or {}).get("left")),
                      "plan_total": as_float((plan or {}).get("max")),
                      "unit": refill.get("unit") or "GB", "t": now}
            self._log_volume(volume)
        self.set_status(volume=volume, cooldown=cooldown, ultima_checagem=now,
                        erro=None, fase="aguardando", precisa_login=None,
                        token_expira=as_float(self.st.get("expires_at")))

        if self.st.get("pending_refill") and self.resolve_pending(left, cooldown):
            return self._confirm_delay()
        if left is None:
            return float(cfg["check_interval"]), "volume ilegível"
        return self.decide(token, left, total, cooldown)

    def decide(self, token, left, total, cooldown):
        cfg = self.cfg
        now = time.time()
        since = now - (as_float(self.st.get("last_refill")) or now)
        forced = since >= cfg["max_without_refill"]
        low = left <= cfg["gb_threshold"]
        if not (forced or low):
            return self.next_delay(left)
        reason = f"GB baixo ({left:.2f})" if low else f"timeout ({since / 60:.0f} min)"

        if total and left >= total - 0.005:
            self.log(f"Refill pedido ({reason}) mas o balde está cheio "
                     f"({left:g}/{total:g} GB) — só zero o relógio.")
            self.persist(last_refill=now, refill_fails=0, next_refill_try=0)
            return self.next_delay(left)
        next_try = as_float(self.st.get("next_refill_try")) or 0
        if now < next_try:
            self.log.once("backoff", f"Refill pedido ({reason.split(' (')[0]}) mas em "
                          f"espera por mais {(next_try - now) / 60:.0f} min "
                          "(falhas seguidas).", 300)
            delay, why = self.next_delay(left)
            return min(delay, max(2.0, next_try - now)), "fim da espera de falha"
        if cooldown is True:
            self.log.once("cooldown", f"Refill pedido ({reason.split(' (')[0]}) mas a "
                          "opção está em cooldown — espero.", 600)
            if low:
                return float(cfg["fast_interval"]), "esperando o cooldown acabar"
            return self.next_delay(left)
        if cooldown is None:
            # Reservar sem conferir o cooldown é o único jeito de reservar duas
            # vezes sem perceber. Esperar uns segundos não custa nada.
            self.log(f"Refill pedido ({reason}) mas não consegui conferir o "
                     "cooldown — confiro de novo em instantes.")
            return float(cfg["fast_interval"]), "cooldown ilegível"
        return self.book(token, left, reason)

    def book(self, token, left, reason):
        self.log(f"Refill: {reason} — pedindo.")
        sent_at = time.time()
        r = self.api.book_refill(token)
        if r.status == 401 and self.refresh(force=True):
            # 401 = recusada antes de processar: mandar de novo é seguro.
            sent_at = time.time()
            r = self.api.book_refill(self.st["access"])
        body = r.json() if r.status == 200 else None
        data = (body or {}).get("data") if isinstance(body, dict) else None
        if isinstance(data, dict) and (data.get("bookTariffoptionsDirect") or {}).get("success"):
            self.log(f"Refill aceito pela API ({r.ms} ms).")
            self._worked(left, None, "API confirmou", sent_at)
            return self.next_delay(left)
        if r.status == 500:
            self.log(f"Refill enviado ({r.ms} ms): HTTP 500 genérico — normal aqui, "
                     "confirmo pelo efeito.")
        elif r.status == -1:
            self.log(f"Refill: a conexão falhou ({r.text[:80]}) — pode ter chegado; "
                     "confiro pelo efeito, sem repetir.")
        else:
            self.log(f"Refill: resposta HTTP {r.status} {r.snippet(100)} — confiro pelo efeito.")
        pend = {"at": sent_at, "before": left}
        self.persist(pending_refill=pend)
        self.set_status(pendente=pend, fase="confirmando refill")
        return self._confirm_delay()

    def resolve_pending(self, left, cooldown):
        """True se a reserva anterior segue sem veredito (não mexer em nada)."""
        pend = self.st["pending_refill"]
        before = as_float(pend.get("before"))
        at = as_float(pend.get("at")) or 0
        if left is not None and before is not None and left > before + 0.005:
            self._worked(before, left, "volume subiu", at)
            return False
        if cooldown is True:
            self._worked(before, left, "cooldown da opção", at)
            return False
        if time.time() - at < self.cfg["pending_grace"]:
            self.set_status(fase="confirmando refill")
            return True
        self.log(f"A reserva não surtiu efeito em {self.cfg['pending_grace']} s.")
        self._failed(before)
        return False

    def _worked(self, before, after, why, sent_at):
        now = time.time()
        latency = round(now - sent_at, 1) if sent_at else None
        self.persist(pending_refill=None, last_refill=now, refill_fails=0, next_refill_try=0)
        record_event("refill_ok", before=before, after=after, why=why, latency=latency)
        txt = f"{before:.2f} → {after:.2f} GB" if before is not None and after is not None else ""
        self.log(f"Refill confirmado ({why}){': ' + txt if txt else ''}"
                 f"{f' — levou {latency:.0f} s' if latency else ''}.")
        self.set_status(pendente=None, falhas_seguidas=0, ultimo_refill={
            "t": now, "before": before, "after": after, "why": why, "latencia": latency})

    def _failed(self, before):
        fails = int(self.st.get("refill_fails") or 0) + 1
        backoff = min(self.cfg["check_interval"] * 2 ** fails, self.cfg["refill_backoff_max"])
        self.persist(pending_refill=None, refill_fails=fails, next_refill_try=time.time() + backoff)
        record_event("refill_fail", before=before, fails=fails)
        self.log(f"Refill falhou ({fails}x seguidas) — nova tentativa em {backoff / 60:.0f} min.")
        self.set_status(pendente=None, falhas_seguidas=fails)
        if fails == 4:
            self.notify("O refill da Lidl falhou 4 vezes seguidas. Vale olhar o Tasker.")

    # -- ritmo ---------------------------------------------------------------

    def rate(self):
        """Consumo em GB/s: o maior entre a janela de 2 min e a de 10 min.

        A curta pega um download que acabou de começar; a longa dá estabilidade.
        Subidas (refills) são ignoradas — só conta o que foi gasto.
        """
        now = time.time()
        best = None
        for window in (120, 600):
            pts = [p for p in self.samples if p[0] >= now - window]
            if len(pts) < 2:
                continue
            span = pts[-1][0] - pts[0][0]
            if span < 40:
                continue
            spent = sum(max(0.0, a[1] - b[1]) for a, b in zip(pts, pts[1:]))
            rate = spent / span
            best = rate if best is None else max(best, rate)
        return best

    def next_delay(self, left):
        cfg = self.cfg
        delay, why = float(cfg["check_interval"]), "intervalo normal"
        rate = self.rate()
        self.set_status(ritmo_gb_h=round(rate * 3600, 2) if rate is not None else None)
        if cfg["adaptive"] and rate and left is not None:
            head = left - cfg["gb_threshold"]
            if head > 0 and head / rate / 2 < delay:
                eta = head / rate
                delay = max(float(cfg["fast_interval"]), eta / 2)
                why = f"adaptativo: no ritmo atual cruza o limite em ~{eta:.0f} s"
        last = as_float(self.st.get("last_refill")) or time.time()
        until_forced = cfg["max_without_refill"] - (time.time() - last)
        if 0 < until_forced + 1 < delay:
            delay, why = until_forced + 1, "hora do refill forçado"
        return max(2.0, delay), why

    def _confirm_delay(self):
        return float(self.cfg["fast_interval"]), "confirmando o refill"

    def _error_delay(self, login=False):
        base = self.cfg["check_interval"]
        if login:
            return float(max(base, 60)), "sem sessão"
        return float(min(base * 2 ** min(self.errors - 1, 3), 300)), "erro na leitura"

    def _log_volume(self, v):
        key = (v["left"], v["total"], v["plan_left"])
        last_key, last_t = self._last_volume_log
        if key == last_key and time.time() - last_t < 600:
            return
        self._last_volume_log = (key, time.time())
        plan = ""
        if v["plan_left"] is not None:
            plan = f" | plano: {v['plan_left']:g}"
            plan += f"/{v['plan_total']:g} GB" if v["plan_total"] is not None else " GB"
        total = f"/{v['total']:g}" if v["total"] is not None else ""
        self.log(f"Volume de refill: {v['left']:.2f}{total} GB{plan}")

    # -- loop ----------------------------------------------------------------

    def describe(self):
        c = self.cfg
        adapt = f" (adaptativa, até {c['fast_interval']} s)" if c["adaptive"] else ""
        return (f"checagem a cada {c['check_interval']} s{adapt} | refill se ≤ "
                f"{c['gb_threshold']:g} GB | força após {c['max_without_refill'] // 60} min")

    def run(self, once=False):
        try:
            self.load_state()
            self.log(f"Monitor ligado | {self.describe()}")
            self.set_status(fase="iniciando")
            while not self._stop.is_set():
                paused, until = pause_state()
                if paused:
                    self._wait_pause(until)
                    if once:
                        break
                    continue
                try:
                    with self.lock:
                        delay, why = self.cycle()
                except Exception as e:   # nunca deixa o loop morrer por exceção solta
                    self.errors += 1
                    self.log(f"Erro inesperado no ciclo: {type(e).__name__}: {e}")
                    self.log(traceback.format_exc().strip().splitlines()[-2].strip())
                    delay, why = self._error_delay()
                self.write_status_file()
                # No modo de uma passada (cron), um refill recém-pedido é
                # confirmado aqui mesmo, no intervalo rápido — senão a pendência
                # ficaria pra próxima execução, minutos depois.
                if once and not self.st.get("pending_refill"):
                    break
                self._sleep(delay, why)
        finally:
            self.api.close()
            self.set_status(fase="parado", proxima_checagem=None)
            self.write_status_file()
            if not once:
                self.log("Monitor desligado.")

    def _sleep(self, delay, why):
        self.set_status(proxima_checagem=time.time() + delay, motivo_intervalo=why)
        if self._wake.wait(delay):
            self._wake.clear()

    def _wait_pause(self, until):
        txt = time.strftime("até %H:%M", time.localtime(until)) if until else \
            "até você retomar"
        self.log(f"Pausado {txt} — não toco na sessão (conta liberada pra você).")
        self.set_status(fase="pausado", proxima_checagem=None)
        self.write_status_file()
        while not self._stop.is_set() and pause_state()[0]:
            if self._wake.wait(2):
                self._wake.clear()
        if not self._stop.is_set():
            self.log("Pausa encerrada — voltando ao trabalho.")


def watch_control_files(monitor, on_stop, stop_event):
    """tasker.poke = checar agora; tasker.stop = encerrar limpo (qualquer script cria)."""
    def _run():
        for f in (paths.STOP_FILE, paths.POKE_FILE):   # sobras da execução anterior
            try:
                f.unlink()
            except OSError:
                pass
        while not stop_event.wait(1):
            try:
                if paths.POKE_FILE.exists():
                    paths.POKE_FILE.unlink()
                    monitor.log("Checagem adiantada (tasker.poke).")
                    monitor.poke()
            except OSError:
                pass
            try:
                if paths.STOP_FILE.exists():
                    paths.STOP_FILE.unlink()
                    monitor.log("Parada pedida por tasker.stop — encerrando limpo.")
                    on_stop()
            except OSError:
                pass
            if pause_state()[0] and monitor.status.get("fase") not in ("pausado", "parado"):
                monitor.poke()   # pausa criada por fora: o motor percebe já
    t = threading.Thread(target=_run, name="controle", daemon=True)
    t.start()
    return t
