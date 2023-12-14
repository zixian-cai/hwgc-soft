use anyhow::Result;
use clap::Parser;
use std::io::BufRead;
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};
struct LBRParser {
    stack_records: Vec<StackRecord>,
    symbols: HashMap<u64, String>,
}

#[derive(Debug)]
struct StackRecord {
    from: u64,
    to: u64,
    predicted: bool,
    cycles: u64,
}

impl From<&str> for StackRecord {
    fn from(value: &str) -> Self {
        let parts: Vec<&str> = value.split('/').collect();
        StackRecord {
            from: u64::from_str_radix(parts[0].trim_start_matches("0x"), 16).unwrap(),
            to: u64::from_str_radix(parts[1].trim_start_matches("0x"), 16).unwrap(),
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
        for (addr_record, sym_record) in addr_line.split(' ').zip(sym_line.split(' ')) {
            if addr_record.is_empty() {
                continue;
            }
            let sr: StackRecord = addr_record.into();
            self.resolve_symbol(&sr, sym_record);
        }
    }

    fn resolve_symbol(&mut self, sr: &StackRecord, sym_record: &str) {
        self.symbols.entry(sr.from).or_insert_with(|| sym_record.split('/').next().unwrap().into());
        if !self.symbols.contains_key(&sr.to) {
            let mut iter = sym_record.split('/');
            iter.next();
            self.symbols.insert(sr.from, iter.next().unwrap().into());
        }
    }

    fn parse_zst(addr_p: impl AsRef<Path>, sym_p: impl AsRef<Path>) -> Result<LBRParser> {
        let mut p = LBRParser::new();
        let addr_file = File::open(addr_p)?;
        let sym_file = File::open(sym_p)?;
        let addr_reader = zstd::Decoder::new(addr_file)?;
        let sym_reader = zstd::Decoder::new(sym_file)?;
        let addr_lines: std::io::Lines<BufReader<zstd::Decoder<'_, BufReader<File>>>> =
            BufReader::new(addr_reader).lines();
        let sym_lines = BufReader::new(sym_reader).lines();
        for (al, sl) in addr_lines.zip(sym_lines) {
            p.parse_line_pair(&al?, &sl?)
        }
        Ok(p)
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
    let _p = LBRParser::parse_zst(args.addr_file, args.sym_file)?;
    Ok(())
}
