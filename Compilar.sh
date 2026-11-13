#!/usr/bin/env bash
# Gera os dois programas na pasta do projeto: tasker (Linux) e tasker.exe (Windows).
# O .exe precisa de: sudo apt install gcc-mingw-w64-x86-64
#                    rustup target add x86_64-pc-windows-gnu
cd "$(dirname "$(readlink -f "$0")")" || exit 1
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
WIN=x86_64-pc-windows-gnu

cargo build --release || exit 1
# Troca atômica: funciona mesmo com o Tasker rodando.
install -m 755 target/release/tasker tasker.novo && mv -f tasker.novo tasker

cargo build --release --target "$WIN" || exit 1
install -m 755 "target/$WIN/release/tasker.exe" tasker.exe

ls -lh tasker tasker.exe
