use anyhow::Result;
use clap::Parser;
use lazy_static::lazy_static;
use regex::Regex;
use std::fmt::Debug;
use std::io::{self, BufRead, Write};
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};

fn indent(count: u64) {
    for _ in 0..count {
        print!("\t");
    }
}

#[derive(Debug)]
struct LBRParser {
    stack_records: Vec<Vec<StackRecord>>,
    symbols: HashMap<Address, Symbol>,
}

struct Symbol {
    function: String,
    offset: u64,
}

impl Debug for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}+0x{:x}", self.function, self.offset))
    }
}

impl From<&str> for Symbol {
    fn from(value: &str) -> Self {
        let parts: Vec<&str> = value.split('+').collect();
        let function = parts[0].into();
        let offset: Address = if let Some(o) = parts.get(1) {
            (*o).into()
        } else {
            Address(0)
        };
        Symbol {
            function,
            offset: offset.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum StackRecordType {
    Call,
    Cond,
    Ret,
    Eret,
    Uncond,
    Ind,
    Irq,
    IndCall,
    SysRet,
}

impl From<&str> for StackRecordType {
    fn from(value: &str) -> Self {
        match value {
            "ERET" => Self::Eret,
            "CALL" => Self::Call,
            "UNCOND" => Self::Uncond,
            "RET" => Self::Ret,
            "COND" => Self::Cond,
            "IND" => Self::Ind,
            "IRQ" => Self::Irq,
            "IND_CALL" => Self::IndCall,
            "SYSRET" => Self::SysRet,
            _ => {
                unreachable!();
            }
        }
    }
}

#[derive(Debug)]
struct StackRecord {
    from: Address,
    to: Address,
    predicted: bool,
    cycles: u64,
    rtype: StackRecordType,
}

impl From<&str> for StackRecord {
    fn from(value: &str) -> Self {
        let parts: Vec<&str> = value.split('/').collect();
        StackRecord {
            from: parts[0].into(),
            to: parts[1].into(),
            predicted: parts[2] == "P",
            cycles: parts[5].parse::<u64>().unwrap(),
            rtype: parts[6].into(),
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
        for (addr_record, sym_record) in addr_line
            .trim()
            .split_ascii_whitespace()
            .zip(sym_line.trim().split_ascii_whitespace())
        {
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
        let parts: Vec<&str> = sym_record.split('/').collect();
        self.symbols
            .entry(sr.from)
            .or_insert_with(|| parts[0].into());
        self.symbols.entry(sr.to).or_insert_with(|| parts[1].into());
    }

    fn parse_zst(addr_p: impl AsRef<Path>, sym_p: impl AsRef<Path>) -> Result<LBRParser> {
        let mut p = LBRParser::new();
        let addr_file = File::open(addr_p)?;
        let sym_file = File::open(sym_p)?;
        let addr_reader = zstd::Decoder::new(addr_file)?;
        let sym_reader = zstd::Decoder::new(sym_file)?;
        let addr_lines = BufReader::new(addr_reader).lines();
        let sym_lines = BufReader::new(sym_reader).lines();
        for (i, (al, sl)) in addr_lines.zip(sym_lines).enumerate() {
            if i % 1000 == 0 {
                println!("Processed {} lines", i);
            }
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
    symbols: HashMap<Address, Symbol>,
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
    rtype: StackRecordType,
    targets: HashMap<Address, Block>,
    predicts: HashMap<Address, u64>,
    mispredicts: HashMap<Address, u64>,
    latencies: Vec<u64>,
    cumulative_latencies: Vec<u64>,
    count: u64,
}

impl Branch {
    fn new(from: Address, rtype: StackRecordType) -> Branch {
        Branch {
            from,
            rtype,
            targets: HashMap::new(),
            predicts: HashMap::new(),
            mispredicts: HashMap::new(),
            latencies: vec![],
            cumulative_latencies: vec![],
            count: 0,
        }
    }

    fn record_edge(&mut self, cumulative_latency: u64, edge: &StackRecord) -> u64 {
        debug_assert_eq!(edge.from, self.from);
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
            .or_insert_with(|| Branch::new(edges[0].from, edges[0].rtype));
        let new_cumulative_latency = branch.record_edge(cumulative_latency, &edges[0]);
        if branch.from != end {
            branch.follow_edge(new_cumulative_latency, end, &edges[0], &edges[1..]);
        }
    }

    fn latency_summary(latencies: &[u64]) -> String {
        let mut latencies = latencies.to_owned();
        latencies.sort();
        let sum = latencies.iter().sum::<u64>() as f64;
        format!(
            "min {} median {} max {} mean {:.2} sum {}",
            latencies[0],
            latencies[latencies.len() / 2],
            latencies[latencies.len() - 1],
            sum / latencies.len() as f64,
            sum
        )
    }

    fn print_dfs(
        &self,
        level: u64,
        end: Address,
        symbols: &HashMap<Address, Symbol>,
        objdump: &Option<Objdump>,
    ) {
        if self.count < 500 {
            return;
        }
        indent(level);
        let from_sym = symbols.get(&self.start).unwrap();
        println!("{:?} {} {:?}", self.start, self.count, from_sym);
        let mut branches: Vec<(&Address, &Branch)> = self.branches.iter().collect();
        branches.sort_by(|(_, a), (_, b)| b.count.cmp(&a.count));
        for (addr, branch) in branches {
            let to_sym = symbols.get(addr).unwrap();
            if let Some(o) = objdump.as_ref() {
                o.print_range(level + 1, from_sym, to_sym);
            }
            indent(level + 1);
            println!(
                "~{:?} {:?} {}/{} {:?} ->",
                addr, branch.rtype, branch.count, self.count, to_sym
            );
            if branch.from == end {
                indent(level + 1);
                println!(
                    "END cumulative latencies {}",
                    Self::latency_summary(&branch.cumulative_latencies)
                );
            } else {
                for target in branch.targets.values() {
                    target.print_dfs(level + 1, end, symbols, objdump);
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

#[derive(Debug)]
struct Objdump {
    functions: HashMap<String, ObjdumpFunction>,
}

impl Objdump {
    fn parse_zst(p: impl AsRef<Path>) -> Result<Objdump> {
        let file = File::open(p)?;
        let reader = zstd::Decoder::new(file)?;
        let lines = BufReader::new(reader).lines();
        let mut func_lines = vec![];
        let mut in_func = false;
        let mut objdump = Objdump {
            functions: HashMap::new(),
        };
        for line in lines {
            let l = line?;
            if l.starts_with("0000000") {
                func_lines.push(l);
                in_func = true;
            } else if in_func {
                if l.starts_with(' ') {
                    func_lines.push(l);
                } else {
                    debug_assert!(l.trim().is_empty());
                    in_func = false;
                    let func = ObjdumpFunction::new(&func_lines);
                    objdump.functions.insert(func.name.clone(), func);
                    func_lines.clear();
                }
            }
        }
        Ok(objdump)
    }

    fn print_range(&self, level: u64, from: &Symbol, to: &Symbol) {
        if from.function != to.function {
            // This is a bogus branch point
            // We can't be in a function, continue executing, and execute
            // a branch instruction in another function without having
            // another branch in between
            eprintln!(
                "Range of instructions to be printed is not in the same function {:?} {:?}",
                from, to
            );
        }
        let func = self.functions.get(&from.function).unwrap();
        for inst in &func.insts {
            if inst.offset < from.offset {
                continue;
            }
            if inst.offset > to.offset {
                break;
            }
            inst.print(level);
        }
    }
}

#[derive(Debug)]
struct ObjdumpFunction {
    _addr: u64,
    name: String,
    insts: Vec<ObjdumpInstruction>,
}

lazy_static! {
    static ref OBJDUMP_FUNCTION_HEADER: Regex = Regex::new(r"([0-9a-f]+) <(.*)>:").unwrap();
}

impl ObjdumpFunction {
    fn new(lines: &[String]) -> ObjdumpFunction {
        let caps = OBJDUMP_FUNCTION_HEADER.captures(&lines[0]).unwrap();
        let addr = u64::from_str_radix(caps.get(1).unwrap().as_str(), 16).unwrap();
        let name = caps.get(2).unwrap().as_str();
        let insts: Vec<ObjdumpInstruction> = lines[1..]
            .iter()
            .map(|l| ObjdumpInstruction::new(l, addr))
            .collect();
        ObjdumpFunction {
            _addr: addr,
            name: name.into(),
            insts,
        }
    }
}

#[derive(Debug)]
struct ObjdumpInstruction {
    offset: u64,
    text: Option<String>,
}

impl ObjdumpInstruction {
    fn new(line: &str, base: u64) -> ObjdumpInstruction {
        let parts: Vec<&str> = line.split('\t').collect();
        let addr_text = parts[0].trim().trim_end_matches(':');
        let addr = u64::from_str_radix(addr_text, 16).unwrap();
        let offset = addr - base;
        let text = parts.get(2).map(|x| x.trim().to_owned());
        ObjdumpInstruction { offset, text }
    }

    fn print(&self, level: u64) {
        if let Some(t) = &self.text {
            indent(level);
            println!("{}", t);
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(required = true)]
    addr_file: String,
    #[arg(required = true)]
    sym_file: String,
    #[arg(short, long)]
    objdump: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let p = LBRParser::parse_zst(args.addr_file, args.sym_file)?;
    let analysis: Analysis = p.into();
    let objdump = if let Some(p) = args.objdump {
        Some(Objdump::parse_zst(p)?)
    } else {
        None
    };
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
                println!("analyze <start> <end>");
            }
            "analyze" => {
                let start: Address = parts[1].into();
                let end: Address = parts[2].into();
                let block = analysis.run_query(start, end);
                block.print_dfs(0, end, &analysis.symbols, &objdump);
            }
            _ => {
                println!("Invalid command");
            }
        }
    }
    Ok(())
}
