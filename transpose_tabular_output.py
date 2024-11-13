#!/usr/bin/env python3
import sys

next_line_value = False
lines = open(sys.argv[1]).readlines()
for line in lines:
    if line.startswith("obj"):
        columns = line.strip().split("\t")
        next_line_value = True
        continue
    if next_line_value:
        values = line.strip().split("\t")
        for (c, v) in zip(columns, values):
            print(f"{c} {v}")
        next_line_value = False
        print()