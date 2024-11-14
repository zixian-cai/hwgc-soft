use crate::*;
use anyhow::Result;
use polars::prelude::*;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
enum ObjectShape {
    NoRef,
    RefArray,
    Refs(Vec<i64>),
}

impl ObjectShape {
    fn from_object(obj: &HeapObject) -> ObjectShape {
        if obj.objarray_length.is_some() {
            ObjectShape::RefArray
        } else if obj.edges.is_empty() {
            ObjectShape::NoRef
        } else {
            let mut offsets: Vec<i64> = obj
                .edges
                .iter()
                .map(|e| e.slot as i64 - obj.start as i64)
                .collect();
            offsets.sort();
            ObjectShape::Refs(offsets)
        }
    }

    fn into_array(self) -> Vec<i64> {
        match self {
            ObjectShape::NoRef => vec![],
            ObjectShape::RefArray => vec![i64::MIN],
            ObjectShape::Refs(r) => r,
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

// RUST_LOG=info PATH=$HOME/protoc/bin:$PATH cargo run --release -- ../heapdumps/sampled -o OpenJDK paper-analyze --analysis-name ShapeDemographic -o shapes.parquet
pub(super) fn shape_demographic(
    paths: &[String],
    analysis_args: PaperAnalysisArgs,
    object_model: ObjectModelChoice,
) -> Result<()> {
    assert_eq!(
        paths.len(),
        1,
        "Should only have one path that is a folder contains subfolders for different benchmarks"
    );
    assert!(
        matches!(object_model, ObjectModelChoice::OpenJDK)
            || matches!(object_model, ObjectModelChoice::OpenJDKAE),
        "Only support shape analysis for OpenJDK object model for now"
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
        let (shapes, counts): (Vec<Series>, Vec<u64>) = count_map
            .iter()
            .map(|(a, b)| (a.clone().into_array().iter().collect::<Series>(), *b as u64))
            .unzip();
        let lf: LazyFrame = df!(
            "shape" => &shapes,
            "count" => &counts,
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
