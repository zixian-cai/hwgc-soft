use crate::*;
use anyhow::Result;
use polars::prelude::*;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

fn analyze_one_file(heapdump: &HeapDump) -> Result<LazyFrame> {
    // First, build a dataframe of each pointer, with source and target
    let mut sources = vec![];
    let mut targets = vec![];
    for obj in &heapdump.objects {
        for edge in &obj.edges {
            if edge.objref != 0 {
                sources.push(obj.start as i64);
                targets.push(edge.objref as i64);
            }
        }
    }
    let lf = df!("source" => sources, "target" => targets)?.lazy();
    let in_degrees_frequency = lf
        .clone()
        .group_by([col("target")])
        .agg([col("source").count().alias("degree")])
        .group_by([col("degree")])
        .agg([col("target").count().alias("degree_frequency")])
        // Use degree to calculate weighted frequency
        .with_column((col("degree") * col("degree_frequency")).alias("weighted_frequency"))
        // Normalize the weighted frequency
        .with_column(
            (col("weighted_frequency").cast(DataType::Float64)
                / sum("weighted_frequency").cast(DataType::Float64))
            .alias("normalized_weighted_frequency"),
        )
        .with_column(lit("in").alias("degree_type"));
    let out_degrees_frequency = lf
        .group_by([col("source")])
        .agg([col("target").count().alias("degree")])
        .group_by([col("degree")])
        .agg([col("source").count().alias("degree_frequency")])
        .with_column((col("degree") * col("degree_frequency")).alias("weighted_frequency"))
        .with_column(
            (col("weighted_frequency").cast(DataType::Float64)
                / sum("weighted_frequency").cast(DataType::Float64))
            .alias("normalized_weighted_frequency"),
        )
        .with_column(lit("out").alias("degree_type"));
    let output_lf = concat(
        vec![in_degrees_frequency, out_degrees_frequency],
        UnionArgs::default(),
    )?;
    Ok(output_lf)
}

/// Measure the in-degrees and out-degrees of objects in the heap dump.
// PATH=$HOME/protoc/bin:$PATH cargo run -- ../heapdumps/sampled/biojava/heapdump.5.binpb.zst  -o OpenJDK paper-analyze --analysis-name Degrees --output-path biojava.parquet
pub(super) fn degrees(
    paths: &[String],
    analysis_args: PaperAnalysisArgs,
    // we look at objects abstractly so don't care about concrete in-memory layout
    _object_model: ObjectModelChoice,
) -> Result<()> {
    let mut lfs = vec![];
    for p in paths {
        let heapdump = HeapDump::from_path(p)?;
        let lf = analyze_one_file(&heapdump)?;
        lfs.push(lf);
    }
    let final_lf = concat(
        lfs,
        UnionArgs {
            parallel: true,
            ..Default::default()
        },
    )?;
    let mut df = final_lf.collect()?;
    df.as_single_chunk_par();
    let file = File::create(analysis_args.output_path)?;
    let writer = ParquetWriter::new(file);
    writer.finish(&mut df)?;

    Ok(())
}
