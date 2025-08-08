use std::{collections::HashMap, fs::File, io};

use flate2::{write::GzEncoder, Compression};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;

pub(crate) enum InstantEventScope {
    Global,
    Process,
    Thread,
}

#[derive(Serialize)]
pub(crate) struct TracingEvent {
    pub(crate) name: String,
    pub(crate) ph: String,
    pub(crate) ts: u64,
    pub(crate) pid: u32,
    pub(crate) tid: u32,
    pub(crate) args: HashMap<String, Value>,
    pub(crate) dur: Option<u64>,
    pub(crate) s: Option<String>, // scope for instant events
}

impl TracingEvent {
    pub(crate) fn new_threadname_event(pid: u32, tid: u32, name: String) -> Self {
        let mut args = HashMap::new();
        args.insert("name".to_string(), Value::String(name));
        Self {
            name: "thread_name".to_string(),
            ph: "M".to_string(),
            ts: 0,
            pid,
            tid,
            args,
            dur: None,
            s: None,
        }
    }

    pub(crate) fn new_duration_event(
        pid: u32,
        tid: u32,
        name: String,
        ts: u64,
        args: HashMap<String, Value>,
        begin: bool, // if dur is set, begin is ignored
        dur: Option<u64>,
    ) -> Self {
        Self {
            name,
            ph: if dur.is_some() {
                "X".to_string()
            } else {
                if begin {
                    "B".to_string()
                } else {
                    "E".to_string()
                }
            },
            ts,
            pid,
            tid,
            args,
            dur,
            s: None,
        }
    }

    pub(crate) fn new_instant_event(
        pid: u32,
        tid: u32,
        name: String,
        ts: u64,
        args: HashMap<String, Value>,
        scope: InstantEventScope,
    ) -> Self {
        Self {
            name,
            ph: "i".to_string(),
            ts,
            pid,
            tid,
            args,
            dur: None,
            s: Some(match scope {
                InstantEventScope::Global => "g".to_string(),
                InstantEventScope::Process => "p".to_string(),
                InstantEventScope::Thread => "t".to_string(),
            }),
        }
    }
}

pub fn serialize_to_gzip_json<T: Serialize>(value: &T, path: &str) -> io::Result<()> {
    // Open the file for writing
    let file = File::create(path)?;

    // Create a GzEncoder with best compression
    let encoder = GzEncoder::new(file, Compression::default());

    // Create a BufWriter to buffer writes (optional for performance)
    let mut writer = io::BufWriter::new(encoder);

    // Serialize the struct as JSON directly into the encoder
    serde_json::to_writer(&mut writer, value)?;

    // Make sure everything is flushed and finished
    writer.flush()?;

    Ok(())
}
