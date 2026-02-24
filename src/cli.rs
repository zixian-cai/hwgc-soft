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
    pub(crate) rle: bool,
    #[arg(short, long, default_value_t = false)]
    pub(crate) eager_load: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct DepthArgs {
    #[arg(long)]
    pub(crate) output_file: String,
}

#[derive(Parser, Debug, Clone)]
pub struct PaperAnalysisArgs {
    #[arg(short, long, value_enum)]
    pub(crate) analysis_name: PaperAnalysisChoice,
    #[arg(short, long)]
    pub(crate) output_path: String,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum PaperAnalysisChoice {
    ShapeDemographic,
    EdgeChunks,
    Degrees,
}

/// Simulation args
#[derive(Parser, Debug, Clone)]
pub struct SimulationArgs {
    #[arg(short, long)]
    pub(crate) processors: usize,
    #[arg(short, long, value_enum)]
    pub(crate) architecture: SimulationArchitectureChoice,
    #[arg(short, long)]
    pub(crate) trace_path: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) use_dramsim3: bool,
    #[arg(long, default_value = "configs/DDR4_8Gb_x8_3200.ini")]
    pub(crate) dramsim3_config: String,
    #[arg(long, value_enum, default_value_t = TopologyChoice::Line)]
    pub(crate) topology: TopologyChoice,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum SimulationArchitectureChoice {
    IdealTraceUtilization,
    NMPGC,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum TopologyChoice {
    Line,
    Ring,
    FullyConnected,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Trace(TraceArgs),
    Analyze(AnalysisArgs),
    Depth(DepthArgs),
    PaperAnalyze(PaperAnalysisArgs),
    Simulate(SimulationArgs),
    Export(ExportArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct ExportArgs {
    #[arg(short, long)]
    pub(crate) output_path: String,
    #[arg(short, long)]
    pub(crate) format: ExportFormatChoice,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum ExportFormatChoice {
    CosmographCsv,
}
