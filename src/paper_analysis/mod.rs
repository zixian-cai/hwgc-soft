use crate::*;
use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
enum ObjectShape {
    NoRef,
    RefArray,
    Refs(Vec<u64>),
}

impl ObjectShape {
    fn from_object(obj: &HeapObject) -> ObjectShape {
        if obj.objarray_length.is_some() {
            ObjectShape::RefArray
        } else if obj.edges.is_empty() {
            ObjectShape::NoRef
        } else {
            let offsets: Vec<u64> = obj.edges.iter().map(|e| e.slot - obj.start).collect();
            ObjectShape::Refs(offsets)
        }
    }
}

type CountMap = HashMap<ObjectShape, usize>;

fn merge_counts(count_a: &mut CountMap, count_b: &CountMap) {
    for (key, val) in count_b.iter() {
        *count_a.entry(key.clone()).or_default() += val;
    }
}

fn analyze_one_file(path: &Path) -> Result<CountMap> {
    let heapdump = HeapDump::from_binpb_zst(path)?;
    let shape_count = heapdump
        .objects
        .par_iter()
        .fold(
            HashMap::new,
            |mut partial_count: CountMap, object: &HeapObject| {
                *partial_count
                    .entry(ObjectShape::from_object(object))
                    .or_default() += 1;
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

// cargo run -- ../heapdumps/sampled  -o OpenJDK paper-analyze
pub fn reified_paper_analysis<O: ObjectModel>(mut _object_model: O, args: Args) -> Result<()> {
    assert_eq!(
        args.paths.len(),
        1,
        "Should only have one path that is a folder contains subfolders for different benchmarks"
    );
    let heapdump_path = Path::new(args.paths.first().unwrap());
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
    println!("{:?}", bm_countmaps);
    // let analysis_args = if let Some(Commands::PaperAnalyze(a)) = args.command {
    //     a
    // } else {
    //     panic!("Incorrect dispatch");
    // };

    Ok(())
}
