"""python -m scripts.govd entry point."""
from __future__ import annotations

import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

import gate_rc

from .cli import main

if __name__ == "__main__":
    sys.exit(gate_rc.run(lambda: main(sys.argv[1:])))

