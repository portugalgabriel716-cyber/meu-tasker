"""Log: arquivo com rotação + memória curta numerada (a tela puxa por número)."""
import collections
import sys
import threading
import time

from . import paths

LOG_MAX_BYTES = 2 * 1024 * 1024


class Logger:
    def __init__(self, to_file=True, to_stdout=True, keep=600, date=False):
        self.to_file = to_file
        self.to_stdout = to_stdout
        self.date = date
        self._lock = threading.Lock()
        self._lines = collections.deque(maxlen=keep)
        self._seq = 0
        self._quiet = {}

    def __call__(self, msg):
        stamp = time.strftime("%Y-%m-%d %H:%M:%S" if self.date else "%d/%m %H:%M:%S")
        line = f"[{stamp}] {msg}"
        with self._lock:
            self._seq += 1
            self._lines.append((self._seq, line))
            if self.to_file:
                self._write(line)
        if self.to_stdout and sys.stdout is not None:   # pythonw não tem stdout
            try:
                print(line, flush=True)
            except (OSError, ValueError):
                pass

    def once(self, key, msg, every=300):
        """Loga, mas não repete a mesma chave+mensagem antes de `every` segundos.

        É o que segura o log quando uma situação dura (backoff, cooldown, balde
        cheio): antes saía a mesma linha a cada checagem.
        """
        last = self._quiet.get(key)
        if last and last[0] == msg and time.time() - last[1] < every:
            return
        self._quiet[key] = (msg, time.time())
        self(msg)

    def _write(self, line):
        try:
            paths.ensure_data_dir()
            f = paths.LOG_FILE
            if f.exists() and f.stat().st_size > LOG_MAX_BYTES:
                f.replace(f.with_name(f.name + ".1"))
            with open(f, "a", encoding="utf-8") as fh:
                fh.write(line + "\n")
        except OSError:
            pass   # log em disco é conveniência; nunca derruba o monitor

    def since(self, seq):
        """Linhas depois de `seq`. seq<=0 (ou que já saiu da memória) = tudo."""
        with self._lock:
            lines = list(self._lines)
            current = self._seq
        # Número maior que o atual = a tela é de antes de um reinício: manda tudo.
        reset = seq <= 0 or seq > current or (lines and seq < lines[0][0] - 1)
        if not reset:
            lines = [item for item in lines if item[0] > seq]
        return {"seq": current, "reset": bool(reset), "linhas": [t for _, t in lines]}

    def preload(self, n=200):
        """Traz o fim do tasker.log pra memória (a tela abre já com contexto)."""
        try:
            with open(paths.LOG_FILE, "rb") as f:
                f.seek(0, 2)
                start = max(0, f.tell() - 64 * 1024)
                f.seek(start)
                tail = f.read().decode("utf-8", "replace").splitlines()
        except OSError:
            return
        if start:
            tail = tail[1:]   # a primeira linha veio cortada pelo meio
        with self._lock:
            for line in tail[-n:]:
                self._seq += 1
                self._lines.append((self._seq, line))
