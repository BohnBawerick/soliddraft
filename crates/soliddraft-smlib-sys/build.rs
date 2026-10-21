//! Compiles SMLib and SM_API from the pinned usd-brep submodule into one static library.
//!
//! The kernel is compiled with the `cc` crate rather than with upstream's premake and
//! `repo.sh` scripts: those fetch a toolchain and OpenUSD through packman, and SMLib and
//! SM_API need neither. The defines are upstream's release configuration
//! (`premake5.lua`): NDEBUG, NOMINMAX and SM_GRAPHICS_CALLBACKS=1, in C++17. SM_USE_TBB
//! is left undefined, as upstream does, so the kernel runs single-threaded and needs no
//! TBB.
//!
//! Upstream builds with -Werror. GCC 16 adds three diagnostics that fire in the
//! unmodified sources (-Woverloaded-virtual, -Wtemplate-id-cdtor and
//! -Wunused-but-set-variable), so the upstream build fails on GCC 16. This build does
//! not pass -Werror and silences the kernel's warnings. No kernel source is patched, and
//! warning flags do not change the generated code.

use std::path::{Path, PathBuf};

/// Upstream's release-configuration defines.
const KERNEL_DEFINES: &[(&str, &str)] = &[("NDEBUG", "1"), ("NOMINMAX", "1"), ("SM_GRAPHICS_CALLBACKS", "1")];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../third_party/usd-brep");
    let smlib = root.join("source/SMLib");
    let sm_api = root.join("source/SM_API");
    if !smlib.join("src").is_dir() || !sm_api.join("src").is_dir() {
        panic!(
            "usd-brep sources not found under {}. Run `git submodule update --init third_party/usd-brep`.",
            root.display()
        );
    }

    // SM_API and SMLib each have their own StdAfx.h, so each set is compiled with its own
    // include order; both then go into one archive, since they reference each other.
    let mut api = kernel_build(&[&sm_api.join("inc"), &smlib.join("inc")]);
    api.define("SM_API_EXPORTS", None).files(sources(&sm_api.join("src")));
    if is_gnu_like(&api) {
        // Upstream's SM_API project flag.
        api.flag("-Wno-write-strings");
    }
    let mut objects = api.compile_intermediates();

    let mut kernel = kernel_build(&[&smlib.join("inc")]);
    kernel.files(sources(&smlib.join("src")));
    objects.extend(kernel.compile_intermediates());

    cc::Build::new().cpp(true).objects(objects).compile("soliddraft_smlib_kernel");

    println!("cargo:include_api={}", sm_api.join("inc").display());
    println!("cargo:include_kernel={}", smlib.join("inc").display());
    println!("cargo:rerun-if-changed={}", smlib.display());
    println!("cargo:rerun-if-changed={}", sm_api.display());
    println!("cargo:rerun-if-changed={}", root.join("VERSION").display());
    let version = std::fs::read_to_string(root.join("VERSION")).unwrap_or_default();
    println!("cargo:rustc-env=SOLIDDRAFT_USD_BREP_VERSION={}", version.trim());
}

fn kernel_build(includes: &[&Path]) -> cc::Build {
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").warnings(false);
    for dir in includes {
        build.include(dir);
    }
    for (name, value) in KERNEL_DEFINES {
        build.define(name, Some(*value));
    }
    if build.get_compiler().is_like_msvc() {
        build.flag("/EHsc").flag("/bigobj").flag("/utf-8").flag("/w");
    } else {
        build.flag("-w");
    }
    build
}

fn is_gnu_like(build: &cc::Build) -> bool {
    let compiler = build.get_compiler();
    compiler.is_like_gnu() || compiler.is_like_clang()
}

fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "cpp"))
        // StdAfx.cpp only builds the Windows precompiled header.
        .filter(|p| p.file_name().is_some_and(|n| n != "StdAfx.cpp"))
        .collect();
    files.sort();
    files
}
