//! Templates and styles from File locations join the built-in ones. A test binary of its
//! own, because the library is built once per process.

use std::path::PathBuf;

use soliddraft_drawing::library::{self, use_files};

#[test]
fn listed_files_join_the_library_and_bad_ones_are_reported() {
    let dir = std::env::temp_dir().join(format!("soliddraft-user-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let builtin = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/iso-a3.toml")).unwrap();
    let office = builtin.replacen("id = \"iso-a3\"", "id = \"office-a3\"", 1).replacen(
        "name = \"ISO A3 landscape · first angle\"",
        "name = \"Office A3\"",
        1,
    );
    let files: Vec<PathBuf> = ["office.toml", "taken.toml", "broken.toml"].iter().map(|n| dir.join(n)).collect();
    std::fs::write(&files[0], office).unwrap();
    std::fs::write(&files[1], &builtin).unwrap();
    std::fs::write(&files[2], "id = ").unwrap();
    assert!(use_files(files.clone(), Vec::new()));
    let lib = library::library();
    assert_eq!(library::template("office-a3").name, "Office A3");
    assert_eq!(library::template("iso-a3").name, "ISO A3 landscape · first angle", "built-ins stay");
    assert_eq!(lib.problems.len(), 2, "{:?}", lib.problems);
    assert!(lib.problems[0].contains("already taken") && lib.problems[1].contains("broken.toml"), "{:?}", lib.problems);
    assert!(!use_files(Vec::new(), Vec::new()), "too late once the library is built");
    let _ = std::fs::remove_dir_all(&dir);
}
