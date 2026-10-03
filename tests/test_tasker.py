"""Testes do Tasker 2 — 100% offline, contra a API de mentira (fake_lidl.py).

    python3 tests/test_tasker.py

Nada aqui encosta nos seus arquivos de verdade: a pasta de dados é temporária,
XDG_* apontam pra pastas temporárias, e uma sentinela confere no fim que a
sessão, o config e os históricos reais não mudaram nem um byte.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TMP = Path(tempfile.mkdtemp(prefix="tasker-teste-"))
for k in list(os.environ):
    if k.startswith("TASKER_") or k in ("XDG_CONFIG_HOME", "XDG_DATA_HOME"):
        del os.environ[k]
os.environ["TASKER_DATA_DIR"] = str(TMP / "dados")
os.environ["XDG_CONFIG_HOME"] = str(TMP / "xdg-config")
os.environ["XDG_DATA_HOME"] = str(TMP / "xdg-data")
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tests"))

from fake_lidl import FakeLidl, make_jwt  # noqa: E402
from tasker import config, engine, paths, store, system  # noqa: E402
from tasker.app import App  # noqa: E402
from tasker.engine import Monitor  # noqa: E402
from tasker.lidl import LidlApi, jwt_exp, parse_painel  # noqa: E402
from tasker.logs import Logger  # noqa: E402
from tasker.server import UiServer, history  # noqa: E402

# ------------------------------------------------------------------ sentinela
# Os arquivos que o app só muda por ação sua: têm que sair idênticos. Os que o
# Tasker de verdade muda enquanto roda (sessão, log, históricos) não dá pra
# comparar byte a byte — neles a sentinela procura rastro dos dados de teste.
REAL_DATA = ROOT / "dados"
_FIXED = [REAL_DATA / "config.json", REAL_DATA / "credenciais.json",
          Path.home() / ".config/autostart/tasker.desktop",
          Path.home() / ".local/share/applications/tasker.desktop"]
_TEST_MARKS = (b"rt-", b"assinatura", b"tok-a", b"01761234567", b"chave-de-teste")


def _fingerprint():
    out = {}
    for p in _FIXED:
        try:
            out[str(p)] = hashlib.sha256(p.read_bytes()).hexdigest()
        except OSError:
            out[str(p)] = None
    return out


def _leaks():
    found = []
    for p in REAL_DATA.glob("*") if REAL_DATA.exists() else []:
        if p.is_file() and p.suffix in (".json", ".jsonl"):
            data = p.read_bytes()
            found += [f"{p.name}: {m.decode()}" for m in _TEST_MARKS if m in data]
    return found


_BEFORE = _fingerprint()
_LEAKS_BEFORE = _leaks()


class FakeNotifier:
    def __init__(self):
        self.msgs = []

    def send(self, msg):
        self.msgs.append(msg)
        return 1


class Base(unittest.TestCase):
    def setUp(self):
        self.dir = TMP / f"dados-{self.id().split('.')[-1]}"
        shutil.rmtree(self.dir, ignore_errors=True)
        paths.set_data_dir(self.dir)
        paths.ensure_data_dir()
        self.fake = FakeLidl()
        self.origin = self.fake.start()
        self.monitors = []

    def tearDown(self):
        for m in self.monitors:
            m.api.close()
        self.fake.stop()

    def monitor(self, tokens=True, **cfg):
        if tokens:
            a, r = self.fake.issue()
            store.StateStore().update({"access": a, "refresh": r, "expires_at": jwt_exp(a)})
        log = Logger(to_file=False, to_stdout=False)
        m = Monitor(log, api=LidlApi(self.origin, timeout=5))
        m.notifier = FakeNotifier()
        m.cfg.update(cfg)
        m.load_state()
        self.monitors.append(m)
        return m

    def disk(self):
        return store.StateStore().load()


# ======================================================================= config
class TestConfig(Base):
    def test_defaults_and_precedence(self):
        self.assertEqual(config.load(env={})["check_interval"], 60)
        store.write_json(paths.CONFIG_FILE, {"check_interval": 40, "gb_threshold": "0,4",
                                             "headless": True, "keepalive_interval": 600})
        vals, src = config.load_with_sources(env={})
        self.assertEqual((vals["check_interval"], src["check_interval"]), (40, "arquivo"))
        self.assertEqual(vals["gb_threshold"], 0.4)
        vals, src = config.load_with_sources(env={"TASKER_CHECK_INTERVAL": "120"})
        self.assertEqual((vals["check_interval"], src["check_interval"]), (120, "ambiente"))
        self.assertNotIn("headless", vals)   # ajuste da v1, morto

    def test_invalid_values_fall_back(self):
        store.write_json(paths.CONFIG_FILE, {"check_interval": 1, "gb_threshold": "abc"})
        vals = config.load(env={})
        self.assertEqual(vals["check_interval"], 60)
        self.assertEqual(vals["gb_threshold"], 0.85)
        paths.CONFIG_FILE.write_text("{quebrado", encoding="utf-8")
        self.assertEqual(config.load(env={})["check_interval"], 60)

    def test_display_units(self):
        spec = config.BY_KEY["max_without_refill"]
        self.assertEqual(config.to_display(spec, 840), 14)
        self.assertEqual(config.from_display(spec, "14"), 840)
        with self.assertRaises(ValueError):
            config.from_display(spec, "1")
        self.assertEqual(config.from_display(config.BY_KEY["ntfy_topic"], "https://n.tld/x"),
                         "https://n.tld/x")
        with self.assertRaises(ValueError):
            config.from_display(config.BY_KEY["ntfy_topic"], "com espaço")

    def test_save_drops_obsolete_keys(self):
        store.write_json(paths.CONFIG_FILE, {"headless": False, "check_interval": 40})
        from tasker.server import save_settings
        ok, _, errs = save_settings({"gb_threshold": "0,5"})
        self.assertTrue(ok, errs)
        saved = json.loads(paths.CONFIG_FILE.read_text())
        self.assertEqual(saved, {"check_interval": 40, "gb_threshold": 0.5})
        ok, _, errs = save_settings({"gb_threshold": "9"})
        self.assertFalse(ok)
        self.assertIn("gb_threshold", errs)


# ==================================================================== migração
class TestMigration(Base):
    def test_moves_v1_files_without_overwriting(self):
        old = TMP / "raiz-v1"
        shutil.rmtree(old, ignore_errors=True)
        old.mkdir()
        (old / "session_tokens.json").write_text('{"refresh": "rt-velho"}')
        (old / "config.json").write_text('{"check_interval": 40}')
        (old / "refill_history.jsonl").write_text("")
        (old / "tasker.log").write_text("x\n")
        store.write_json(paths.CONFIG_FILE, {"check_interval": 99})   # já existe: fica
        moved = paths.migrate_legacy(base=old)
        self.assertIn("session_tokens.json", moved)
        self.assertNotIn("config.json", moved)
        self.assertEqual(store.StateStore().load()["refresh"], "rt-velho")
        self.assertEqual(json.loads(paths.CONFIG_FILE.read_text())["check_interval"], 99)
        self.assertTrue((old / "config.json").exists())
        self.assertEqual(paths.migrate_legacy(base=old), [])

    def test_default_migration_is_noop_with_custom_dir(self):
        self.assertEqual(paths.migrate_legacy(), [])


# ======================================================================== API
class TestApi(Base):
    def test_parse_painel(self):
        units, cd = parse_painel({"consumptions": {"consumptionsForUnit": [
            {"type": "REFILLABLE_DATA", "left": 0.3, "max": 1}]},
            "tariffoptions": {"disabledTariffoptions": {"disabledTariffoptions": [
                {"tariffoptionId": "CCS_92061"}]}}})
        self.assertEqual(units["refill"]["left"], 0.3)
        self.assertIs(cd, True)
        units, cd = parse_painel({"consumptions": {"consumptionsForUnit": []}})
        self.assertEqual(units, {})
        self.assertIsNone(cd)
        self.assertEqual(parse_painel(None), (None, None))

    def test_jwt_exp(self):
        self.assertAlmostEqual(jwt_exp(make_jwt(123.0)), 123.0)
        self.assertEqual(jwt_exp("lixo"), 0.0)

    def test_keepalive_reuses_connection(self):
        m = self.monitor()
        for _ in range(4):
            m.cycle()
        self.assertEqual(self.fake.count("painel"), 4)
        self.assertEqual(self.fake.connections, 1, "devia reaproveitar a conexão")

    def test_reconnects_when_server_drops(self):
        m = self.monitor()
        m.cycle()
        m.api._conn.sock.close()   # simula o servidor fechando a keep-alive
        m.api._conn.sock = None
        delay, _ = m.cycle()
        self.assertEqual(self.fake.count("painel"), 2)
        self.assertIsNone(m.status["erro"])


# ===================================================================== refill
class TestRefill(Base):
    def test_reads_volume_in_one_request(self):
        self.fake.left = 0.9
        m = self.monitor(gb_threshold=0.4)
        delay, why = m.cycle()
        self.assertEqual([op for op, _ in self.fake.calls], ["painel"])
        self.assertEqual(m.status["volume"]["left"], 0.9)
        self.assertIs(m.status["cooldown"], False)
        self.assertTrue(paths.VOLUME_FILE.exists())

    def test_low_volume_books_once_and_confirms_fast(self):
        self.fake.left = 0.3
        self.fake.rise_after_reads = 2
        m = self.monitor(gb_threshold=0.4, fast_interval=7)
        delay, why = m.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1)
        self.assertEqual(delay, 7.0)
        self.assertEqual(why, "confirmando o refill")
        pend = self.disk()["pending_refill"]
        self.assertEqual(pend["before"], 0.3)      # persistida ANTES de confirmar
        delay, _ = m.cycle()                       # ainda não subiu
        self.assertEqual(delay, 7.0)
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1)
        m.cycle()                                  # subiu
        self.assertIsNone(self.disk()["pending_refill"])
        ev = [json.loads(x) for x in paths.REFILLS_FILE.read_text().splitlines()]
        self.assertEqual(ev[-1]["kind"], "refill_ok")
        self.assertEqual(ev[-1]["why"], "volume subiu")
        self.assertIsNotNone(ev[-1]["latency"])
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1)

    def test_pending_survives_restart(self):
        self.fake.left = 0.3
        self.fake.rise_after_reads = 99
        m = self.monitor(gb_threshold=0.4)
        m.cycle()
        m2 = self.monitor(tokens=False, gb_threshold=0.4)
        self.assertIsNotNone(m2.st["pending_refill"])
        m2.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1, "reservou duas vezes")

    def test_no_effect_counts_failure_and_backs_off(self):
        self.fake.left = 0.3
        self.fake.refill_mode = "noop"
        m = self.monitor(gb_threshold=0.4, pending_grace=30, check_interval=60)
        m.cycle()
        m.st["pending_refill"]["at"] -= 31      # passou o prazo
        m.cycle()
        d = self.disk()
        self.assertEqual(d["refill_fails"], 1)
        self.assertGreater(d["next_refill_try"], time.time() + 100)
        m.cycle()                               # em espera: não reserva de novo
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1)

    def test_fourth_failure_notifies(self):
        self.fake.left = 0.3
        self.fake.refill_mode = "noop"
        m = self.monitor(gb_threshold=0.4, pending_grace=30)
        for _ in range(4):
            m.st["next_refill_try"] = 0
            m.cycle()
            m.st["pending_refill"]["at"] -= 31
            m.cycle()
        self.assertEqual(self.disk()["refill_fails"], 4)
        self.assertTrue(any("4 vezes" in x for x in m.notifier.msgs))

    def test_unknown_cooldown_never_books(self):
        self.fake.left = 0.2
        self.fake.omit_tariffs = True
        m = self.monitor(gb_threshold=0.4, fast_interval=9)
        delay, why = m.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 0)
        self.assertEqual((delay, why), (9.0, "cooldown ilegível"))

    def test_cooldown_waits(self):
        self.fake.left = 0.2
        self.fake.cooldown = True
        m = self.monitor(gb_threshold=0.4)
        m.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 0)

    def test_cooldown_after_booking_confirms(self):
        self.fake.left = 0.3
        self.fake.rise_after_reads = 99
        m = self.monitor(gb_threshold=0.4)
        m.cycle()
        self.fake.cooldown = True
        m.cycle()
        self.assertIsNone(self.disk()["pending_refill"])

    def test_full_bucket_forced_only_resets_clock(self):
        self.fake.left = 1.0
        m = self.monitor(max_without_refill=300)
        m.st["last_refill"] = time.time() - 400
        m.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 0)
        self.assertGreater(self.disk()["last_refill"], time.time() - 5)

    def test_forced_refill_when_not_full(self):
        self.fake.left = 0.95
        m = self.monitor(max_without_refill=300, gb_threshold=0.4)
        m.st["last_refill"] = time.time() - 400
        m.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1)

    def test_api_success_is_trusted(self):
        self.fake.left = 0.1
        self.fake.refill_mode = "success"
        m = self.monitor(gb_threshold=0.4)
        m.cycle()
        self.assertIsNone(self.disk()["pending_refill"])
        self.assertEqual(m.status["ultimo_refill"]["why"], "API confirmou")

    def test_dropped_mutation_is_not_repeated(self):
        self.fake.left = 0.1
        self.fake.refill_mode = "drop"
        m = self.monitor(gb_threshold=0.4)
        m.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1)
        self.assertIsNotNone(self.disk()["pending_refill"])
        m.cycle()
        self.assertEqual(self.fake.count("bookTariffOptionsDirect"), 1)

    def test_transient_500_on_read_is_retried(self):
        self.fake.fail_reads = [500]
        m = self.monitor()
        m.cycle()
        self.assertIsNone(m.status["erro"])
        self.assertEqual(self.fake.count("painel"), 2)

    def test_persistent_read_errors_back_off(self):
        self.fake.fail_reads = [500] * 10
        m = self.monitor(check_interval=40)
        d1, _ = m.cycle()
        d2, _ = m.cycle()
        self.assertEqual((d1, d2), (40.0, 80.0))
        self.assertIsNotNone(m.status["erro"])

    def test_combined_rejected_falls_back(self):
        self.fake.reject_combined = True
        self.fake.left = 0.9
        m = self.monitor()
        m.cycle()
        self.assertFalse(m.combined)
        self.assertEqual(m.status["volume"]["left"], 0.9)
        self.assertIs(m.status["cooldown"], False)

    def test_pause_is_silent(self):
        m = self.monitor()
        engine.set_pause(30)
        t = threading.Thread(target=m.run, daemon=True)
        m._thread = t
        t.start()
        time.sleep(1.0)
        self.assertEqual(self.fake.calls, [])
        self.assertEqual(m.snapshot()["fase"], "pausado")
        engine.clear_pause()
        m.poke()
        for _ in range(40):
            if self.fake.calls:
                break
            time.sleep(0.1)
        m.stop()
        self.assertTrue(self.fake.calls)
        self.assertFalse(engine.pause_state()[0])

    def test_expired_pause_is_removed(self):
        store.write_json(paths.PAUSE_FILE, {"until": time.time() - 1})
        self.assertEqual(engine.pause_state(), (False, 0.0))
        self.assertFalse(paths.PAUSE_FILE.exists())


# ===================================================================== tokens
class TestTokens(Base):
    def test_refresh_rotates_and_persists_first(self):
        a, r = self.fake.issue(ttl=60)   # vence dentro da margem de 5 min
        store.StateStore().update({"access": a, "refresh": r, "expires_at": jwt_exp(a)})
        m = self.monitor(tokens=False)
        m.cycle()
        d = self.disk()
        self.assertNotEqual(d["refresh"], r)
        self.assertIn(d["refresh"], self.fake.refresh)
        self.assertNotIn(r, self.fake.refresh)
        self.assertGreater(d["expires_at"], time.time() + 3000)

    def test_401_refreshes_and_retries(self):
        m = self.monitor()
        self.fake.access.clear()            # revogado no servidor
        m.cycle()
        self.assertIsNone(m.status["erro"])
        self.assertEqual(self.fake.count("refresh"), 1)

    def test_dead_session_without_credentials_asks_login_once(self):
        m = self.monitor()
        self.fake.access.clear()
        self.fake.refresh.clear()
        m.cycle()
        self.assertTrue(m.status["precisa_login"])
        m.cycle()
        m.cycle()
        self.assertEqual(len([x for x in m.notifier.msgs if "login" in x.lower()]), 1)
        self.assertEqual(self.fake.count("refresh"), 1, "marretou o refresh morto")

    def test_dead_session_relogs_with_password(self):
        store.save_credentials(*self.fake.password)
        m = self.monitor()
        self.fake.access.clear()
        self.fake.refresh.clear()
        m.cycle()
        self.assertIsNone(m.status["precisa_login"])
        self.assertEqual(self.fake.count("password"), 1)
        self.assertEqual(self.fake.count("painel"), 2)

    def test_seed_from_env(self):
        _, r = self.fake.issue()
        os.environ["TASKER_REFRESH_TOKEN"] = r
        try:
            m = self.monitor(tokens=False)
            m.cycle()
        finally:
            del os.environ["TASKER_REFRESH_TOKEN"]
        self.assertIsNone(m.status["erro"])
        self.assertIn(self.disk()["refresh"], self.fake.refresh)

    def test_v1_state_file_is_compatible(self):
        a, r = self.fake.issue()
        store.write_json(paths.STATE_FILE, {
            "access": a, "refresh": r, "expires_at": jwt_exp(a), "saved_at": 1,
            "pending_refill": None, "last_refill": time.time() - 60,
            "refill_fails": 0, "next_refill_try": 0}, private=True)
        m = self.monitor(tokens=False)
        m.cycle()
        self.assertIsNone(m.status["erro"])
        self.assertEqual(self.fake.count("refresh"), 0)

    def test_disk_adoption(self):
        m = self.monitor()
        a, r = self.fake.issue()
        store.StateStore().update({"access": a, "refresh": r, "expires_at": jwt_exp(a) + 100})
        m.st["expires_at"] = time.time() + 10   # o meu está vencendo
        m.cycle()
        self.assertEqual(m.st["access"], a)
        self.assertEqual(self.fake.count("refresh"), 0)


class TestPasswordLogin(Base):
    def test_wrong_password_backs_off(self):
        m = self.monitor()
        ok, msg = m.password_login(auto=True, msisdn="01761234567", password="errada")
        self.assertFalse(ok)
        self.assertIn("falsch", msg)
        d = self.disk()
        self.assertEqual(d["login_fails"], 1)
        self.assertGreater(d["login_blocked_until"], time.time() + 200)
        ok, msg = m.password_login(auto=True, msisdn="01761234567", password="certa")
        self.assertFalse(ok)              # ainda na espera
        self.assertEqual(self.fake.count("password"), 1)
        ok, _ = m.password_login(auto=False, msisdn="01761234567", password="certa")
        self.assertTrue(ok)               # manual não respeita a espera
        self.assertEqual(self.disk()["login_fails"], 0)

    def test_max_fails_blocks_long_and_notifies(self):
        m = self.monitor(login_max_fails=2)
        for _ in range(2):
            m.st["login_blocked_until"] = 0
            store.StateStore().update({"login_blocked_until": 0})
            m.password_login(auto=True, msisdn="x", password="y")
        self.assertGreater(self.disk()["login_blocked_until"], time.time() + 5 * 3600)
        self.assertTrue(any("2 vezes" in x for x in m.notifier.msgs))

    def test_captcha_stops_automatic_login(self):
        self.fake.captcha = True
        m = self.monitor()
        ok, msg = m.password_login(auto=True, msisdn="01761234567", password="certa")
        self.assertFalse(ok)
        self.assertIn("captcha", msg.lower())
        self.assertGreater(self.disk()["login_blocked_until"], time.time() + 5 * 3600)

    def test_server_error_does_not_count(self):
        m = self.monitor()
        m.api = LidlApi("http://127.0.0.1:9", timeout=2)   # porta fechada
        ok, _ = m.password_login(auto=True, msisdn="x", password="y")
        self.assertFalse(ok)
        self.assertEqual(int(self.disk().get("login_fails") or 0), 0)

    def test_set_credentials_reset_is_seen(self):
        m = self.monitor()
        m.password_login(auto=True, msisdn="01761234567", password="errada")
        store.StateStore().update({"login_fails": 0, "login_blocked_until": 0})  # --set-credentials
        ok, _ = m.password_login(auto=True, msisdn="01761234567", password="certa")
        self.assertTrue(ok)


# =================================================================== adaptativo
class TestAdaptive(Base):
    def test_fast_consumption_shortens_interval(self):
        m = self.monitor(check_interval=60, fast_interval=15, gb_threshold=0.4,
                         max_without_refill=3600)
        now = time.time()
        m.samples.extend([(now - 120, 0.9), (now - 60, 0.8), (now, 0.7)])  # 0.1 GB/min
        delay, why = m.next_delay(0.7)
        # cruza 0.4 em ~180 s -> olha em ~90 s, limitado aos 60 normais
        self.assertEqual(delay, 60.0)
        m.samples.clear()
        m.samples.extend([(now - 120, 1.0), (now - 60, 0.75), (now, 0.5)])  # 0.25 GB/min
        delay, why = m.next_delay(0.5)
        self.assertAlmostEqual(delay, 15.0, delta=0.5)   # ~12 s -> piso de 15
        self.assertIn("adaptativo", why)

    def test_idle_keeps_normal_interval(self):
        m = self.monitor(check_interval=60, max_without_refill=3600)
        now = time.time()
        m.samples.extend([(now - 300, 0.9), (now, 0.9)])
        self.assertEqual(m.next_delay(0.9)[0], 60.0)

    def test_disabled(self):
        m = self.monitor(check_interval=60, adaptive=False, max_without_refill=3600)
        now = time.time()
        m.samples.extend([(now - 120, 1.0), (now, 0.5)])
        self.assertEqual(m.next_delay(0.5)[0], 60.0)

    def test_wakes_for_forced_refill(self):
        m = self.monitor(check_interval=60, max_without_refill=300)
        m.st["last_refill"] = time.time() - 280
        delay, why = m.next_delay(0.9)
        self.assertLess(delay, 25)
        self.assertEqual(why, "hora do refill forçado")


# ========================================================================= log
class TestLogger(Base):
    def test_since_and_reset(self):
        log = Logger(to_file=False, to_stdout=False)
        log("a")
        log("b")
        r = log.since(0)
        self.assertEqual(len(r["linhas"]), 2)
        log("c")
        r2 = log.since(r["seq"])
        self.assertEqual(len(r2["linhas"]), 1)
        self.assertFalse(r2["reset"])
        self.assertTrue(log.since(9999)["reset"])

    def test_once(self):
        log = Logger(to_file=False, to_stdout=False)
        log.once("k", "x")
        log.once("k", "x")
        log.once("k", "y")
        self.assertEqual(len(log.since(0)["linhas"]), 2)

    def test_file_rotation(self):
        import tasker.logs as L
        old = L.LOG_MAX_BYTES
        L.LOG_MAX_BYTES = 200
        try:
            log = Logger(to_stdout=False)
            for i in range(30):
                log(f"linha {i}")
        finally:
            L.LOG_MAX_BYTES = old
        self.assertTrue(paths.LOG_FILE.with_name("tasker.log.1").exists())


# ====================================================================== tela
class TestServer(Base):
    def setUp(self):
        super().setUp()
        self.app = App(Logger(to_file=False, to_stdout=False))
        self.app.monitor.api = LidlApi(self.origin, timeout=5)
        self.app.monitor.notifier = FakeNotifier()
        self.ui = UiServer(self.app, 18765)
        self.app.server = self.ui
        threading.Thread(target=self.ui.serve, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.ui.port}"
        self.op = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect)

    def tearDown(self):
        self.ui.httpd.shutdown()
        self.ui.httpd.server_close()
        self.app.monitor.api.close()
        super().tearDown()

    def req(self, path, body=None, key=True, host=None):
        h = {"Content-Type": "application/json"}
        if key:
            h["X-Tasker-Key"] = self.ui.key
        if host:
            h["Host"] = host
        r = urllib.request.Request(self.base + path, headers=h,
                                   data=json.dumps(body).encode() if body is not None else None)
        try:
            with self.op.open(r, timeout=5) as resp:
                return resp.status, resp.read(), resp.headers
        except urllib.error.HTTPError as e:
            return e.code, e.read(), e.headers

    def test_auth(self):
        self.assertEqual(self.req("/api/estado", key=False)[0], 403)
        self.assertEqual(self.req("/api/estado")[0], 200)
        self.assertEqual(self.req("/api/estado", host="evil.example")[0], 403)
        self.assertEqual(self.req("/api/acao", {"acao": "sair"}, key=False)[0], 403)
        self.assertEqual(self.req("/")[0], 200)              # a casca não tem segredo
        self.assertNotIn(self.ui.key.encode(), self.req("/")[1])

    def test_boot_token_single_use(self):
        boot = self.ui.new_boot()
        st, _, h = self.req(f"/?boot={boot}", key=False)
        self.assertEqual(st, 302)
        self.assertEqual(h["Location"], f"/#k={self.ui.key}")
        self.assertEqual(self.req(f"/?boot={boot}", key=False)[0], 403)

    def test_state_settings_and_actions(self):
        st, body, _ = self.req("/api/estado")
        data = json.loads(body)
        self.assertIn("monitor", data)
        self.assertFalse(data["monitor"]["ligado"])
        st, body, _ = self.req("/api/ajustes")
        self.assertEqual(len(json.loads(body)["campos"]), len(config.SETTINGS))
        st, body, _ = self.req("/api/ajustes", {"valores": {"check_interval": "5"}})
        self.assertFalse(json.loads(body)["ok"])
        st, body, _ = self.req("/api/ajustes", {"valores": {"check_interval": "45"}})
        self.assertTrue(json.loads(body)["ok"])
        self.assertEqual(self.app.monitor.cfg["check_interval"], 45)   # vale na hora
        self.req("/api/acao", {"acao": "pausar", "minutos": 30})
        self.assertTrue(engine.pause_state()[0])
        self.req("/api/acao", {"acao": "retomar"})
        self.assertFalse(engine.pause_state()[0])
        st, body, _ = self.req("/api/acao", {"acao": "inexistente"})
        self.assertIn("erro", json.loads(body))

    def test_password_login_action(self):
        st, body, _ = self.req("/api/acao", {"acao": "login_senha", "msisdn": "01761234567",
                                             "senha": "certa", "lembrar": True})
        self.assertTrue(json.loads(body)["ok"])
        self.assertTrue(paths.CREDENTIALS_FILE.exists())
        if os.name != "nt":
            self.assertEqual(paths.CREDENTIALS_FILE.stat().st_mode & 0o777, 0o600)
        st, body, _ = self.req("/api/acao", {"acao": "login_senha", "msisdn": "x", "senha": "y"})
        self.assertFalse(json.loads(body)["ok"])

    def test_history(self):
        now = time.time()
        with open(paths.VOLUME_FILE, "w") as f:
            for i in range(3000):
                f.write(json.dumps({"t": now - 3000 + i, "gb": 0.5 + (i % 10) / 100}) + "\n")
        engine.record_event("refill_ok", before=0.4, after=1.0, why="volume subiu", latency=12)
        h = history(24)
        self.assertLessEqual(len(h["volume"]), 1510)
        self.assertEqual(h["resumo"]["refills_hoje"], 1)
        self.assertEqual(h["resumo"]["latencia_media"], 12)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k):
        return None


# ================================================================ linha de comando
class TestCli(Base):
    def env(self):
        e = dict(os.environ, TASKER_DATA_DIR=str(self.dir), TASKER_API_ORIGIN=self.origin,
                 TASKER_NOTIFY="0", TASKER_BROWSER="/nao/existe")
        return e

    def run_cli(self, *args, timeout=40):
        return subprocess.run([sys.executable, str(ROOT / "Tasker.pyw"), *args], env=self.env(),
                              capture_output=True, text=True, timeout=timeout)

    def test_once_and_status(self):
        a, r = self.fake.issue()
        store.StateStore().update({"access": a, "refresh": r, "expires_at": jwt_exp(a)})
        self.fake.left = 0.9
        res = self.run_cli("--once")
        self.assertEqual(res.returncode, 0, res.stdout + res.stderr)
        self.assertIn("Volume de refill: 0.90", res.stdout)
        self.assertTrue(paths.STATUS_FILE.exists())
        res = self.run_cli("--status")
        self.assertIn("refill: 0.9/1.0", res.stdout)

    def test_running_instance_handoff_and_clean_stop(self):
        a, r = self.fake.issue()
        store.StateStore().update({"access": a, "refresh": r, "expires_at": jwt_exp(a)})
        proc = subprocess.Popen([sys.executable, str(ROOT / "Tasker.pyw"), "--fundo",
                                 "--porta", "18900"], env=self.env(),
                                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        try:
            for _ in range(100):
                if paths.UI_FILE.exists() and paths.STATUS_FILE.exists():
                    break
                time.sleep(0.1)
            self.assertTrue(store.instance_running()[0])
            res = self.run_cli("--once")
            self.assertIn("Checando agora", res.stdout)
            res = self.run_cli("--status")
            self.assertEqual(res.returncode, 0, res.stdout)
            self.assertEqual(self.run_cli("--fundo").stdout.strip(), "O Tasker já está rodando.")
            paths.STOP_FILE.write_text("parar")
            proc.wait(20)
            self.assertEqual(proc.returncode, 0, proc.stderr.read())
            self.assertFalse(paths.UI_FILE.exists())
            self.assertFalse(store.instance_running()[0])
        finally:
            if proc.poll() is None:
                proc.kill()

    def test_bootstrap_and_credentials(self):
        self.assertEqual(self.run_cli("--bootstrap").returncode, 1)
        store.StateStore().update({"refresh": "rt-xyz"})
        self.assertIn("rt-xyz", self.run_cli("--bootstrap").stdout)
        store.save_credentials("1", "2")
        self.assertIn("apagados", self.run_cli("--forget-credentials").stdout)
        self.assertFalse(paths.CREDENTIALS_FILE.exists())


# ===================================================================== sistema
class TestSystem(Base):
    @unittest.skipIf(os.name == "nt", "Linux")
    def test_autostart_and_shortcut_linux(self):
        self.assertFalse(system.autostart_enabled())
        system.set_autostart(True)
        f = Path(os.environ["XDG_CONFIG_HOME"]) / "autostart/tasker.desktop"
        self.assertIn("--fundo", f.read_text())
        self.assertTrue(system.autostart_enabled())
        system.set_autostart(False)
        self.assertFalse(f.exists())
        old = system._linux_desktop_dir
        system._linux_desktop_dir = lambda: None   # não toca na área de trabalho real
        try:
            made = system.create_shortcuts()
        finally:
            system._linux_desktop_dir = old
        self.assertEqual(len(made), 1)
        self.assertIn("Tasker.pyw", Path(made[0]).read_text())


# ======================================================== login pelo navegador
@unittest.skipUnless(system.find_browser(), "sem Chrome/Edge/Chromium")
class TestBrowserLogin(Base):
    def test_captures_tokens_via_devtools(self):
        import http.server
        exp_ms = int((time.time() + 3600) * 1000)
        # Valor embrulhado duas vezes (JSON dentro de string), como o site às vezes faz.
        sess = {"authentication": {"authenticationToken": {
            "accessToken": "tok-a", "refreshToken": "tok-r", "expiresAtTS": exp_ms}}}
        page = ("<!doctype html><title>login</title><script>setTimeout(function(){"
                "sessionStorage.setItem('portal-session', %s)}, 800);</script>"
                % json.dumps(json.dumps(json.dumps(sess)))).encode()

        class H(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", "text/html")
                self.send_header("Content-Length", str(len(page)))
                self.end_headers()
                self.wfile.write(page)

        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        from tasker.browser_login import BrowserLogin
        got = []
        bl = BrowserLogin(Logger(to_file=False, to_stdout=False), got.append,
                          url=f"http://127.0.0.1:{srv.server_address[1]}/login")
        bl.extra_args = ("--headless=new",)
        try:
            bl.start()
            for _ in range(300):
                if not bl.active():
                    break
                time.sleep(0.1)
        finally:
            bl.cancel()
            srv.shutdown()
        self.assertEqual(bl.state, "ok", bl.message)
        self.assertEqual(got[0]["access"], "tok-a")
        self.assertEqual(got[0]["refresh"], "tok-r")


# ====================================================================== fim
class TestZZSentinel(unittest.TestCase):
    def test_real_files_untouched(self):
        self.assertEqual(_fingerprint(), _BEFORE, "um teste mexeu nos arquivos de verdade!")
        self.assertEqual(_leaks(), _LEAKS_BEFORE, "dado de teste vazou pra dados/!")


if __name__ == "__main__":
    try:
        unittest.main(verbosity=1)
    finally:
        shutil.rmtree(TMP, ignore_errors=True)
