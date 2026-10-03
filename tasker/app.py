"""Ponto de entrada: a janela (padrão), os modos de servidor e a linha de comando.

    python Tasker.pyw                 abre o Tasker (se já estiver aberto, só a janela)
    python Tasker.pyw --fundo         sobe sem janela (iniciar com o sistema)
    python Tasker.pyw --loop          só o motor, log no terminal (VM/Docker)
    python Tasker.pyw --once          uma checagem e sai (cron/GitHub Actions)
    python Tasker.pyw --status        mostra o volume e sai
    python Tasker.pyw --login         login pelo navegador e sai
    python Tasker.pyw --set-credentials | --forget-credentials
    python Tasker.pyw --test-notification | --bootstrap

Um processo só faz tudo: o motor roda numa thread e a tela é servida pelo
mesmo processo. Fechar a janela não para nada — clicar no atalho de novo só
reabre a janela. Pra encerrar de vez: botão "Sair" (ou tasker.stop, ou sinal).
"""
import argparse
import json
import signal
import sys
import threading
import time
import urllib.request

from . import __version__, config, engine, paths, store, system
from .browser_login import BrowserLogin
from .engine import Monitor
from .logs import Logger
from .notify import Notifier
from .stress import StressTest

_LOCAL = urllib.request.build_opener(urllib.request.ProxyHandler({}))   # sem proxy


class App:
    def __init__(self, log):
        self.log = log
        self.monitor = Monitor(log)
        self.monitor.notifier = Notifier(lambda: self.monitor.cfg, log)
        self.login = BrowserLogin(log, self._browser_tokens)
        self.stress = StressTest(log, lambda: (self.monitor.status.get("volume") or {}).get("left"))
        self.server = None
        self._browser = system.find_browser()
        self._today_cache = (None, None, 0)

    def _browser_tokens(self, data):
        exp = (float(data["expiresAtTS"]) / 1000) if data.get("expiresAtTS") else None
        self.monitor.adopt_tokens(data["access"], data.get("refresh"), exp)

    # -- tela ----------------------------------------------------------------

    def _refills_today(self):
        try:
            mtime = paths.REFILLS_FILE.stat().st_mtime
        except OSError:
            mtime = 0
        day = time.strftime("%Y-%m-%d")
        if self._today_cache[:2] != (mtime, day):
            from .server import history
            self._today_cache = (mtime, day, history(24)["resumo"]["refills_hoje"])
        return self._today_cache[2]

    def state(self):
        snap = self.monitor.snapshot()
        snap["refills_hoje"] = self._refills_today()
        snap["limite"] = self.monitor.cfg["gb_threshold"]
        stress = self.stress.info()
        stress["cfg_gb"] = self.monitor.cfg["stress_max_gb"]
        stress["cfg_min"] = self.monitor.cfg["stress_max_minutes"] // 60
        if stress["inicio"]:
            stress["refills"], stress["falhas"] = engine.refills_since(stress["inicio"])
        return {"monitor": snap, "login_navegador": self.login.info(), "estresse": stress,
                "sistema": {"versao": __version__, "autostart": system.autostart_enabled(),
                            "navegador": bool(self._browser), "pasta": str(paths.DATA_DIR),
                            "windows": store.IS_WINDOWS}}

    def action(self, body):
        act = body.get("acao")
        m = self.monitor
        if act == "checar":
            if not m.running():
                return {"msg": "O monitor está desligado — ligue primeiro."}
            if engine.pause_state()[0]:
                return {"msg": "Está pausado — retome primeiro."}
            m.poke()
            return {"msg": "Checando agora…"}
        if act == "pausar":
            try:
                minutes = max(0, int(body.get("minutos") or 0))
            except (TypeError, ValueError):
                minutes = 0
            until = engine.set_pause(minutes)
            m.poke()
            when = time.strftime("%H:%M", time.localtime(until)) if until else "você retomar"
            return {"msg": f"Pausado até {when}: o Tasker não toca na sessão — pode "
                           "usar a conta no celular à vontade."}
        if act == "retomar":
            engine.clear_pause()
            m.poke()
            return {"msg": "Retomando."}
        if act == "ligar":
            return {"msg": "Monitor ligado." if m.start() else "Já estava ligado."}
        if act == "desligar":
            threading.Thread(target=m.stop, daemon=True).start()
            return {"msg": "Desligando o monitor (a tela continua aberta)."}
        if act == "testar_aviso":
            n = m.notifier.send("Teste do Tasker: se você está lendo isto, os avisos funcionam.")
            return {"msg": "Aviso de teste disparado — chegou?" if n else
                    "Os avisos estão todos desligados nos ajustes."}
        if act == "login_senha":
            msisdn = str(body.get("msisdn") or "").strip()
            senha = str(body.get("senha") or "")
            if not msisdn or not senha:
                return {"ok": False, "msg": "Preencha número e senha."}
            with m.lock:
                ok, msg = m.password_login(auto=False, msisdn=msisdn, password=senha)
            if ok and body.get("lembrar"):
                store.save_credentials(msisdn, senha)
                msg += " Número e senha guardados pro login automático."
            return {"ok": ok, "msg": msg}
        if act == "login_navegador":
            return {"msg": self.login.start()}
        if act == "login_cancelar":
            self.login.cancel()
            return {"msg": "Cancelando…"}
        if act == "credenciais_apagar":
            return {"msg": "Número e senha apagados." if store.forget_credentials()
                    else "Não havia nada guardado."}
        if act == "padroes":
            config.save(config.defaults())
            m.reload_config()
            return {"msg": "Ajustes de volta ao padrão."}
        if act == "autostart":
            try:
                return {"msg": system.set_autostart(bool(body.get("ligado")))}
            except OSError as e:
                return {"msg": f"Não consegui mudar: {e}"}
        if act == "atalho":
            made = system.create_shortcuts()
            return {"msg": ("Atalho criado: " + " · ".join(made)) if made else
                    "Não consegui criar o atalho."}
        if act == "estresse_iniciar":
            c = m.cfg
            msg = self.stress.start(c["stress_streams"], c["stress_max_gb"],
                                    c["stress_max_minutes"], c["stress_url"])
            m.poke()   # leitura nova já, pra o ritmo de consumo começar a contar
            return {"msg": msg}
        if act == "estresse_parar":
            return {"msg": self.stress.stop()}
        if act == "sair":
            threading.Timer(0.3, self.shutdown).start()
            return {"msg": "Encerrando o Tasker…"}
        return {"erro": "Ação desconhecida."}

    # -- execução ------------------------------------------------------------

    def run_gui(self, open_window=True, port=8765):
        from .server import UiServer
        self.server = UiServer(self, port)
        self.server.publish()
        stop = threading.Event()
        engine.watch_control_files(self.monitor, self.shutdown, stop)
        self.monitor.start()
        self.log(f"Tasker {__version__} — tela em http://127.0.0.1:{self.server.port} "
                 "(abra pelo atalho).")
        if open_window:
            system.open_window(self.server.boot_url())
        try:
            self.server.serve()
        finally:
            stop.set()
            self.login.cancel()
            self.stress.stop("o Tasker foi encerrado")
            self.monitor.stop()
            self.server.unpublish()
            self.log("Tasker encerrado.")

    def shutdown(self):
        if self.server:
            self.server.shutdown()
        else:
            self.monitor.stop(wait=0)


# ------------------------------------------------------------ instância rodando

def _ask_running(path, body=None):
    """Fala com a instância que já está rodando. None se não houver tela."""
    info = store.read_json(paths.UI_FILE, {})
    if not info.get("port") or not info.get("key"):
        return None
    req = urllib.request.Request(
        f"http://127.0.0.1:{info['port']}{path}",
        data=json.dumps(body).encode() if body is not None else None,
        headers={"X-Tasker-Key": info["key"], "Content-Type": "application/json"})
    try:
        with _LOCAL.open(req, timeout=8) as r:
            return json.loads(r.read().decode())
    except Exception:
        return None


def _open_running_window():
    res = _ask_running("/api/boot", {})
    if not res or not res.get("boot"):
        return False
    port = store.read_json(paths.UI_FILE, {})["port"]
    system.open_window(f"http://127.0.0.1:{port}/?boot={res['boot']}")
    return True


def _install_signals(handler):
    for name in ("SIGTERM", "SIGINT", "SIGBREAK"):
        if hasattr(signal, name):
            try:
                signal.signal(getattr(signal, name), lambda *_: handler())
            except (ValueError, OSError):
                pass


def _console():
    try:
        return sys.stdout is not None and sys.stdout.isatty()
    except (ValueError, OSError):
        return False


def _fmt_status(snap):
    v = snap.get("volume") or {}
    lines = []
    if v:
        lines.append(f"Refill: {v['left']:.2f}/{v.get('total') or 0:g} GB"
                     + (f" | plano: {v['plan_left']:g} GB" if v.get("plan_left") is not None else ""))
    if snap.get("ultima_checagem"):
        lines.append("Última checagem: " + time.strftime(
            "%d/%m %H:%M:%S", time.localtime(snap["ultima_checagem"])))
    lines.append(f"Fase: {snap.get('fase')}")
    if snap.get("precisa_login"):
        lines.append(snap["precisa_login"])
    if snap.get("erro"):
        lines.append(f"Último erro: {snap['erro']}")
    return "\n".join(lines)


# -------------------------------------------------------------------- main

def main(argv=None):
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError, OSError):
        pass
    p = argparse.ArgumentParser(prog="tasker", description="Refill automático da Lidl Connect")
    mode = p.add_mutually_exclusive_group()
    mode.add_argument("--fundo", action="store_true", help="sobe sem abrir a janela")
    mode.add_argument("--loop", action="store_true", help="só o motor, sem tela (servidor/Docker)")
    mode.add_argument("--once", action="store_true", help="uma checagem e sai (cron)")
    mode.add_argument("--status", action="store_true", help="mostra o volume e sai")
    mode.add_argument("--login", action="store_true", help="login pelo navegador e sai")
    mode.add_argument("--set-credentials", action="store_true", help="guarda número e senha")
    mode.add_argument("--forget-credentials", action="store_true", help="apaga número e senha")
    mode.add_argument("--test-notification", action="store_true", help="manda um aviso de teste")
    mode.add_argument("--bootstrap", action="store_true",
                      help="mostra o refresh token (pra levar a sessão pra um servidor)")
    p.add_argument("--porta", type=int, default=8765, help="porta da tela (padrão 8765)")
    args = p.parse_args(argv)

    moved = paths.migrate_legacy()
    paths.ensure_data_dir()

    if args.forget_credentials:
        print("Número e senha apagados." if store.forget_credentials() else "Não havia nada guardado.")
        return 0
    if args.set_credentials:
        import getpass
        print("Com número e senha guardados, o Tasker refaz o login sozinho quando a")
        print(f"sessão cair. Ficam em {paths.CREDENTIALS_FILE}, legível só pelo seu usuário.\n")
        msisdn = input("Número (Mobilfunknummer): ").strip()
        password = getpass.getpass("Senha (não aparece): ") if msisdn else ""
        if not msisdn or not password:
            print("Cancelado.")
            return 1
        store.save_credentials(msisdn, password)
        store.StateStore().update({"login_fails": 0, "login_blocked_until": 0})
        print("Guardado.")
        return 0
    if args.test_notification:
        n = Notifier(config.load).send("Teste do Tasker: se você está lendo isto, os avisos funcionam.")
        time.sleep(3 if n else 0)   # as threads de aviso precisam de um instante
        print("Aviso de teste disparado." if n else "Os avisos estão desligados nos ajustes.")
        return 0
    if args.bootstrap:
        rt = store.StateStore().load().get("refresh")
        if not rt:
            print("Não há sessão salva. Faça login no Tasker primeiro.")
            return 1
        print("Refresh token (guarde como segredo TASKER_REFRESH_TOKEN no servidor):\n")
        print(rt)
        print("\nO refresh token ROTACIONA e a conta aceita UMA sessão: depois que o")
        print("servidor assumir, feche o Tasker daqui (e vice-versa).")
        return 0

    lock = store.acquire_instance_lock()
    if lock is None:
        return _with_running_instance(args)

    gui = not (args.loop or args.once or args.status or args.login)
    log = Logger(to_stdout=_console() or not gui, date=args.loop or args.once)
    if gui:
        log.preload()
    if moved:
        log(f"Arquivos da versão 1 movidos pra {paths.DATA_DIR.name}/: {', '.join(moved)}.")
    app = App(log)
    try:
        if args.status:
            with app.monitor.lock:
                app.monitor.load_state()
                token = app.monitor.ensure_token()
                if not token:
                    print("Sem sessão válida — faça login no Tasker.")
                    return 1
                units, cooldown, err = app.monitor.read(token)
            if err:
                print(f"Não consegui ler o volume: {err}")
                return 1
            for key, unit in units.items():
                print(f"{key}: {unit.get('left')}/{unit.get('max')} {unit.get('unit')}")
            print(f"cooldown do refill: {cooldown}")
            return 0
        if args.login:
            print(app.login.start())
            while app.login.active():
                time.sleep(0.5)
            print(app.login.message)
            return 0 if app.login.state == "ok" else 1
        if args.once:
            app.monitor.run(once=True)
            snap = app.monitor.snapshot()
            return 1 if snap.get("erro") or snap.get("precisa_login") else 0
        if args.loop:
            stop = threading.Event()
            _install_signals(lambda: app.monitor.stop(wait=0))
            engine.watch_control_files(app.monitor, lambda: app.monitor.stop(wait=0), stop)
            app.monitor.run()
            stop.set()
            return 0
        _install_signals(app.shutdown)
        if hasattr(signal, "SIGHUP"):
            # Aberto por um terminal: fechar o terminal não derruba o Tasker
            # (ele é um programa de segundo plano; quem encerra é o "Sair").
            signal.signal(signal.SIGHUP, signal.SIG_IGN)
        app.run_gui(open_window=not args.fundo, port=args.porta)
        return 0
    finally:
        lock.close()


def _with_running_instance(args):
    """Já tem um Tasker com esta sessão: conversa com ele em vez de brigar."""
    if args.fundo:
        print("O Tasker já está rodando.")
        return 0
    if args.once:
        res = _ask_running("/api/acao", {"acao": "checar"})
        print(res.get("msg") if res else "Já tem outra execução nesta sessão — saindo.")
        return 0   # pro cron, isso não é erro
    if args.status:
        snap = store.read_json(paths.STATUS_FILE, {})
        print(_fmt_status(snap) if snap else "O Tasker está rodando, mas ainda sem leitura.")
        last = snap.get("ultima_checagem") or 0
        return 0 if time.time() - last < 15 * 60 else 1   # healthcheck do Docker
    if args.login:
        res = _ask_running("/api/acao", {"acao": "login_navegador"})
        print(res.get("msg") if res else "O Tasker está rodando sem tela: pare ele antes.")
        return 0 if res else 1
    if args.loop:
        print("Já tem um Tasker rodando com esta sessão — saindo.")
        return 1
    if _open_running_window():
        return 0
    print("O Tasker já está rodando, mas não consegui abrir a janela dele.")
    return 1
