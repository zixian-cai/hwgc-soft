use crate::*;
use anyhow::Result;
use polars::prelude::*;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

type CountMap = HashMap<u32, usize>;

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
struct EdgeChunk {
    chunk_size_log: u32,
    edge_count: usize,
}

impl EdgeChunk {
    fn log2_ceil(x: usize) -> u32 {
        usize::BITS - x.leading_zeros()
    }

    fn from_object(obj: &HeapObject) -> Vec<EdgeChunk> {
        if let Some(l) = obj.objarray_length {
            vec![EdgeChunk {
                chunk_size_log: Self::log2_ceil(l as usize),
                edge_count: l as usize,
            }]
        } else if obj.edges.is_empty() {
            vec![]
        } else {
            let mut offsets: Vec<i64> = obj
                .edges
                .iter()
                .map(|e| e.slot as i64 - obj.start as i64)
                .collect();
            offsets.sort();
            let mut i = 1;
            let mut chunk_size: usize = 1;
            let mut counts = vec![];
            while i < offsets.len() {
                if offsets[i] != offsets[i - 1] + 8 {
                    // not contiguous, push old chunk
                    counts.push(EdgeChunk {
                        chunk_size_log: Self::log2_ceil(chunk_size),
                        edge_count: chunk_size,
                    });
                    // start a new chunk
                    chunk_size = 1;
                } else {
                    chunk_size += 1;
                }
                i += 1;
            }
            counts.push(EdgeChunk {
                chunk_size_log: Self::log2_ceil(chunk_size),
                edge_count: chunk_size,
            });
            // println!("{:?} {:?}", offsets, counts);
            counts
        }
    }
}

fn merge_counts(count_a: &mut CountMap, count_b: &CountMap) {
    for (key, val) in count_b.iter() {
        *count_a.entry(*key).or_default() += val;
    }
}

fn analyze_one_file(path: &Path) -> Result<CountMap> {
    let heapdump = HeapDump::from_path(path.to_str().expect("File path should be valid UTF-8"))?;
    let shape_count = heapdump
        .objects
        .par_iter()
        .fold(
            HashMap::new,
            |mut partial_count: CountMap, object: &HeapObject| {
                let chunks = EdgeChunk::from_object(object);
                chunks.iter().for_each(|c| {
                    *partial_count.entry(c.chunk_size_log).or_default() += c.edge_count
                });
                partial_count
            },
        )
        .reduce(HashMap::new, |mut count_a: CountMap, count_b: CountMap| {
            merge_counts(&mut count_a, &count_b);
            count_a
        });
    Ok(shape_count)
}

// https://github.com/caizixian/mmtk-core/blob/shape/tools/shapes/shapes.py
fn analyze_benchmark(bm_path: &Path) -> Result<CountMap> {
    let heapdumps: Vec<PathBuf> = fs::read_dir(bm_path)?
        .map(|entry| {
            let entry = entry.unwrap();
            entry.path()
        })
        .collect();
    let shape_count: CountMap = heapdumps
        .par_iter()
        .map(|p| analyze_one_file(p).unwrap())
        .reduce(HashMap::new, |mut count_a: CountMap, count_b: CountMap| {
            merge_counts(&mut count_a, &count_b);
            count_a
        });
    Ok(shape_count)
}

pub(super) fn edge_chunks(paths: &[String], analysis_args: PaperAnalysisArgs) -> Result<()> {
    assert_eq!(
        paths.len(),
        1,
        "Should only have one path that is a folder contains subfolders for different benchmarks"
    );
    let heapdump_path = Path::new(paths.first().unwrap());
    assert!(heapdump_path.is_dir());
    let bms: Vec<PathBuf> = fs::read_dir(heapdump_path)?
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                info!("Found benchmark {:?}", path.file_stem().unwrap());
                Some(path)
            } else {
                None
            }
        })
        .collect();
    let bm_countmaps: Vec<(&str, CountMap)> = bms
        .par_iter()
        .map(|b| {
            let bm_name = b.file_stem().unwrap().to_str().unwrap();
            (bm_name, analyze_benchmark(b).unwrap())
        })
        .collect();

    let mut lfs = vec![];
    for (bm, count_map) in bm_countmaps {
        let (chunk_size_log, edges): (Vec<u32>, Vec<u64>) =
            count_map.iter().map(|(a, b)| (*a, *b as u64)).unzip();
        let lf: LazyFrame = df!(
            "chunk_size_log" => &chunk_size_log,
            "edges" => &edges,
        )
        .unwrap()
        .lazy();
        let lf = lf.with_column(lit(bm).alias("bm"));
        lfs.push(lf);
    }
    let lf = concat(
        lfs,
        UnionArgs {
            parallel: true,
            ..Default::default()
        },
    )?;
    let mut df = lf.collect()?;
    df.as_single_chunk_par();
    let file = File::create(analysis_args.output_path)?;
    let writer = ParquetWriter::new(file);
    writer.finish(&mut df)?;
    Ok(())
}
