#!/usr/bin/env python3
"""Tasker — refill automático da Lidl Connect.

Duplo clique (Windows) ou o atalho do menu (Linux) abre o Tasker. Na linha de
comando: `python Tasker.pyw --help`. Precisa só do Python 3.8+, nada mais.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from tasker.app import main  # noqa: E402

if __name__ == "__main__":
    sys.exit(main())
