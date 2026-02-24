# Inter-DIMM Interconnect and Routing

**Date**: 2026-02-23

## Overview
The NMPGC simulator previously modeled inter-processor communication as instantaneous message delivery with a latency-only stall on the sender. This ignored three properties of a real inter-DIMM network: messages traverse intermediate hops, links have finite bandwidth, and routing logic runs on the DIMM link controllers. We refactored the architecture into a route-aware topology layer and a cycle-accurate network fabric that forwards messages hop-by-hop, tracks per-link bandwidth using pipelined flits, and correctly applies handoff stalls (`dimm_to_rank_latency`) at both sender and receiver.

## Architecture
- **`topology.rs`**: The `Topology` trait defines the physical DIMM interconnect. `get_route` returns the ordered directed links. `LineTopology` implements a `0 ↔ 2 ↔ 1 ↔ 3` layout using explicit position maps for O(1) route computation.
- **`network.rs`**: The `Network` struct models the inter-DIMM fabric. It manages `InFlightMessage`s carrying the entire physical route, with a cursor and per-hop countdown timer to keep track of the progress of the message. `tick()` decrements timers and advances messages. Per-directed-link counters track active traversing flits via `current_tick_flits` to establish peak message demand.
- **`work.rs`**: `SendMessage` and `ReadInbox` stall the processor for `dimm_to_rank_latency`—the local handoff cost. The network handles multi-hop transit asynchronously (modelled after having dedicated hardware for routing and forwarding).
- **`mod.rs`**: Each tick of `NMPGC` (after ticking all `NMPProcessor`s) consists of: (1) injecting new messages into the network with routes (by querying the topology); (2) calling into `Network` to advance in-flight messages; (3) `Network` returning messages that arrive at their destination DIMM at this tick, which are then put in the inbox of the destination processor. When the workload finishes, network statistics are printed.
- **`cli.rs`**: Allow run-time selectable topology through `--topology <Line|Ring|FullyConnected>` flag.

## Design Decisions & Lessons Learned

### 1. Match Physical Action Delays

**Challenge**: `SendMessage` stalled the sender for the full end-to-end multi-hop latency, while `ReadInbox` used a hardcoded 2-cycle delay.

**Mistake**: Coupling sender stalls to network transit times blocks processors unnecessarily. Using hardcoded magic numbers for receiver inbox reads disconnects latency from physical constants.

**Solution**: Processors stall for `DIMM_TO_RANK_LATENCY` (2 cycles) during both `SendMessage` (rank-to-controller) and `ReadInbox` (controller-to-rank). The link controller handles forwarding autonomously.

**Lesson**: Do not stall the `NMPProcessor` unnecessarily when the task can be handed off asynchronously.

### 2. Route Computation via Position Maps
**Challenge**: The original `LineTopology` stored a 4×4 latency matrix but had no concept of routing, which is unsuitable for computing physical link routes to simulate per-link behaviors.

**Solution**: Each topology now implements its own routing algorithm. For example, `LineTopology` stores two arrays: `dimm_at[position]` (the DIMM at each line position) and `position_of[dimm_id]` (the position of each DIMM). Route computation walks between positions:

```
Route from DIMM 0 to DIMM 3 on line [0, 2, 1, 3]:
  position_of[0] = 0, position_of[3] = 3
Walk positions 0→1→2→3 → links (0,2), (2,1), (1,3)
```

### 3. Bandwidth Tracking via Pipelined Flits

**Challenge**: Lumping an entire 64-bit message into a single tick upon injection (`peak_tick_counts`) artificially inflated peak bandwidth spikes.

**Mistake**: We initially tracked peak single-tick injection counts, assuming messages consumed link capacity instantaneously rather than spreading load over the hop duration.

**Solution**: We track "flits" traversing the link each cycle. `Network::tick` evaluates active routes in `in_flight` and increments `current_tick_flits`. We scale `peak_flits_per_tick` by `FLIT_SIZE_BYTES` (`MESSAGE_SIZE_BYTES / PER_HOP_LATENCY`) to compute true peak GB/s.

**Lesson**: Model bandwidth at the cycle resolution of the transfer interconnect (the flit) to derive accurate peak throughput.

### 4. Topology Extensibility
The `Topology` trait is agnostic. We implemented `RingTopology` (which wraps DIMM 3 to DIMM 0) and `FullyConnectedTopology`. `RingTopology` routing automatically selects the shortest path. The `Network` struct operates on arbitrary routes provided by the topology trait without internal modification.

### 5. Human-Readable Output Formatting
The simulator outputs a formatted diagram of the active topology via a generalized `Topology::print_diagram()`. This logic converts any topology's links into an adjacency list printed alongside the DIMMs physical location (e.g. `DIMM2 (C0-D1)`). Furthermore, network link traffic statistics are sorted by physical connection order rather than DIMM ID, making it much easier to trace traffic bottlenecks. Numbers are scaled with thousands separators for readability.

### 6. Load Balancing in Symmetric Rings

**Challenge**: In a symmetric ring (like 4 nodes, 0-2-1-3-0), traffic between diametrically opposite pairs (e.g., 0 ↔ 1) is equidistant in both directions (2 hops).

**Mistake**: A naive tie-breaker (e.g., `if cw_dist <= ccw_dist { route_cw() }`) causes all bidirectional traffic between opposite pairs to be routed in the same direction, leaving the reverse links idle and artificially bottlenecking the network throughput.

**Solution**: Implement deterministic parity-based load balancing. For equidistant routes, the direction is chosen based on the parity of the source node's position:
- **Even** source position: Route Clockwise (`cw`).
- **Odd** source position: Route Counter-Clockwise (`ccw`).

This rule utilizes both directions of links when routing traffic between equidistant nodes.

**Example**: In a 4-node ring `0 ↔ 2 ↔ 1 ↔ 3 ↔ 0`, the pairs `0 ↔ 1` and `2 ↔ 3` are equidistant (2 hops).
- For `0 ↔ 1` (even positions 0 and 2): $0 \to 1$ routes clockwise `0 → 2 → 1` and $1 \to 0$ routes clockwise `1 → 3 → 0`. Combined, they perfectly utilize all 4 clockwise links exactly once.
- For `2 ↔ 3` (odd positions 1 and 3): $2 \to 3$ routes counter-clockwise `2 → 0 → 3` and $3 \to 2$ routes counter-clockwise `3 → 1 → 2`. Combined, they perfectly utilize all 4 counter-clockwise links exactly once.

**Lesson**: Symmetric topologies require explicit tie-breaking strategies to distribute load. Without parity or randomized routing, "shortest path" logic can accidentally create hot links.

## Interconnect Model in Detail

### Physical Layout
Four DIMMs across two channels, two ranks per DIMM (8 ranks total). The `RankID` bitfield in `memory.rs` encodes `[rank:2, dimm:1, channel:0]`.

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
7. P7 executes ReadInbox → stalls 2 cycles (controller-to-rank handoff)
   Total network transit: 12 cycles. Total end-to-end latency: 16 cycles.
```

### Bandwidth Accounting
Each active cycle a message traverses a link, it increments `current_tick_flits`. The network tracks the maximum concurrency over the simulation to determine `peak_flits_per_tick`.

Peak throughput demand for a link is: `peak_flits_per_tick × FLIT_SIZE_BYTES × freq_GHz`. For example, for a DDR4-3200 system at 1.6 GHz, two flits inflight on the same link per tick (where `FLIT_SIZE_BYTES` is 2 B) equals 6.4 GB/s.

## Verification
All 26 unit tests pass (`cargo test`). `cargo clippy` and `cargo fmt` produce no warnings or changes.

### Performance Comparison with Master
Simulation on `fop/heapdump.2.binpb.zst`, 8 processors, `Line` topology:

| Metric | Master Naive | Current Naive | Master DRAMsim3 | Current DRAMsim3 |
| :--- | ---: | ---: | ---: | ---: |
| **Total Cycles** | 1,802,370 | 1,687,031 | 1,658,059 | 1,560,047 |
| **Utilization** | 0.818 | 0.812 | 0.823 | 0.809 |
| **Busy Ticks (sum)** | 11,789,730 | 10,957,484 | 10,912,492 | 10,099,101 |
| **Read Hit Rate** | 0.714 | 0.717 | 0.715 | 0.716 |
| **Marked Objects** | 93,180 | 93,180 | 93,180 | 93,180 |

The reduction in total cycles across both memory models (~6% for Naive and ~6% for DRAMsim3) is expected: the sender now stalls for only 2 cycles (DIMM-to-rank handoff) instead of the full end-to-end latency (4–12 cycles depending on destination). This frees processors to do productive work sooner while messages transit the network asynchronously. Marked objects remain identical, confirming correctness.

```
cargo run -- ../heapdumps/sampled/fop/heapdump.2.binpb.zst -o OpenJDK simulate -p 8 -a NMPGC
```

Network link stats from the simulation (8 B per message, split into 2 B flits over 4 ticks):

| Link | Msgs Fwd | Peak Flits | Peak GB/s | Avg GB/s |
| :--- | ---: | ---: | ---: | ---: |
| DIMM0 → DIMM2 | 20,855 | 4 | 12.8 | 0.17 |
| DIMM2 → DIMM0 | 21,652 | 5 | 16.0 | 0.18 |
| DIMM2 → DIMM1 | 36,665 | 6 | 19.2 | 0.30 |
| DIMM1 → DIMM2 | 29,286 | 5 | 16.0 | 0.24 |
| DIMM1 → DIMM3 | 22,371 | 5 | 16.0 | 0.18 |
| DIMM3 → DIMM1 | 20,288 | 4 | 12.8 | 0.17 |

DIMM 2 → DIMM 1 carries the most traffic (36,665 messages). DIMM 2 sits at position 1 in the line `[0, 2, 1, 3]`, so it forwards both DIMM 0's outbound traffic and DIMM 3's inbound traffic toward the center. Maximum flit overlap hits 6 concurrently active flit transfers (19.2 GB/s).

## Known Limitations
- **No link contention modeling**: Multiple messages traverse the same link simultaneously without queuing delay. The peak flits calculate demand but do not throttle throughput.
- **Fixed per-hop latency**: All hops cost 4 cycles regardless of message queue depth or link load.
