#!/usr/bin/env bash
# Linux: abre o Tasker (o mesmo que o atalho do menu). Aceita as flags do Tasker.pyw.
exec python3 "$(dirname "$(readlink -f "$0")")/Tasker.pyw" "$@"
