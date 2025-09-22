#!/usr/bin/env python3
# ./src/simulate/nmpgc/simulate_paper.py ../heapdumps/sampled
from pathlib import Path
import sys
import subprocess
import os

heapdumps = Path(sys.argv[1])
for benchmark in heapdumps.iterdir():
    if benchmark.is_dir():
        for heapdump in benchmark.iterdir():
            if heapdump.suffix == ".zst":
                gc_number = heapdump.stem.split(".")[-2]
                output_path = f"{benchmark.name}.{gc_number}.log"
                cmd = "./target/release/hwgc_soft {} -o OpenJDK simulate -p 8 -a NMPGC".format(str(heapdump))

                p = subprocess.check_output(
                    cmd.split(),
                    env={
                        "PROTOC": str(Path.home() / "protoc" / "bin" / "protoc"),
                        "PROTOC_INCLUDE": str(Path.home() / "protoc" / "include"),
                        **os.environ,
                    },
                )

                with open(output_path, "wb") as f:
                    f.write(p)
