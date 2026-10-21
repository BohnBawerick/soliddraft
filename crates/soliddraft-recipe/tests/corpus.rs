//! Every recipe in the corpus parses, passes its structural check and survives a
//! round trip through JSON. Needs no kernel.

use std::path::Path;

#[test]
fn corpus_recipes_parse_and_round_trip() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let recipe = soliddraft_recipe::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(path.file_stem().unwrap().to_str().unwrap(), recipe.name, "file name matches recipe name");
        let again = soliddraft_recipe::parse(&serde_json::to_string(&recipe).unwrap()).unwrap();
        assert_eq!(recipe, again);
        count += 1;
    }
    assert!(count >= 6, "the exit-test recipes are present");
}
