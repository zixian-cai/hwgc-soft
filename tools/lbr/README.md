```
perf record -F 999 -b -e cycles ../hwgc_soft/builds/all_in_one -o OpenJDK -t NodeObjref ../hwgc_soft/builds/dumps/* -i 1000
perf script --no-demangle -F brstacksym | zstd > dump_sym.txt.zst
perf script --no-demangle -F brstack | zstd > dump_addr.txt.zst
objdump -M intel -d ../hwgc_soft/builds/all_in_one |zstd > objdump.txt.zst
```

```
cargo run --release -- --objdump objdump.txt.zst dump_addr.txt.zst dump_sym.txt.zst
analyze 0x00005555556848df 0x00005555556848d9
analyze 0x000055daaf7ef260 0x000055daaf7ef26e # already marked
analyze 0x000055daaf7ef260 0x000055daaf7ef289 # not yet marked
quit
```