#!/usr/bin/env bash
# Linux: compila quando o código mudou e abre o Tasker. Aceita as flags do programa.
cd "$(dirname "$(readlink -f "$0")")" || exit 1
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if command -v cargo >/dev/null 2>&1 && { [ ! -x tasker ] || [ -n "$(find src assets Cargo.toml Cargo.lock -newer tasker -print -quit)" ]; }; then
    cargo build --release || exit 1
    # Troca atômica: funciona mesmo com o Tasker rodando.
    install -m 755 target/release/tasker tasker.novo && mv -f tasker.novo tasker
fi
exec ./tasker "$@"
