//! Fuzzing recipes, the kernel commands `soliddraft replay`, `view` and `draw` take from a
//! file: the corpus recipes with numbers replaced by extreme ones, values retyped, keys and
//! steps deleted, duplicated, swapped and repeated.
//!
//! A recipe that parses is replayed on the Catallax backend. Nothing may panic or run over
//! two minutes, a refusal is a typed error, and a replay that succeeds returns bodies the
//! kernel has proven valid. A fixed number of cases from a fixed seed run by default; to
//! search further: `PROPTEST_CASES=2000 PROPTEST_RNG_SEED=7 cargo test -p soliddraft-cli --test recipe_fuzz`.

use std::path::Path;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use serde_json::{Number, Value, json};
use soliddraft_kernel_catallax::CatallaxKernel;

const LIMIT: Duration = Duration::from_secs(120);

/// The stack the replay thread gets: a Unix main thread's, which is where the command line replays.
const STACK: usize = 8 * 1024 * 1024;

fn config() -> ProptestConfig {
    // A failing input is printed with the test's seed; nothing is written beside the sources.
    let mut config = ProptestConfig { failure_persistence: None, ..ProptestConfig::default() };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 24;
    }
    if std::env::var_os("PROPTEST_RNG_SEED").is_none() {
        config.rng_seed = RngSeed::Fixed(0x52_43_50);
    }
    config
}

/// The corpus recipes, by file name.
fn corpus() -> Vec<(String, Value)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes");
    let mut all: Vec<(String, Value)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap(),
            )
        })
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(all.len() >= 6, "the corpus is present");
    all
}

/// Every value below the root, as a JSON pointer.
fn pointers(value: &Value) -> Vec<String> {
    fn walk(value: &Value, at: &str, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    let at = format!("{at}/{}", k.replace('~', "~0").replace('/', "~1"));
                    out.push(at.clone());
                    walk(v, &at, out);
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    let at = format!("{at}/{i}");
                    out.push(at.clone());
                    walk(v, &at, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

#[derive(Clone, Debug)]
enum Mutation {
    /// The number at the `at`th number becomes `to`.
    Number { at: usize, to: Value },
    /// The `at`th value becomes `to`.
    Retype { at: usize, to: Value },
    /// The `at`th value is removed from its object or list.
    Delete { at: usize },
    /// The `at`th list element is repeated beside itself.
    Duplicate { at: usize },
    /// Two steps trade places.
    Swap { a: usize, b: usize },
    /// The steps are repeated `times` over.
    Repeat { times: usize },
}

/// Numbers a recipe could be made to hold. Nonzero numbers smaller than 0.01 in size are left
/// out: moved a hair off a round value, a plane origin sends Catallax's boolean region sampling
/// into a recursion that takes a minute or overflows the stack and aborts the process (see
/// `kernel_abort.rs`, which holds the isolated reproduction).
fn extreme() -> impl Strategy<Value = Value> {
    fn float(f: f64) -> Option<Value> {
        Number::from_f64(f).map(Value::Number)
    }
    prop_oneof![
        3 => prop::sample::select(vec![
            0.0, -0.0, -1.0, 1.0, 0.5, 1e6, 1e9, 1e15, 1e100, 1e300, -1e300, f64::MAX,
        ])
        .prop_filter_map("finite", float),
        1 => any::<f64>().prop_filter_map("finite and not tiny", |f| if f == 0.0 || f.abs() >= 0.01 { float(f) } else { None }),
        1 => any::<i64>().prop_map(Value::from),
        1 => any::<u64>().prop_map(Value::from),
    ]
}

fn anything() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        extreme(),
        any::<String>().prop_map(Value::String),
        Just(json!([])),
        Just(json!({})),
        Just(json!([1, 2, 3])),
        Just(json!({ "center": [0, 0] })),
    ]
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        6 => (any::<usize>(), extreme()).prop_map(|(at, to)| Mutation::Number { at, to }),
        2 => (any::<usize>(), anything()).prop_map(|(at, to)| Mutation::Retype { at, to }),
        1 => any::<usize>().prop_map(|at| Mutation::Delete { at }),
        1 => any::<usize>().prop_map(|at| Mutation::Duplicate { at }),
        1 => (any::<usize>(), any::<usize>()).prop_map(|(a, b)| Mutation::Swap { a, b }),
        1 => (2usize..40).prop_map(|times| Mutation::Repeat { times }),
    ]
}

fn mutate(recipe: &mut Value, how: &Mutation) {
    let all = pointers(recipe);
    let numbers: Vec<&String> = all.iter().filter(|p| recipe.pointer(p).is_some_and(Value::is_number)).collect();
    match how {
        Mutation::Number { at, to } if !numbers.is_empty() => {
            let at = numbers[at % numbers.len()].clone();
            *recipe.pointer_mut(&at).unwrap() = to.clone();
        }
        Mutation::Retype { at, to } if !all.is_empty() => {
            *recipe.pointer_mut(&all[at % all.len()]).unwrap() = to.clone();
        }
        Mutation::Delete { at } if !all.is_empty() => {
            let (parent, key) = all[at % all.len()].rsplit_once('/').unwrap();
            match recipe.pointer_mut(parent) {
                Some(Value::Object(map)) => {
                    map.remove(&key.replace("~1", "/").replace("~0", "~"));
                }
                Some(Value::Array(items)) => {
                    if let Ok(i) = key.parse::<usize>() {
                        items.remove(i);
                    }
                }
                _ => {}
            }
        }
        Mutation::Duplicate { at } => {
            let lists: Vec<&String> =
                all.iter().filter(|p| p.rsplit('/').next().is_some_and(|k| k.parse::<usize>().is_ok())).collect();
            if !lists.is_empty() {
                let (parent, key) = lists[at % lists.len()].rsplit_once('/').unwrap();
                let i: usize = key.parse().unwrap();
                if let Some(Value::Array(items)) = recipe.pointer_mut(parent) {
                    let copy = items[i].clone();
                    items.insert(i, copy);
                }
            }
        }
        Mutation::Swap { a, b } => {
            if let Some(Value::Array(steps)) = recipe.get_mut("steps")
                && !steps.is_empty()
            {
                let n = steps.len();
                steps.swap(a % n, b % n);
            }
        }
        Mutation::Repeat { times } => {
            if let Some(Value::Array(steps)) = recipe.get_mut("steps") {
                let once = steps.clone();
                for _ in 1..*times {
                    steps.extend(once.iter().cloned());
                }
            }
        }
        _ => {}
    }
}

/// Parses and replays the text, on a thread of its own; returns the outcome as text.
fn replay(text: String) -> String {
    let (done, finished) = channel();
    let worker = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let outcome = match soliddraft_recipe::parse(&text) {
                Err(e) => {
                    assert!(!e.to_string().trim().is_empty(), "a refused recipe says why");
                    "refused: not a recipe".to_owned()
                }
                Ok(recipe) => match soliddraft_recipe::replay(&CatallaxKernel::new(), &recipe) {
                    Err(e) => {
                        assert!(!e.to_string().trim().is_empty(), "a failed replay says why");
                        "failed: refused by the kernel".to_owned()
                    }
                    Ok(report) => {
                        assert!(
                            report.result.valid,
                            "a replay that succeeded returned a body the kernel did not prove valid"
                        );
                        "replayed".to_owned()
                    }
                },
            };
            let _ = done.send(outcome);
        })
        .expect("a thread to replay on");
    match finished.recv_timeout(LIMIT) {
        Ok(outcome) => {
            worker.join().expect("the replay finished");
            outcome
        }
        Err(RecvTimeoutError::Disconnected) => std::panic::resume_unwind(worker.join().unwrap_err()),
        Err(RecvTimeoutError::Timeout) => panic!("no answer after {LIMIT:?}"),
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn mutated_recipes_never_panic_or_hang(pick in any::<usize>(), mutations in prop::collection::vec(mutation(), 1..5)) {
        let corpus = corpus();
        let (name, original) = &corpus[pick % corpus.len()];
        let mut recipe = original.clone();
        for how in &mutations {
            mutate(&mut recipe, how);
        }
        let text = serde_json::to_string(&recipe).unwrap();
        let what = format!("{name} after {mutations:?}");
        let trace = std::env::var_os("FUZZ_TRACE").is_some();
        if trace {
            eprintln!("replaying {what}");
        }
        let outcome = replay(text);
        if trace {
            eprintln!("{outcome}");
        }
    }

    #[test]
    fn any_text_is_refused_or_parsed_as_a_recipe(text in prop_oneof![any::<String>(), "[\\[\\]{}\",:0-9a-z. -]{0,80}"]) {
        let _ = soliddraft_recipe::parse(&text);
    }
}

#[test]
fn the_corpus_replays_unmutated() {
    // Recipes that use what waits for Catallax (fillets, shells, revolves) parse and the
    // kernel refuses them; the others replay.
    let replays = ["box", "cylinder", "union", "subtract", "bracket"];
    for (name, recipe) in corpus() {
        let outcome = replay(serde_json::to_string(&recipe).unwrap());
        assert!(!outcome.starts_with("refused"), "{name} is a recipe: {outcome}");
        if replays.iter().any(|r| name == format!("{r}.json")) {
            assert_eq!(outcome, "replayed", "{name}");
        }
    }
}
