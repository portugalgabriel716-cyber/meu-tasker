"""Teste de estresse do download: puxa o máximo que a rede der, pra ver o
refill aguentando consumo pesado.

Baixa arquivos grandes de servidores feitos pra teste de velocidade, em várias
conexões paralelas, e joga os bytes fora na memória (nada vai pro disco: sem
desgaste de SSD e quase sem CPU). As conexões são espalhadas entre os
servidores, e um servidor que falha fica de molho um minuto.

Para sozinho no limite de GB ou de tempo — consumir volume é o objetivo, mas
um teste esquecido ligado não pode comer o mês inteiro.
"""
import threading
import time
import urllib.request

MIRRORS = (
    "https://nbg1-speed.hetzner.com/10GB.bin",     # Nuremberg
    "https://fsn1-speed.hetzner.com/10GB.bin",     # Falkenstein
    "https://proof.ovh.net/files/10Gb.dat",        # França
    "https://hel1-speed.hetzner.com/10GB.bin",     # Helsinque
    "http://speedtest.tele2.net/10GB.zip",         # Suécia
)
CHUNK = 256 * 1024
USER_AGENT = "Tasker-stress/2 (+teste de download)"


class StressTest:
    def __init__(self, log, volume_getter=None):
        self.log = log
        self._volume = volume_getter or (lambda: None)
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._threads = []
        self._bytes = 0
        self._samples = []          # (instante, bytes acumulados), últimos segundos
        self._bench = {}            # url -> instante até quando fica de molho
        self.state = "parado"       # parado | rodando | parando
        self.started = self.ended = None
        self.limit_bytes = 0
        self.limit_seconds = 0
        self.streams = 0
        self.active = 0
        self.peak_bps = 0.0
        self.min_volume = None
        self.reason = ""
        self.errors = 0
        self.urls = MIRRORS

    # -- controle ------------------------------------------------------------

    def running(self):
        return self.state in ("rodando", "parando")

    def start(self, streams=6, max_gb=3.0, max_seconds=900, custom_url=""):
        if self.running():
            return "O teste já está rodando."
        self._stop.clear()
        with self._lock:
            self._bytes = 0
            self._samples = [(time.time(), 0)]
        self._bench.clear()
        self.urls = (custom_url,) if custom_url else MIRRORS
        self.streams = max(1, int(streams))
        self.limit_bytes = int(max_gb * 1e9)
        self.limit_seconds = int(max_seconds)
        self.started, self.ended = time.time(), None
        self.peak_bps, self.min_volume, self.errors, self.reason = 0.0, None, 0, ""
        self.state = "rodando"
        self._threads = [threading.Thread(target=self._worker, args=(i,), name=f"stress-{i}",
                                          daemon=True) for i in range(self.streams)]
        for t in self._threads:
            t.start()
        threading.Thread(target=self._watch, name="stress-watch", daemon=True).start()
        self.log(f"Teste de estresse: começando com {self.streams} conexões "
                 f"(para em {max_gb:g} GB ou {max_seconds // 60} min).")
        return "Teste de estresse iniciado — acompanhe o volume caindo e o refill entrando."

    def stop(self, reason="parado por você"):
        if self.state != "rodando":
            return "O teste não está rodando."
        self.reason = reason
        self.state = "parando"
        self._stop.set()
        return "Parando o teste…"

    # -- trabalho ------------------------------------------------------------

    def _pick(self, index, attempt):
        now = time.time()
        live = [u for u in self.urls if self._bench.get(u, 0) <= now] or list(self.urls)
        return live[(index + attempt) % len(live)]

    def _worker(self, index):
        buf = bytearray(CHUNK)
        view = memoryview(buf)
        attempt = 0
        while not self._stop.is_set():
            url = self._pick(index, attempt)
            try:
                req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT,
                                                           "Cache-Control": "no-cache"})
                with urllib.request.urlopen(req, timeout=15) as resp:
                    with self._lock:
                        self.active += 1
                    try:
                        while not self._stop.is_set():
                            n = resp.readinto(view)
                            if not n:
                                break       # arquivo acabou: pega outro
                            with self._lock:
                                self._bytes += n
                    finally:
                        with self._lock:
                            self.active -= 1
            except Exception as e:
                self.errors += 1
                self._bench[url] = time.time() + 60
                attempt += 1
                if self.errors in (1, 10, 50):
                    self.log(f"Teste de estresse: {url.split('/')[2]} falhou "
                             f"({type(e).__name__}) — sigo pelos outros.")
                if self._stop.wait(1):
                    break

    def _watch(self):
        while not self._stop.wait(1):
            now = time.time()
            with self._lock:
                total = self._bytes
                self._samples.append((now, total))
                self._samples = [s for s in self._samples if s[0] >= now - 5]
            speed = self.speed_bps()
            if now - self.started > 5:
                self.peak_bps = max(self.peak_bps, speed)
            vol = self._volume()
            if vol is not None and (self.min_volume is None or vol < self.min_volume):
                self.min_volume = vol
            if self.limit_bytes and total >= self.limit_bytes:
                self.stop(f"chegou no limite de {self.limit_bytes / 1e9:g} GB")
            elif self.limit_seconds and now - self.started >= self.limit_seconds:
                self.stop(f"chegou no limite de {self.limit_seconds // 60} min")
            elif self.errors > 30 and total == 0:
                self.stop("nenhum servidor de teste respondeu")
        for t in self._threads:
            t.join(20)
        self.ended = time.time()
        self.state = "parado"
        self.log("Teste de estresse terminou (" + (self.reason or "parado") + "): " +
                 self.summary_text())

    # -- números -------------------------------------------------------------

    def speed_bps(self):
        with self._lock:
            s = list(self._samples)
        if len(s) < 2 or s[-1][0] - s[0][0] <= 0:
            return 0.0
        return (s[-1][1] - s[0][1]) * 8 / (s[-1][0] - s[0][0])

    def elapsed(self):
        if not self.started:
            return 0.0
        return (self.ended or time.time()) - self.started

    def summary_text(self):
        secs = self.elapsed()
        avg = self._bytes * 8 / secs / 1e6 if secs else 0
        txt = (f"{self._bytes / 1e9:.2f} GB em {int(secs // 60)} min {int(secs % 60)} s "
               f"(média {avg:.0f} Mbit/s, pico {self.peak_bps / 1e6:.0f} Mbit/s)")
        if self.min_volume is not None:
            txt += f", volume mínimo {self.min_volume:.2f} GB"
        return txt + "."

    def info(self):
        secs = self.elapsed()
        return {
            "estado": self.state, "inicio": self.started, "fim": self.ended,
            "segundos": round(secs, 1), "bytes": self._bytes,
            "limite_bytes": self.limit_bytes, "limite_segundos": self.limit_seconds,
            "mbps": round(self.speed_bps() / 1e6, 1) if self.state == "rodando" else 0,
            "media_mbps": round(self._bytes * 8 / secs / 1e6, 1) if secs else 0,
            "pico_mbps": round(self.peak_bps / 1e6, 1),
            "conexoes": self.active, "erros": self.errors,
            "volume_minimo": self.min_volume, "motivo": self.reason,
        }
