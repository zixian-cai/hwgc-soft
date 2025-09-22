use crate::*;
use anyhow::Result;
use polars::prelude::*;
use std::fs::File;

fn analyze_one_file(heapdump: &HeapDump) -> Result<LazyFrame> {
    // First, build a dataframe of each pointer, with source and target
    let mut sources = vec![];
    let mut targets = vec![];
    let mut objects = vec![];
    for obj in &heapdump.objects {
        objects.push(obj.start);
        for edge in &obj.edges {
            if edge.objref != 0 {
                sources.push(obj.start as u64);
                targets.push(edge.objref as u64);
            }
        }
    }
    let object_lf = df! {
        "source" => objects
    }?
    .lazy();
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
    // For each object with some outgoing references, count the out-degree
    let out_degrees = lf
        .group_by([col("source")])
        .agg([col("target").count().alias("degree")]);
    // Join with the list of all objects to include those with zero out-degrees
    let out_degrees = object_lf
        .left_join(out_degrees, col("source"), col("source"))
        .with_column(col("degree").fill_null(lit(0u32)));
    // Now finally compute the out-degree frequency
    let out_degrees_frequency = out_degrees
        .group_by([col("degree")])
        .agg([col("source").count().alias("degree_frequency")])
        .with_column(
            // Treat zero-degree objects as having degree one for weighted frequency
            (when(col("degree").eq(lit(0u32)))
                .then(lit(1u32))
                .otherwise(col("degree"))
                * col("degree_frequency"))
            .alias("weighted_frequency"),
        )
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
