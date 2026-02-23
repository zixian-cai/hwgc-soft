# Inter-DIMM Interconnect and Routing

**Date**: 2026-02-23

## Overview
The NMPGC simulator previously modeled inter-processor communication as an instantaneous message delivery with a latency-only stall on the sender. This ignored three properties of a real inter-DIMM network: messages traverse intermediate hops on non-directly-connected DIMMs, each link has finite bandwidth, and routing logic runs on the DIMM link controllers—not the processors. We refactored the topology system into a route-aware topology layer and a cycle-accurate network fabric that forwards messages hop-by-hop, tracks per-link bandwidth, and reports peak throughput demand in GB/s.

## Architecture
- **`topology.rs`**: The `Topology` trait defines the physical DIMM interconnect. `get_route(from_dimm, to_dimm)` returns the ordered sequence of directed links a message must traverse. `get_per_hop_latency()` returns the cost of one link traversal. `get_dimm_to_rank_latency()` returns the local cost of moving a message between a rank and its DIMM's link controller. `get_links()` enumerates all physical links for network initialization. `LineTopology` implements a line topology `0 ↔ 2 ↔ 1 ↔ 3` with explicit position maps for O(1) route computation.
- **`network.rs`**: The `Network` struct models the fabric between DIMMs. It holds a list of `InFlightMessage`s, each carrying its remaining route (a `Vec<(u8, u8)>` of directed links) and a per-hop countdown timer. `inject()` places a message on its first link. `tick()` decrements all timers; when a hop completes, the message advances to the next link or is delivered. Per-directed-link counters track total messages forwarded and per-tick peak message count. The fabric constants `PER_HOP_LATENCY` (4 cycles) and `DIMM_TO_RANK_LATENCY` (2 cycles) are defined here rather than in the topologies.
- **`work.rs`**: `SendMessage` stalls the processor for only `dimm_to_rank_latency` (2 cycles)—the cost of handing the message to the DIMM link controller. The network handles the multi-hop transit asynchronously.
- **`mod.rs`**: `NMPGC` owns the `Network` and dynamically resolves the topology trait. Each tick: (1) processors execute, yielding outgoing messages; (2) messages are injected into the network with routes computed from the topology; (3) the network ticks, advancing all in-flight messages; (4) arrived messages are delivered to recipient inboxes. Termination requires all processors idle **and** no in-flight messages. Stats output includes a visual connection diagram and physically-sorted link throughput in GB/s, with thousands-separators for scale metrics.
- **`cli.rs`**: Added `--topology <Line|Ring>` flag to `.simulate` to select the interconnect type at runtime.

## Design Decisions & Lessons Learned

### 1. Processor Stalls Only for Local Handoff
**Challenge**: In the old model, `SendMessage` stalled the sender for the full end-to-end latency. This is unrealistic—a processor with a 3-hop message shouldn't block for 12+ cycles while DIMMs in between forward the message.

**Solution**: The processor stalls for `DIMM_TO_RANK_LATENCY` (2 cycles), representing the cost of passing the message from the rank to the DIMM link controller. The link controller and intermediate DIMMs handle forwarding autonomously. This matches real NMP systems where each DIMM has dedicated routing logic.

### 2. Route Computation via Position Maps
**Challenge**: The original `LineTopology` stored a 4×4 latency matrix but had no concept of routing. Computing routes from a latency matrix requires pathfinding, which is fragile and O(n²).

**Solution**: `LineTopology` stores two arrays: `dimm_at[position]` (the DIMM at each line position) and `position_of[dimm_id]` (the position of each DIMM). Route computation walks between positions:

```rust
// Route from DIMM 0 to DIMM 3 on line [0, 2, 1, 3]:
// position_of[0] = 0, position_of[3] = 3
// Walk positions 0→1→2→3 → links (0,2), (2,1), (1,3)
```

The latency matrix is retained for backwards compatibility with `get_latency()` and validated by `test_line_topology_route_consistency`, which asserts that `route.len() * per_hop_latency + 2 * dimm_to_rank_latency` equals the matrix-derived latency for every pair.

### 3. Bandwidth Tracking via Per-Tick Peak Counts
**Challenge**: Reporting average throughput (total messages / total time) understates link pressure. A link that is idle for 90% of the simulation but saturated for 10% has low average throughput but needs high peak bandwidth.

**Solution**: Each directed link maintains a `current_tick_counts` counter, flushed at every `tick()` into `peak_tick_counts` (the maximum observed). Peak throughput demand in GB/s is `peak_messages_per_tick * 8 * frequency_ghz`. Average throughput is also reported for comparison.

### 4. Same-DIMM Message Bypass
Messages between ranks on the same DIMM bypass the network entirely and are delivered directly to the recipient's inbox. The sender stalls for `DIMM_TO_RANK_LATENCY` (2 cycles) in `get_latency()`. No network link is involved.

### 5. Topology Extensibility & Ring Topology
The `Topology` trait is topology-agnostic. In addition to `LineTopology`, we implemented `RingTopology` which wraps a link from DIMM 3 to DIMM 0. Routing in the ring automatically selects the shortest path (clockwise or counter-clockwise). Topologies are selectable via CLI (`--topology Line` or `--topology Ring`). The `Network` struct requires no modification—it operates on arbitrary routes provided by the topology trait. Methods like `print_diagram()` and `link_sort_key()` allow any topology to cleanly present itself in the simulation output.

### 6. Human-Readable Output Formatting
The simulator outputs a formatted diagram of the active topology via `Topology::print_diagram()`, which works for both Line and Ring (or any future topology). Furthermore, network link traffic statistics are sorted by physical connection order rather than DIMM ID, making it much easier to trace traffic bottlenecks. Numbers are scaled with thousands separators for readability.

## Interconnect Model in Detail

### Physical Layout
Four DIMMs across two channels, two ranks per DIMM (8 ranks total). The `RankID` bitfield in `memory.rs` encodes `[channel:0, dimm:1, rank:2]`.

```
DIMM 0 (C0-D0)  ←→  DIMM 2 (C0-D1)  ←→  DIMM 1 (C1-D0)  ←→  DIMM 3 (C1-D1)
  R0, R4              R2, R6              R1, R5              R3, R7
```

### Message Lifecycle
```
Processor P0 (on DIMM 0, Rank 0) sends to P7 (on DIMM 3, Rank 1):

1. P0 executes SendMessage → stalls 2 cycles (DIMM-to-rank handoff)
2. NMPGC::tick() computes route: [(0,2), (2,1), (1,3)]
3. Network::inject() places message on link (0,2), starts 4-cycle countdown
4. After 4 ticks: message moves to link (2,1), starts 4-cycle countdown
5. After 4 more ticks: message moves to link (1,3), starts 4-cycle countdown
6. After 4 more ticks: message delivered to P7's inbox
   Total network transit: 12 cycles
```

### Bandwidth Accounting
Each link traversal increments `messages_forwarded` on the directed link and `current_tick_counts`. The 3-hop message above increments three directed links: `(0,2)`, `(2,1)`, and `(1,3)`.

Peak throughput demand for a link is: `peak_messages_per_tick × 8 B × freq_GHz`. For a DDR4-3200 system at 1.6 GHz, one message per tick equals 12.8 GB/s.

## Verification
All 21 unit tests pass (`cargo test`). `cargo clippy` and `cargo fmt` produce no warnings or changes.

### Performance Comparison with Master
Simulation on `fop/heapdump.2.binpb.zst`, 8 processors, `Line` topology:

| Metric | Master Naive | Current Naive | Master DRAMsim3 | Current DRAMsim3 |
| :--- | ---: | ---: | ---: | ---: |
| **Total Cycles** | 1,802,370 | 1,687,031 | 1,658,059 | 1,560,047 |
| **Utilization** | 0.818 | 0.812 | 0.823 | 0.809 |
| **Busy Ticks (sum)** | 11,789,730 | 10,957,484 | 10,912,492 | 10,099,101 |
| **Read Hit Rate** | 0.714 | 0.717 | 0.715 | 0.716 |
| **Marked Objects** | 93,180 | 93,180 | 93,180 | 93,180 |

The reduction in total cycles across both memory models (~6% for Naive and ~6% for DRAMsim3) is expected: the sender now stalls for only 2 cycles (DIMM-to-rank handoff) instead of the full end-to-end latency (4–12 cycles depending on destination). This frees processors to do productive work sooner while messages transit the network asynchronously. Utilization drops slightly because the faster completion means proportionally less of the total time is spent busy. Marked objects remain identical, confirming correctness.

```
cargo run -- ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC
```

Network link stats from the simulation (8 B per message):

| Link | Msgs Fwd | Peak/Tick | Peak GB/s | Avg GB/s |
| :--- | ---: | ---: | ---: | ---: |
| DIMM0 → DIMM2 | 20,855 | 2 | 25.6 | 0.16 |
| DIMM2 → DIMM0 | 21,652 | 3 | 38.4 | 0.16 |
| DIMM2 → DIMM1 | 36,665 | 2 | 25.6 | 0.28 |
| DIMM1 → DIMM2 | 29,286 | 2 | 25.6 | 0.22 |
| DIMM1 → DIMM3 | 22,371 | 3 | 38.4 | 0.17 |
| DIMM3 → DIMM1 | 20,288 | 2 | 25.6 | 0.15 |

DIMM 2 → DIMM 1 carries the most traffic (36,665 messages). DIMM 2 sits at position 1 in the line `[0, 2, 1, 3]`, so it forwards both DIMM 0's outbound traffic and DIMM 3's inbound traffic toward the center.

## Known Limitations
- **No link contention modeling**: Multiple messages can traverse the same link simultaneously without queuing delay. The peak-per-tick counter measures demand but does not throttle throughput.
- **Fixed per-hop latency**: All hops cost `PER_HOP_LATENCY` (4 cycles) regardless of message queue depth or link load.
- **No message pipelining within a link**: A message occupies a link for the full hop latency, but does not block other messages from entering the same link in the same tick.
