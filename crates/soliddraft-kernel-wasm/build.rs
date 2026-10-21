//! Link settings for the kernel module. They apply only to this crate's binary, so other
//! Emscripten builds, such as the kernel's tests under node, link as usual.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("emscripten") {
        return;
    }
    for arg in [
        // A factory function the page calls, instead of a script that runs on load.
        "-sMODULARIZE=1",
        "-sEXPORT_NAME=SolidDraftKernel",
        "-sENVIRONMENT=web",
        "-sEXPORTED_RUNTIME_METHODS=HEAPU8,UTF8ToString",
        // Optimised and without debug info whatever the profile: the kernel's C++ is
        // built optimised in every profile, and the module is downloaded.
        "-O2",
        "-g0",
    ] {
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
