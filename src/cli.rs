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
    pub(crate) tracing_loop: TracingLoopChoice,
    #[arg(short, long, default_value_t = 5)]
    pub(crate) iterations: usize,
    #[arg(long, default_value_t = 16)]
    pub(crate) shape_cache_size: usize,
    /// Number of worker threads to use, if the tracing loop supports parallelism.
    #[arg(long, default_value_t = num_cpus::get())]
    pub(crate) threads: usize,
    /// Work Packet buffer capacity.
    #[arg(long, default_value_t = 4096)]
    pub(crate) wp_capacity: usize,
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

#[derive(Parser, Debug, Clone)]
pub struct DepthArgs {
    #[arg(long)]
    pub(crate) output_file: String,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Trace(TraceArgs),
    Analyze(AnalysisArgs),
    Depth(DepthArgs),
    Memdump(MemdumpArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct MemdumpArgs {
    #[arg(short, long)]
    pub(crate) workload: MemdumpWorkload,
    #[arg(short, long)]
    pub(crate) output: String,
    #[arg(short, long, value_parser = num_parser)]
    pub(crate) mem_base: usize,
}

static NUM_PARSER_ERR: &str = "Invalid number. Must be base-10, or start with 0x or 0b.";

fn num_parser(s: &str) -> Result<usize, &'static str> {
    match s.get(0..2) {
        Some("0x") => usize::from_str_radix(&s[2..], 16).map_err(|_| NUM_PARSER_ERR),
        Some("0b") => usize::from_str_radix(&s[2..], 2).map_err(|_| NUM_PARSER_ERR),
        _ => s.parse::<usize>().map_err(|_| NUM_PARSER_ERR),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum MemdumpWorkload {
    LinkedList,
    HeapDump,
    HeapLinkedList,
}
