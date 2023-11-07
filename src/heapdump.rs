mod generated_src {
    include!(concat!(env!("OUT_DIR"), "/heapdump.generated_src.rs"));
}

pub use generated_src::*;
