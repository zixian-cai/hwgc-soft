use anyhow::Result;
use clap::Parser;
use std::fmt::Debug;
use std::io::{self, BufRead, Write};
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};

#[derive(Debug)]
struct LBRParser {
    stack_records: Vec<Vec<StackRecord>>,
    symbols: HashMap<Address, String>,
}

#[derive(Debug)]
struct StackRecord {
    from: Address,
    to: Address,
    predicted: bool,
    cycles: u64,
}

impl From<&str> for StackRecord {
    fn from(value: &str) -> Self {
        let parts: Vec<&str> = value.split('/').collect();
        StackRecord {
            from: parts[0].into(),
            to: parts[1].into(),
            predicted: parts[2] == "P",
            cycles: parts[5].parse::<u64>().unwrap(),
        }
    }
}

impl LBRParser {
    fn new() -> Self {
        LBRParser {
            stack_records: vec![],
            symbols: HashMap::new(),
        }
    }

    fn parse_line_pair(&mut self, addr_line: &str, sym_line: &str) {
        let mut records = vec![];
        for (addr_record, sym_record) in addr_line.split(' ').zip(sym_line.split(' ')) {
            if addr_record.is_empty() {
                continue;
            }
            let sr: StackRecord = addr_record.into();
            if sr.from.is_zero() {
                continue;
            }
            self.resolve_symbol(&sr, sym_record);
            records.push(sr);
        }
        records.reverse();
        self.stack_records.push(records);
    }

    fn resolve_symbol(&mut self, sr: &StackRecord, sym_record: &str) {
        self.symbols
            .entry(sr.from)
            .or_insert_with(|| sym_record.split('/').next().unwrap().into());
        self.symbols.entry(sr.to).or_insert_with(|| {
            let mut iter = sym_record.split('/');
            iter.next();
            iter.next().unwrap().into()
        });
    }

    fn parse_zst(addr_p: impl AsRef<Path>, sym_p: impl AsRef<Path>) -> Result<LBRParser> {
        let mut p = LBRParser::new();
        let addr_file = File::open(addr_p)?;
        let sym_file = File::open(sym_p)?;
        let addr_reader = zstd::Decoder::new(addr_file)?;
        let sym_reader = zstd::Decoder::new(sym_file)?;
        let addr_lines = BufReader::new(addr_reader).lines();
        let sym_lines = BufReader::new(sym_reader).lines();
        for (al, sl) in addr_lines.zip(sym_lines) {
            p.parse_line_pair(&al?, &sl?)
        }
        Ok(p)
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
#[repr(transparent)]
struct Address(u64);

impl From<u64> for Address {
    fn from(value: u64) -> Self {
        Address(value)
    }
}

impl From<Address> for u64 {
    fn from(value: Address) -> Self {
        value.0
    }
}

impl From<&str> for Address {
    fn from(value: &str) -> Self {
        u64::from_str_radix(value.trim_start_matches("0x"), 16)
            .unwrap()
            .into()
    }
}

impl Debug for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("0x{:x}", self.0))
    }
}

impl Address {
    fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

#[derive(Debug)]
struct Analysis {
    stack_records: Vec<Vec<StackRecord>>,
    symbols: HashMap<Address, String>,
}

#[derive(Debug)]
struct Block {
    start: Address,
    branches: HashMap<Address, Branch>,
    count: u64,
}

#[derive(Debug)]
struct Branch {
    from: Address,
    targets: HashMap<Address, Block>,
    predicts: HashMap<Address, u64>,
    mispredicts: HashMap<Address, u64>,
    latencies: Vec<u64>,
    cumulative_latencies: Vec<u64>,
    count: u64,
}

impl Branch {
    fn new(from: Address) -> Branch {
        Branch {
            from,
            targets: HashMap::new(),
            predicts: HashMap::new(),
            mispredicts: HashMap::new(),
            latencies: vec![],
            cumulative_latencies: vec![],
            count: 0,
        }
    }

    fn record_edge(&mut self, cumulative_latency: u64, edge: &StackRecord) -> u64 {
        self.targets
            .entry(edge.to)
            .or_insert_with(|| Block::new(edge.to));
        if edge.predicted {
            *self.predicts.entry(edge.to).or_insert(0) += 1;
        } else {
            *self.mispredicts.entry(edge.to).or_insert(0) += 1;
        }
        self.latencies.push(edge.cycles);
        self.cumulative_latencies
            .push(cumulative_latency + edge.cycles);
        self.count += 1;
        cumulative_latency + edge.cycles
    }

    fn follow_edge(
        &mut self,
        cumulative_latency: u64,
        end: Address,
        edge: &StackRecord,
        remaining_edges: &[StackRecord],
    ) {
        self.targets.get_mut(&edge.to).unwrap().enter_block(
            cumulative_latency,
            end,
            remaining_edges,
        )
    }
}

impl Block {
    fn new(start: Address) -> Self {
        Block {
            start,
            branches: HashMap::new(),
            count: 0,
        }
    }

    fn enter_block(&mut self, cumulative_latency: u64, end: Address, edges: &[StackRecord]) {
        self.count += 1;
        if edges.is_empty() {
            return;
        }
        let branch: &mut Branch = self
            .branches
            .entry(edges[0].from)
            .or_insert_with(|| Branch::new(edges[0].from));
        let new_cumulative_latency = branch.record_edge(cumulative_latency, &edges[0]);
        if branch.from != end {
            branch.follow_edge(new_cumulative_latency, end, &edges[0], &edges[1..]);
        }
    }

    fn indent(count: u64) {
        for _ in 0..count {
            print!("\t");
        }
    }

    fn latency_summary(latencies: &[u64]) -> String {
        let mut latencies = latencies.to_owned();
        latencies.sort();
        format!(
            "min {} median {} max {}",
            latencies[0],
            latencies[latencies.len() / 2],
            latencies[latencies.len() - 1],
        )
    }

    fn print_dfs(&self, level: u64, end: Address, symbols: &HashMap<Address, String>) {
        Self::indent(level);
        println!(
            "{:?} {} {}",
            self.start,
            self.count,
            symbols.get(&self.start).unwrap()
        );
        let mut branches: Vec<(&Address, &Branch)> = self.branches.iter().collect();
        branches.sort_by(|(_, a), (_, b)| b.count.cmp(&a.count));
        for (addr, branch) in branches {
            Self::indent(level + 1);
            println!(
                "~{:?} {}/{} {} ->",
                addr,
                branch.count,
                self.count,
                symbols.get(addr).unwrap()
            );
            if branch.from == end {
                Self::indent(level + 1);
                println!(
                    "END cumulative latencies {}",
                    Self::latency_summary(&branch.cumulative_latencies)
                );
            } else {
                for target in branch.targets.values() {
                    target.print_dfs(level + 1, end, symbols);
                }
            }
        }
    }
}

impl From<LBRParser> for Analysis {
    fn from(value: LBRParser) -> Self {
        Analysis {
            stack_records: value.stack_records,
            symbols: value.symbols,
        }
    }
}

impl Analysis {
    fn run_query(&self, start: Address, end: Address) -> Block {
        println!(
            "Finding traces starting from {:?} and ending at {:?}",
            start, end
        );
        let mut root_block = Block::new(start);

        for trace in &self.stack_records {
            let mut slice = trace.as_slice();
            while !slice.is_empty() {
                let edge = &slice[0];
                if edge.to == start {
                    root_block.enter_block(0, end, &slice[1..]);
                }
                // find next start
                slice = &slice[1..];
            }
        }
        root_block
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(required = true)]
    addr_file: String,
    #[arg(required = true)]
    sym_file: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let p = LBRParser::parse_zst(args.addr_file, args.sym_file)?;
    let analysis: Analysis = p.into();
    println!("Use 'help' to print a list of commands");
    loop {
        print!("> ");
        io::stdout().flush()?;
        let mut buffer = String::new();
        io::stdin().read_line(&mut buffer)?;
        let parts: Vec<&str> = buffer.trim().split(' ').collect();
        match parts[0] {
            "quit" => {
                break;
            }
            "help" => {
                println!("quit");
                println!("help");
                println!("analyze <text_sec_start> <start> <end>");
            }
            "analyze" => {
                let start: Address = parts[1].into();
                let end: Address = parts[2].into();
                let block = analysis.run_query(start, end);
                block.print_dfs(0, end, &analysis.symbols);
            }
            _ => {
                println!("Invalid command");
            }
        }
    }
    Ok(())
}
