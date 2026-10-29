//! Planted Windows runtime failure for R2 release qualification.
#[test]
fn planted_release_fault() {
    assert_ne!(std::env::consts::OS, "windows", "planted release fault");
}
