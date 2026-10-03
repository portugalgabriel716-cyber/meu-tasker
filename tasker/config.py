"""Ajustes: um lugar só descreve cada opção (rótulo, explicação, padrão, limites).

Precedência:  padrão do código  <  dados/config.json  <  variável TASKER_*

O ambiente vence de propósito: mexer na tela nunca pode desligar um TASKER_*
posto num serviço. Valores são guardados em segundos/GB/número puro; `unit` diz
só como a tela mostra.

As chaves são as mesmas da versão 1, então o config.json antigo vale como está.
As que existiam só por causa do navegador (keep-alive do portal, headless,
reabrir navegador...) são ignoradas na leitura e somem na próxima gravação.
"""
import json
import os
import re

from . import paths

SETTINGS = [
    # ------------------------------------------------------------- checagem
    {
        "key": "check_interval", "env": "TASKER_CHECK_INTERVAL",
        "default": 60, "type": "int", "unit": "s", "min": 15, "max": 1800,
        "group": "Checagem", "label": "Olhar o volume a cada",
        "help": "Intervalo normal entre as perguntas à Lidl. Com a checagem "
                "adaptativa ligada, ele encurta sozinho quando o volume está "
                "acabando rápido.",
    },
    {
        "key": "adaptive", "env": "TASKER_ADAPTIVE",
        "default": True, "type": "bool",
        "group": "Checagem", "label": "Checagem adaptativa",
        "help": "Mede o ritmo de consumo e, quando o volume vai cruzar o limite "
                "antes da próxima checagem, olha mais cedo. O refill sai perto do "
                "limite em vez de bem abaixo dele.",
    },
    {
        "key": "fast_interval", "env": "TASKER_FAST_INTERVAL",
        "default": 15, "type": "int", "unit": "s", "min": 5, "max": 300,
        "group": "Checagem", "label": "Intervalo mais curto permitido",
        "help": "O mínimo que a checagem adaptativa pode chegar. Também é o "
                "ritmo usado pra confirmar um refill recém-pedido.",
    },
    {
        "key": "gb_threshold", "env": "TASKER_GB_THRESHOLD",
        "default": 0.85, "type": "float", "unit": "GB", "min": 0.05, "max": 5.0,
        "group": "Checagem", "label": "Pedir refill quando sobrar",
        "help": "Quando o volume restante chegar nesse valor (ou menos), o "
                "Tasker dispara o Refill. O pacote é de 1 GB.",
    },
    {
        "key": "max_without_refill", "env": "TASKER_MAX_WITHOUT_REFILL",
        "default": 13 * 60, "type": "int", "unit": "min",
        "min": 5 * 60, "max": 12 * 3600,
        "group": "Checagem", "label": "Forçar refill se passar",
        "help": "Rede parada não gasta volume, e aí o limite acima nunca chega. "
                "Passado esse tempo o Tasker pede refill mesmo assim (com o balde "
                "cheio ele só zera o relógio).",
    },

    # ------------------------------------------------------ quando dá errado
    {
        "key": "pending_grace", "env": "TASKER_PENDING_GRACE",
        "default": 150, "type": "int", "unit": "s", "min": 30, "max": 900,
        "group": "Quando dá errado", "label": "Esperar o refill aparecer por",
        "help": "A Lidl responde erro 500 mesmo quando o refill deu certo, então "
                "quem decide é o volume subir. Este é o prazo pra ele subir antes "
                "de contar como falha.",
    },
    {
        "key": "refill_backoff_max", "env": "TASKER_REFILL_BACKOFF_MAX",
        "default": 30 * 60, "type": "int", "unit": "min",
        "min": 60, "max": 4 * 3600,
        "group": "Quando dá errado", "label": "Espera máxima após refill falhar",
        "help": "A cada falha seguida a espera dobra, até esse teto.",
    },
    {
        "key": "token_margin", "env": "TASKER_TOKEN_MARGIN",
        "default": 5 * 60, "type": "int", "unit": "min",
        "min": 60, "max": 45 * 60,
        "group": "Quando dá errado", "label": "Renovar o token com antecedência de",
        "help": "O token da API dura 1 h. Renovar um pouco antes evita a corrida "
                "entre expirar e usar.",
    },
    {
        "key": "refresh_retry_delay", "env": "TASKER_REFRESH_RETRY_DELAY",
        "default": 60, "type": "int", "unit": "s", "min": 10, "max": 900,
        "group": "Quando dá errado", "label": "Espera pra tentar o token de novo",
        "help": "Depois de uma renovação que falhou por rede/instabilidade. "
                "Token recusado de vez espera bem mais (30 min).",
    },

    # ---------------------------------------------------------------- login
    {
        "key": "login_backoff", "env": "TASKER_LOGIN_BACKOFF",
        "default": 5 * 60, "type": "int", "unit": "min",
        "min": 60, "max": 4 * 3600,
        "group": "Login automático", "label": "Espera entre tentativas de login",
        "help": "Só vale com número e senha guardados. A espera dobra a cada "
                "falha.",
    },
    {
        "key": "login_max_fails", "env": "TASKER_LOGIN_MAX_FAILS",
        "default": 3, "type": "int", "unit": "x", "min": 1, "max": 5,
        "group": "Login automático", "label": "Senhas erradas antes de parar",
        "help": "A Lidl trava a conta depois de algumas senhas erradas. Este "
                "limite garante que o Tasker nunca seja o motivo — deixe baixo.",
    },
    {
        "key": "login_block_long", "env": "TASKER_LOGIN_BLOCK_LONG",
        "default": 6 * 3600, "type": "int", "unit": "h",
        "min": 3600, "max": 48 * 3600,
        "group": "Login automático", "label": "Parada longa após o limite",
        "help": "Depois de estourar o limite acima, o Tasker fica esse tempo sem "
                "tentar login e avisa você.",
    },

    # ------------------------------------------------------ teste de estresse
    {
        "key": "stress_streams", "env": "TASKER_STRESS_STREAMS",
        "default": 6, "type": "int", "unit": "x", "min": 1, "max": 32,
        "group": "Teste de estresse", "label": "Conexões paralelas",
        "help": "Quantos downloads ao mesmo tempo. Mais conexões enchem melhor uma "
                "rede rápida; 6 costuma bastar pra 4G/5G.",
    },
    {
        "key": "stress_max_gb", "env": "TASKER_STRESS_MAX_GB",
        "default": 3.0, "type": "float", "unit": "GB", "min": 0.1, "max": 200.0,
        "group": "Teste de estresse", "label": "Parar depois de baixar",
        "help": "O teste consome volume de verdade (é o objetivo). Ele para sozinho "
                "ao chegar aqui.",
    },
    {
        "key": "stress_max_minutes", "env": "TASKER_STRESS_MAX_MINUTES",
        "default": 15 * 60, "type": "int", "unit": "min", "min": 60, "max": 6 * 3600,
        "group": "Teste de estresse", "label": "Parar depois de",
        "help": "Limite de tempo, valendo o que vier primeiro com o de cima.",
    },
    {
        "key": "stress_url", "env": "TASKER_STRESS_URL",
        "default": "", "type": "str", "max_len": 500,
        "pattern": r"|https?://\S+",
        "group": "Teste de estresse", "label": "Arquivo de teste próprio (opcional)",
        "help": "Vazio = servidores de teste de velocidade da Hetzner, OVH e Tele2 "
                "(arquivos de 10 GB, revezando). Ponha uma URL se quiser usar outro.",
    },

    # --------------------------------------------------------------- avisos
    {
        "key": "notify", "env": "TASKER_NOTIFY",
        "default": True, "type": "bool",
        "group": "Avisos", "label": "Avisos na área de trabalho",
        "help": "Notificação quando precisa de login ou o refill falha várias "
                "vezes seguidas.",
    },
    {
        "key": "ntfy_topic", "env": "TASKER_NTFY_TOPIC",
        "default": "", "type": "str", "max_len": 200,
        "pattern": r"[A-Za-z0-9_-]*|https?://\S+",
        "group": "Avisos", "label": "Tópico ntfy.sh (push no celular)",
        "help": "Opcional. Assine no app ntfy um tópico com nome difícil de "
                "adivinhar e escreva o mesmo nome aqui (ou a URL de um servidor "
                "ntfy seu). Tópico ntfy não tem senha: quem souber o nome lê.",
    },
    {
        "key": "webhook", "env": "TASKER_WEBHOOK",
        "default": "", "type": "str", "max_len": 500,
        "pattern": r"|https?://\S+",
        "group": "Avisos", "label": "Webhook (opcional)",
        "help": "Recebe um POST JSON {\"text\": \"...\"} a cada aviso — serve pra "
                "Slack, Discord (/slack), Telegram via ponte, etc.",
    },
]

BY_KEY = {s["key"]: s for s in SETTINGS}
GROUPS = list(dict.fromkeys(s["group"] for s in SETTINGS))

_UNIT_SECONDS = {"s": 1, "min": 60, "h": 3600}
UNIT_LABEL = {"s": "segundos", "min": "minutos", "h": "horas", "GB": "GB", "x": "vezes"}


def defaults():
    return {s["key"]: s["default"] for s in SETTINGS}


def _truthy(v):
    if isinstance(v, bool):
        return v
    return str(v).strip().lower() in ("1", "true", "yes", "on", "sim")


def coerce(spec, raw):
    """Texto/JSON -> valor do tipo certo, dentro dos limites. Levanta ValueError."""
    if spec["type"] == "bool":
        return _truthy(raw)
    if spec["type"] == "str":
        val = str(raw or "").strip()
        if len(val) > spec.get("max_len", 200):
            raise ValueError(f"máximo de {spec.get('max_len', 200)} caracteres")
        if val and spec.get("pattern") and not re.fullmatch(spec["pattern"], val):
            raise ValueError("formato inválido")
        return val
    if isinstance(raw, str):
        raw = raw.strip().replace(",", ".")   # quem digita em português usa vírgula
        if not raw:
            raise ValueError("está vazio")
    if isinstance(raw, bool):
        raise ValueError("não é um número")
    try:
        val = int(float(raw)) if spec["type"] == "int" else float(raw)
    except (TypeError, ValueError):
        raise ValueError(f"{raw!r} não é um número")
    lo, hi = spec.get("min"), spec.get("max")
    if lo is not None and val < lo:
        raise ValueError(f"mínimo é {fmt(spec, lo)}")
    if hi is not None and val > hi:
        raise ValueError(f"máximo é {fmt(spec, hi)}")
    return val


def to_display(spec, value):
    if spec["type"] in ("bool", "str"):
        return value
    div = _UNIT_SECONDS.get(spec.get("unit", ""))
    if not div or div == 1:
        return value
    val = value / div
    return int(val) if spec["type"] == "int" and val == int(val) else round(val, 2)


def from_display(spec, shown):
    if spec["type"] in ("bool", "str"):
        return coerce(spec, shown)
    if isinstance(shown, str):
        shown = shown.strip().replace(",", ".")
        if not shown:
            raise ValueError("está vazio")
    try:
        num = float(shown)
    except (TypeError, ValueError):
        raise ValueError(f"{shown!r} não é um número")
    return coerce(spec, num * _UNIT_SECONDS.get(spec.get("unit", ""), 1))


def fmt(spec, value):
    if spec["type"] == "bool":
        return "ligado" if value else "desligado"
    if spec["type"] == "str":
        return value or "vazio"
    suffix = {"s": " s", "min": " min", "h": " h", "GB": " GB", "x": "x"}
    return f"{to_display(spec, value)}{suffix.get(spec.get('unit', ''), '')}"


def read_file(path=None):
    """Só o que está no arquivo. Corrompido/ausente = {} (nunca impede de subir)."""
    try:
        with open(path or paths.CONFIG_FILE, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def load(path=None, env=None):
    return load_with_sources(path, env)[0]


def load_with_sources(path=None, env=None):
    """(valores, origens) — origem é 'padrão', 'arquivo' ou 'ambiente'."""
    env = os.environ if env is None else env
    arquivo = read_file(path)
    values, sources = {}, {}
    for spec in SETTINGS:
        key = spec["key"]
        values[key], sources[key] = spec["default"], "padrão"
        if key in arquivo:
            try:
                values[key], sources[key] = coerce(spec, arquivo[key]), "arquivo"
            except ValueError:
                pass
        raw = env.get(spec["env"], "")
        if str(raw).strip():
            try:
                values[key], sources[key] = coerce(spec, raw), "ambiente"
            except ValueError:
                pass
    return values, sources


def save(values, path=None):
    """Grava só chaves conhecidas e válidas (as obsoletas da v1 caem aqui)."""
    from .store import write_json
    clean = {s["key"]: coerce(s, values[s["key"]]) for s in SETTINGS if s["key"] in values}
    write_json(path or paths.CONFIG_FILE, clean)
    return clean


def warnings(values):
    """Combinações que passam na validação mas se mordem."""
    out = []
    if values["token_margin"] >= 55 * 60:
        out.append("O token dura 1 h — com essa margem ele seria renovado sem parar.")
    if values["gb_threshold"] >= 1.0:
        out.append("O pacote da Lidl é de 1 GB — pedir refill com 1 GB ou mais "
                   "sobrando dispara sempre.")
    if values["fast_interval"] > values["check_interval"]:
        out.append("O intervalo mais curto é maior que o normal: a checagem "
                   "adaptativa não tem como encurtar nada.")
    if values["fast_interval"] * 3 > values["pending_grace"]:
        out.append("O prazo pra confirmar o refill é curto pro intervalo rápido: "
                   "sobram menos de 3 conferências antes de contar como falha.")
    return out
