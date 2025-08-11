#!/usr/bin/env python3
import sys
from pathlib import Path
import os

# Argument will look like
# ./scripts/runbms.py baseline /bin/true ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK trace -t NodeObjref
ROOT = Path(__file__).parent.parent.resolve()
BUILDS = ROOT / "builds"

exe = BUILDS / sys.argv[1]

bms = []
rest_args = []
for arg in sys.argv[3:]:
    if arg.endswith(".binpb.zst"):
        bms.append(arg)
    else:
        rest_args.append(arg)

for bm in bms:
    bm_name = str(Path(bm).parent.stem)
    bm_idx = Path(bm).stem.split(".")[1]
    cmd = "{} {} {} --trace-path {}.{}.json.gz".format(exe, bm, " ".join(rest_args), bm_name, bm_idx)
    os.system(cmd)
