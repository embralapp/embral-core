use std::env;

fn main() {
    println!("cargo:rerun-if-changed=isoc23_shim.c");

    // Only gnu-libc Linux needs the shim; see isoc23_shim.c.
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let libc = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "linux" || libc != "gnu" {
        return;
    }

    cc::Build::new()
        .file("isoc23_shim.c")
        .flag("-std=gnu11")
        .cargo_metadata(false)
        .compile("isoc23_shim");

    // +whole-archive: nothing in Rust references these symbols, and archive
    // member extraction is single-pass; without it the linker can process
    // ort's archives after ours and never pull the shim object in.
    let out_dir = env::var("OUT_DIR").unwrap();
    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static:+whole-archive=isoc23_shim");
}
