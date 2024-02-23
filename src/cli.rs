use crate::*;
use clap::{Parser, Subcommand, ValueEnum};
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum ObjectModelChoice {
    OpenJDK,
    OpenJDKAE,
    Bidirectional,
    BidirectionalFallback,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(required = true)]
    pub paths: Vec<String>,

    #[arg(short, long, value_enum)]
    pub object_model: ObjectModelChoice,

    #[command(subcommand)]
    pub command: Option<Commands>,
}
#[derive(Parser, Debug, Clone, Copy)]
pub struct TraceArgs {
    #[arg(short, long, value_enum)]
    pub tracing_loop: TracingLoopChoice,
    #[arg(short, long, default_value_t = 5)]
    pub iterations: usize,
}
#[derive(Parser, Debug, Clone, Copy)]
pub struct AnalysisArgs {
    #[arg(short, long, default_value_t = 6)]
    pub(crate) owner_shift: usize,
    #[arg(short, long, default_value_t = 3)]
    pub(crate) log_num_threads: usize,
    #[arg(short, long, default_value_t = false)]
    pub(crate) group_slots: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Trace(TraceArgs),
    Analyze(AnalysisArgs),
}
