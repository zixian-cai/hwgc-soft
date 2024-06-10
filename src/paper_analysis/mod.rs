use crate::*;
use anyhow::Result;

mod edges;
mod shape;

pub fn reified_paper_analysis<O: ObjectModel>(mut _object_model: O, args: Args) -> Result<()> {
    let analysis_args = if let Some(Commands::PaperAnalyze(a)) = args.command {
        a
    } else {
        panic!("Incorrect dispatch");
    };

    match analysis_args.analysis_name {
        PaperAnalysisChoice::ShapeDemographic => {
            shape::shape_demographic(&args.paths, analysis_args)
        }
        PaperAnalysisChoice::EdgeChunks => edges::edge_chunks(&args.paths, analysis_args),
    }
}
