#!/usr/bin/env python3
import sys
from pathlib import Path
import os

ROOT = Path(__file__).parent.parent.resolve()
BUILDS = ROOT / "builds"

exe = BUILDS / sys.argv[1]
cmd = "{} {}".format(exe, " ".join(sys.argv[3:]))
os.system(cmd)
