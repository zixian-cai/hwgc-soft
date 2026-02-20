#!/bin/bash
set -e

# Ensure we are running from the repository root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$REPO_ROOT"

echo "Running from repository root: $REPO_ROOT"

# Build release
echo "Building project..."
cargo build --release

# Run Naive simulation
echo "Running Naive simulation..."
target/release/hwgc_soft ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC > scripts/naive_output.txt 2>&1
echo "Naive simulation completed."

# Run DRAMsim3 simulation
# Note: DRAMsim3 output is disabled via 'output_level = 0' in the config file to prevent
# generating large log files in the working directory.
echo "Running DRAMsim3 simulation..."
target/release/hwgc_soft ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC --use-dramsim3 > scripts/dramsim3_output.txt 2>&1

echo "DRAMsim3 simulation completed."

echo "Comparison of outputs:"
echo "--- Naive Output (tail) ---"
tail -n 20 scripts/naive_output.txt
echo "--- DRAMsim3 Output (tail) ---"
tail -n 20 scripts/dramsim3_output.txt

echo "Verification script finished."
