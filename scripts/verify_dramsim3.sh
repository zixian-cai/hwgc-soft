#!/bin/bash
set -e

# Build release
echo "Building project..."
cargo build --release

# Run Naive simulation
echo "Running Naive simulation..."
target/release/hwgc_soft ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC > naive_output.txt 2>&1
echo "Naive simulation completed."

# Run DRAMsim3 simulation
echo "Running DRAMsim3 simulation..."
target/release/hwgc_soft ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC --use-dramsim3 > dramsim3_output.txt 2>&1
echo "DRAMsim3 simulation completed."

echo "Comparison of outputs:"
echo "--- Naive Output (tail) ---"
tail -n 20 naive_output.txt
echo "--- DRAMsim3 Output (tail) ---"
tail -n 20 dramsim3_output.txt

echo "Verification script finished."
