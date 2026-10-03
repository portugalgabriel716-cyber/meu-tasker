"""Onde mora cada arquivo. Código fica na raiz; tudo que é estado fica em dados/.

TASKER_DATA_DIR troca a pasta de dados inteira (o Docker usa /data, os testes
usam uma pasta temporária). TASKER_STATE aponta só o arquivo de sessão — existe
pela compatibilidade com quem já rodava a versão cloud (cloud_state.json).
TASKER_CONFIG aponta outro arquivo de ajustes.

Os outros módulos leem `paths.ALGUMA_COISA` na hora do uso (nunca `from paths
import`), assim `set_data_dir()` vale pra todo mundo.
"""
import os
import shutil
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
UI_DIR = Path(__file__).resolve().parent / "ui"
DEFAULT_DATA_DIR = BASE_DIR / "dados"


def set_data_dir(data_dir=None):
    global DATA_DIR, CONFIG_FILE, STATE_FILE, LOCK_FILE, CREDENTIALS_FILE
    global LOG_FILE, UI_FILE, STATUS_FILE, PAUSE_FILE, STOP_FILE, POKE_FILE
    global REFILLS_FILE, VOLUME_FILE
    DATA_DIR = Path(data_dir or os.environ.get("TASKER_DATA_DIR") or DEFAULT_DATA_DIR)
    CONFIG_FILE = Path(os.environ.get("TASKER_CONFIG") or DATA_DIR / "config.json")
    STATE_FILE = Path(os.environ.get("TASKER_STATE") or DATA_DIR / "sessao.json")
    # O lock é por arquivo de sessão: o que não pode existir em dobro é quem
    # gira a corrente de refresh tokens.
    LOCK_FILE = STATE_FILE.with_name(STATE_FILE.stem + ".lock")
    CREDENTIALS_FILE = DATA_DIR / "credenciais.json"
    LOG_FILE = DATA_DIR / "tasker.log"
    UI_FILE = DATA_DIR / "tela.json"          # porta + chave da tela aberta
    STATUS_FILE = DATA_DIR / "status.json"    # retrato do último ciclo
    PAUSE_FILE = DATA_DIR / "tasker.pause"    # {"until": ts}; 0 = até retomarem
    STOP_FILE = DATA_DIR / "tasker.stop"      # existir = encerrar limpo
    POKE_FILE = DATA_DIR / "tasker.poke"      # existir = checar agora
    REFILLS_FILE = DATA_DIR / "refills.jsonl"
    VOLUME_FILE = DATA_DIR / "volume.jsonl"


set_data_dir()


def ensure_data_dir():
    DATA_DIR.mkdir(parents=True, exist_ok=True)


# Nome antigo (na raiz, versão 1) -> atributo novo em paths.
_LEGACY = {
    "session_tokens.json": "STATE_FILE",
    "config.json": "CONFIG_FILE",
    "credentials.json": "CREDENTIALS_FILE",
    "refill_history.jsonl": "REFILLS_FILE",
    "volume_history.jsonl": "VOLUME_FILE",
    "tasker.log": "LOG_FILE",
}


def migrate_legacy(base=None):
    """Traz os arquivos da versão 1 (soltos na raiz) pra dados/. Devolve a lista.

    Só move quando o destino ainda não existe: rodar duas vezes não faz nada, e
    nunca um arquivo antigo passa por cima de um novo. A sessão vem junto — é o
    que deixa a versão nova continuar logada sem pedir login.
    """
    if base is None:
        if DATA_DIR != DEFAULT_DATA_DIR:
            return []   # pasta de dados trocada (Docker, testes): nada a migrar
        base = BASE_DIR
    ensure_data_dir()
    moved = []
    for old_name, attr in _LEGACY.items():
        old = Path(base) / old_name
        new = globals()[attr]
        if old.exists() and not new.exists():
            shutil.move(str(old), str(new))
            moved.append(old_name)
    old_log1 = Path(base) / "tasker.log.1"
    if old_log1.exists() and not LOG_FILE.with_name("tasker.log.1").exists():
        shutil.move(str(old_log1), str(LOG_FILE.with_name("tasker.log.1")))
        moved.append("tasker.log.1")
    return moved
