#!/usr/bin/env python3
# ./src/simulate/frontier.py ../heapdumps/sampled
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
                output_path = f"{benchmark.name}.{gc_number}.parquet"
                cmd = f"cargo run --release -- {heapdump} -o OpenJDK simulate -p 32 -a IdealTraceUtilization"

                subprocess.run(
                    cmd.split(),
                    env={
                        "PROTOC": str(Path.home() / "protoc" / "bin" / "protoc"),
                        "PROTOC_INCLUDE": str(Path.home() / "protoc" / "include"),
                        **os.environ,
                    },
                )

                # Rename ideal_trace_utilization_frontier.parquet into output_path
                os.rename("ideal_trace_utilization_frontier.parquet", output_path)