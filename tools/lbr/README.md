```
perf record -F 999 -b -e cycles -R ../hwgc_soft/builds/all_in_one -o OpenJDK -t NodeObjref ../hwgc_soft/builds/dumps/* -i 1000
perf script --no-demangle -F brstacksym | zstd > dump_sym.txt.zst
perf script --no-demangle -F brstack | zstd > dump_addr.txt
```