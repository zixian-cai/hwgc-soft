use crate::trace::trace_object;
use crate::*;
use anyhow::Result;
use polars::functions::concat_df_diagonal;
use polars::prelude::*;
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    iter,
};

type Depth = u64;

pub fn object_depth<O: ObjectModel>(mut object_model: O, args: Args) -> Result<()> {
    let object_depth_args = if let Some(Commands::Depth(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };
    let mut dfs = vec![];
    for (i, path) in args.paths.iter().enumerate() {
        let heapdump = HeapDump::from_binpb_zst(path)?;
        object_model.reset();
        heapdump.map_spaces()?;
        object_model.restore_objects(&heapdump);
        let mut depth_hist: HashMap<Depth, u64> = HashMap::new();
        let mut mark_queue: VecDeque<(u64, Depth)> = VecDeque::new();
        for root in object_model.roots() {
            let o = *root;
            mark_queue.push_back((o, 0));
            debug_assert_ne!(o, 0);
        }
        while let Some((o, depth)) = mark_queue.pop_front() {
            if unsafe { trace_object(o, 1) } {
                *depth_hist.entry(depth).or_default() += 1;
                O::scan_object(o, |edge, repeat| {
                    for i in 0..repeat {
                        let e = edge.wrapping_add(i as usize);
                        let child = unsafe { *e };
                        if child != 0 {
                            mark_queue.push_back((child, depth + 1));
                        }
                    }
                });
            }
        }
        debug_assert_eq!(
            depth_hist.values().sum::<u64>() as usize,
            object_model.objects().len()
        );
        let (depth_vec, count_vec): (Vec<Depth>, Vec<u64>) = depth_hist.into_iter().unzip();
        let mut df = df! {
            "depth" => depth_vec,
            "counts" => count_vec
        }?;
        let iteration_series: Series = iter::repeat_n(i as u64, df.height()).collect();
        df.with_column(Series::new("iteration", iteration_series))?;
        dfs.push(df);
        heapdump.unmap_spaces()?;
    }
    let mut df = concat_df_diagonal(&dfs)?;
    df.as_single_chunk_par();
    let file = File::create(object_depth_args.output_file)?;
    let writer = ParquetWriter::new(file);
    writer.finish(&mut df)?;
    Ok(())
}
