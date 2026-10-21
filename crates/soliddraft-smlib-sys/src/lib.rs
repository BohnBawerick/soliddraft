//! Builds and links SMLib and SM_API, the C++ B-rep kernel of NVIDIA USD BRep, from the
//! pinned submodule in `third_party/usd-brep`.
//!
//! This crate has no Rust API beyond the constants below. It exists so the kernel, which
//! takes minutes to compile, is rebuilt only when its sources change. Dependents get the
//! kernel's include directories from the `DEP_SOLIDDRAFT_SMLIB_INCLUDE_API` and
//! `DEP_SOLIDDRAFT_SMLIB_INCLUDE_KERNEL` build-script variables.

#![no_std]

/// The usd-brep commit the kernel is built from.
pub const USD_BREP_COMMIT: &str = "c9c5979f04cea8c38213004d4f04df0dcfb40d05";

/// The usd-brep release of that commit, read from its `VERSION` file.
pub const USD_BREP_VERSION: &str = env!("SOLIDDRAFT_USD_BREP_VERSION");
