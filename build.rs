use std::env;
use std::io::Result;
use std::path::Path;

fn main() -> Result<()> {
    prost_build::compile_protos(&["./src/heapdump.proto"], &["./src"])?;
    if cfg!(feature = "m5") {
        let root = env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-lib=static=m5");
        println!(
            "cargo:rustc-link-search=native={}",
            Path::new(&root).join("vendor/m5").to_str().unwrap()
        );
    }
    built::write_built_file().expect("Failed to acquire build-time information");
    Ok(())
}
