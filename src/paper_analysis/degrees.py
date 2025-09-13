#!/usr/bin/env python3
# ./src/paper_analysis/degrees.py ../heapdumps/sampled
from pathlib import Path
import sys
import subprocess
import os

# Heapdumps are stored in sys.argv[1]
# The heapdumps are group by benchmark names in folders, such as biojava
# Each folder has heapdump in formats like heapdump.5.binpb.zst, where
# 5 is the GC number

# Iterate through all heapdumps and use subprocess to call
# cargo run --release -- -o OpenJDK paper-analyze --analysis-name Degrees --output-path biojava.5.parquet

heapdumps = Path(sys.argv[1])
for benchmark in heapdumps.iterdir():
    if benchmark.is_dir():
        for heapdump in benchmark.iterdir():
            if heapdump.suffix == ".zst":
                gc_number = heapdump.stem.split(".")[-2]
                output_path = f"{benchmark.name}.{gc_number}.parquet"
                cmd = f"cargo run --release -- {heapdump} -o OpenJDK paper-analyze --analysis-name Degrees --output-path {output_path}"

                subprocess.run(
                    cmd.split(),
                    env={
                        "PROTOC": str(Path.home() / "protoc" / "bin" / "protoc"),
                        "PROTOC_INCLUDE": str(Path.home() / "protoc" / "include"),
                        **os.environ,
                    },
                )
