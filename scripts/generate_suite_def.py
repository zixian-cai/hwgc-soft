#!/usr/bin/env python3
import sys
from pathlib import Path

# suites:
#   heapdump:
#     type: BinaryBenchmarkSuite
#     programs:
#       fop:
#         path: "/bin/true"
#         args: "$PWD/builds/dumps/heapdump.3.binpb.zst $PWD/builds/dumps/heapdump.4.binpb.zst $PWD/builds/dumps/heapdump.5.binpb.zst $PWD/builds/dumps/heapdump.6.binpb.zst $PWD/builds/dumps/heapdump.7.binpb.zst $PWD/builds/dumps/heapdump.8.binpb.zst $PWD/builds/dumps/heapdump.9.binpb.zst $PWD/builds/dumps/heapdump.10.binpb.zst $PWD/builds/dumps/heapdump.11.binpb.zst $PWD/builds/dumps/heapdump.12.binpb.zst $PWD/builds/dumps/heapdump.13.binpb.zst $PWD/builds/dumps/heapdump.14.binpb.zst $PWD/builds/dumps/heapdump.15.binpb.zst $PWD/builds/dumps/heapdump.16.binpb.zst $PWD/builds/dumps/heapdump.17.binpb.zst $PWD/builds/dumps/heapdump.18.binpb.zst $PWD/builds/dumps/heapdump.19.binpb.zst $PWD/builds/dumps/heapdump.20.binpb.zst $PWD/builds/dumps/heapdump.21.binpb.zst $PWD/builds/dumps/heapdump.22.binpb.zst"


heapdump_sampled = Path(sys.argv[1])

for bm in sorted(heapdump_sampled.glob("*")):
    print("{}:".format(bm.stem))
    print('  path: "/bin/true"')
    print('  args: "', end="")
    files = list(bm.glob("*"))
    # heapdump.3.binpb.zst
    files.sort(key=lambda p: int(p.stem.split(".")[1]))
    print(" ".join(map(str, files)), end = "")
    print("\"")