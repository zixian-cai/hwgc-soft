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
    #[arg(long)]
    pub(crate) trace_path: Option<String>,
    #[arg(short, long)]
    pub(crate) topology: SimulationMemoryLinkTopology,
    #[arg(short, long)]
    pub(crate) mem_config: SimulationMemoryConfiguration,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
#[clap(rename_all = "verbatim")]
pub enum SimulationArchitectureChoice {
    IdealTraceUtilization,
    NMPGC,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ValueEnum)]
#[clap(rename_all = "verbatim")]
pub enum SimulationMemoryConfiguration {
    C1D1R1 = 0, // 1 channel, 1 DIMM, 1 rank
    C2D1R1 = 1, // 2 channels, 1 DIMM, 1 rank,
    C1D2R1 = 2, // 1 channel, 2 DIMMs, 1 rank
    C1D1R2 = 3, // 1 channel, 1 DIMM, 2 ranks
    C1D2R2 = 4, // 1 channel, 2 DIMMs, 2 ranks
    C2D1R2 = 5, // 2 channels, 1 DIMM, 2 ranks
    C2D2R1 = 6, // 2 channels, 2 DIMMs, 1 rank
    C2D2R2 = 7, // 2 channels, 2 DIMMs, 2 ranks
}

impl SimulationMemoryConfiguration {
    pub fn get_total_ranks(&self) -> u8 {
        match self {
            SimulationMemoryConfiguration::C1D1R1 => 1,
            SimulationMemoryConfiguration::C2D1R1 => 2,
            SimulationMemoryConfiguration::C1D2R1 => 2,
            SimulationMemoryConfiguration::C1D1R2 => 2,
            SimulationMemoryConfiguration::C1D2R2 => 4,
            SimulationMemoryConfiguration::C2D1R2 => 4,
            SimulationMemoryConfiguration::C2D2R1 => 4,
            SimulationMemoryConfiguration::C2D2R2 => 8,
        }
    }

    pub fn get_owner_processor(&self, addr: u64) -> u8 {
        self.get_global_rank_id(addr)
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ValueEnum)]
#[clap(rename_all = "verbatim")]
pub enum SimulationMemoryLinkTopology {
    FullyConnected,
    Line,
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
