//! Builds the cxx bridge and the C++ engine in `cpp/`. The kernel itself is built by
//! `soliddraft-smlib-sys`, which passes its include directories here.

fn main() {
    let api = std::env::var("DEP_SOLIDDRAFT_SMLIB_INCLUDE_API").expect("soliddraft-smlib-sys include path");
    let kernel = std::env::var("DEP_SOLIDDRAFT_SMLIB_INCLUDE_KERNEL").expect("soliddraft-smlib-sys include path");

    let mut bridge = cxx_build::bridge("src/ffi.rs");
    bridge
        .file("cpp/engine.cpp")
        .file("cpp/hlr.cpp")
        .std("c++17")
        // Must match the kernel's build (see soliddraft-smlib-sys).
        .define("NDEBUG", "1")
        .define("NOMINMAX", "1")
        .define("SM_GRAPHICS_CALLBACKS", "1")
        .define("SM_API_EXPORTS", None);
    let compiler = bridge.get_compiler();
    if compiler.is_like_msvc() {
        bridge.include(&api).include(&kernel).flag("/EHsc").flag("/bigobj").flag("/utf-8");
    } else {
        // The kernel headers are third-party code: include them as system headers so the
        // engine's own warnings stay visible.
        bridge.flag("-isystem").flag(&api).flag("-isystem").flag(&kernel);
        // cxx's generated Vec constructors trip a GCC false positive.
        bridge.flag_if_supported("-Wno-maybe-uninitialized");
    }
    bridge.compile("soliddraft_smlib_bridge");

    println!("cargo:rerun-if-changed=cpp");
    println!("cargo:rerun-if-changed=src/ffi.rs");
}
