use std::env;
use std::path::{Path, PathBuf};

fn main() {
    prost_build::compile_protos(&["./src/heapdump.proto"], &["./src"]).unwrap();

    let root = env::var("CARGO_MANIFEST_DIR").unwrap();
    if cfg!(feature = "m5") {
        println!("cargo:rustc-link-lib=static=m5");
        println!(
            "cargo:rustc-link-search=native={}",
            Path::new(&root).join("vendor/m5").to_str().unwrap()
        );
    }
    built::write_built_file().expect("Failed to acquire build-time information");

    // Build DRAMsim3
    //
    // This build script expects the DRAMsim3 repository to be a sibling
    // directory to this crate. The user needs to ensure that `../DRAMsim3` exists.
    //
    // Future improvements could use an environment variable (e.g., DRAMSIM3_ROOT)
    // or a git submodule for better portability.
    // Set LIBCLANG_PATH for bindgen if not set, trying common locations
    if env::var("LIBCLANG_PATH").is_err() {
        let possible_paths = [
            "/usr/lib/llvm-19/lib",
            "/usr/lib/llvm-18/lib",
            "/usr/lib/llvm-14/lib",
        ];
        for path in possible_paths {
            if Path::new(path).exists() {
                println!("cargo:warning=Setting LIBCLANG_PATH to {}", path);
                env::set_var("LIBCLANG_PATH", path);
                break;
            }
        }
    }

    let dramsim3_src = Path::new(&root).parent().unwrap().join("DRAMsim3");
    if !dramsim3_src.exists() {
        println!(
            "cargo:warning=DRAMsim3 not found at {:?}. Skipping DRAMsim3 build.",
            dramsim3_src
        );
    } else {
        let dst = cmake::Config::new(&dramsim3_src)
            .define("CMAKE_BUILD_TYPE", "Release")
            // We explicitly build the 'dramsim3' target.
            // By default, cmake-rs attempts to build the 'install' target,
            // which we want to avoid as we only need the static library.
            .no_build_target(true)
            .build_target("dramsim3")
            .build();

        println!("cargo:rustc-link-search=native={}/build", dst.display());
        println!("cargo:rustc-link-lib=static=dramsim3");
        println!("cargo:rustc-link-lib=dylib=stdc++");

        // Build the wrapper
        cc::Build::new()
            .cpp(true)
            .file("src/shim/dramsim3_wrapper.cc")
            .include(&dramsim3_src.join("src"))
            .include("src/shim")
            .flag("-std=c++11")
            .compile("dramsim3_wrapper");

        println!("cargo:rustc-link-lib=static=dramsim3_wrapper");
        println!("cargo:rerun-if-changed=src/shim/dramsim3_wrapper.cc");
        println!("cargo:rerun-if-changed=src/shim/dramsim3_wrapper.h");
        println!(
            "cargo:rerun-if-changed={}",
            dramsim3_src.join("src").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            dramsim3_src.join("configs").display()
        );

        // Generate bindings
        let bindings = bindgen::Builder::default()
            .header("src/shim/dramsim3_wrapper.h")
            .clang_arg(format!("-I{}", dramsim3_src.join("src").display()))
            .clang_arg("-x")
            .clang_arg("c++")
            .clang_arg("-std=c++14")
            .clang_arg("-I/usr/include/c++/15")
            .clang_arg("-I/usr/include/x86_64-linux-gnu/c++/15")
            .clang_arg("-I/usr/include")
            .allowlist_function("new_dramsim3_wrapper")
            .allowlist_function("dramsim3_add_transaction")
            .allowlist_function("dramsim3_clock_tick")
            .allowlist_function("dramsim3_will_accept_transaction")
            .allowlist_function("dramsim3_is_transaction_done")
            .allowlist_function("delete_dramsim3_wrapper")
            .allowlist_type("CDRAMSim3")
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
            .generate()
            .expect("Unable to generate bindings");

        let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
        bindings
            .write_to_file(out_path.join("dramsim3_bindings.rs"))
            .expect("Couldn't write bindings!");
    }
}
