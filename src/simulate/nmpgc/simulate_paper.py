#!/usr/bin/env python3
# ./src/simulate/nmpgc/simulate_paper.py ../heapdumps/sampled
from pathlib import Path
import sys
import subprocess
import os
import time

heapdumps = Path(sys.argv[1])
max_workers = max(1, os.cpu_count() // 2)

# Build environment: conditionally add protoc paths.
env = dict(os.environ)
protoc_dir = Path.home() / "protoc"
if protoc_dir.is_dir():
    env["PROTOC"] = str(protoc_dir / "bin" / "protoc")
    env["PROTOC_INCLUDE"] = str(protoc_dir / "include")

# Collect all jobs.
jobs = []
for benchmark in heapdumps.iterdir():
    if benchmark.is_dir():
        for heapdump in benchmark.iterdir():
            if heapdump.suffix == ".zst":
                gc_number = heapdump.stem.split(".")[-2]
                output_path = f"{benchmark.name}.{gc_number}.log"
                cmd = (
                    "./target/release/hwgc_soft {} -o OpenJDK simulate"
                    " -p 8 -a NMPGC --use-dramsim3 --page-size TwoMB"
                ).format(str(heapdump))
                jobs.append((cmd, output_path))

total = len(jobs)
if total == 0:
    print("No .zst heapdumps found.")
    sys.exit(0)

print(f"Running {total} simulations with {max_workers} workers")

# Dispatch jobs with a bounded pool.
running = {}  # proc -> (cmd, output_path)
pending = list(jobs)
done = 0
start_time = time.monotonic()

def print_progress():
    elapsed = time.monotonic() - start_time
    bar_len = 40
    filled = int(bar_len * done / total)
    bar = "█" * filled + "░" * (bar_len - filled)
    if done > 0:
        eta_secs = elapsed / done * (total - done)
        m, s = divmod(int(eta_secs), 60)
        eta = f"{m}m{s:02d}s"
    else:
        eta = "..."
    print(f"\r[{bar}] {done}/{total}  ETA {eta}  ", end="", flush=True)

print_progress()

while pending or running:
    # Fill worker slots.
    while pending and len(running) < max_workers:
        cmd, output_path = pending.pop()
        proc = subprocess.Popen(
            cmd.split(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        running[proc] = (cmd, output_path)

    # Poll for completion.
    finished = []
    for proc in list(running):
        ret = proc.poll()
        if ret is not None:
            finished.append(proc)
    for proc in finished:
        cmd, output_path = running.pop(proc)
        stdout, stderr = proc.communicate()
        if proc.returncode != 0:
            print(f"\nFAILED ({proc.returncode}): {cmd}")
            if stderr:
                print(stderr.decode(errors="replace"))
        else:
            with open(output_path, "wb") as f:
                f.write(stdout)
        done += 1
        print_progress()

    if running:
        time.sleep(0.1)

print()  # Final newline after progress bar.
