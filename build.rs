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
    // We assume the DRAMsim3 repo is at ../DRAMsim3 relative to this repo, or we use a git submodule?
    // The user's metadata says: /usr/local/google/home/zixianc/Develop_GitHub/DRAMsim3
    // We should probably rely on an environment variable or a relative path.
    // Let's check if ../DRAMsim3 exists, otherwise fail or try a default.
    // Set LIBCLANG_PATH for bindgen
    std::env::set_var("LIBCLANG_PATH", "/usr/lib/llvm-19/lib");

    let dramsim3_src = Path::new(&root).parent().unwrap().join("DRAMsim3");
    if !dramsim3_src.exists() {
        println!("cargo:warning=DRAMsim3 not found at {:?}. Skipping DRAMsim3 build.", dramsim3_src);
    } else {
        let dst = cmake::Config::new(&dramsim3_src)
            .define("CMAKE_BUILD_TYPE", "Release")
            .no_build_target(true) // We will build it manually if needed, but actually we just need 'dramsim3' target.
                                   // Wait, cmake-rs builds 'install' by default.
                                   // We want to build the default target or specific target.
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
