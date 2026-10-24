//! Process-crash readability, ownership and privacy at the persisted journal boundary.

use serde_json::json;
use soliddraft_document::journal::{self, Identity, Record, Session};
use std::io::Write;

fn folder() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("soliddraft-journal-{}", soliddraft_document::DocumentId::new()))
}

#[test]
fn live_sessions_are_not_offered_and_unfinished_intents_survive_a_torn_tail() {
    let root = folder();
    let session = Session::start(&root).unwrap();
    let path = session.path().to_owned();
    assert!(journal::abandoned(&root).is_empty(), "another running process must not offer this journal");
    let id = Identity::default();
    journal::register(&id, "part", || json!({"features":[]}));
    journal::begin(&id, || json!({"op":"undo"})).finish(Err::<(), _>("nothing to undo")).unwrap_err();
    drop(journal::begin(&id, || json!({"op":"execute"})));
    drop(session);
    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"record\":\"res").unwrap();
    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(&[0xe2]).unwrap();
    let records = journal::read(&path).unwrap();
    assert!(matches!(&records[2], Record::Result { sequence:1, error:Some(e) } if e == "nothing to undo"));
    assert!(matches!(&records[3], Record::Command { sequence: 2, .. }));
    assert_eq!(journal::abandoned(&root), [path.clone()]);
    // A malformed complete record is corruption, not an interrupted write to ignore.
    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"\n").unwrap();
    assert!(journal::read(&path).unwrap_err().contains("journal record 5"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn report_export_replaces_paths_even_in_embedded_commands_and_errors() {
    let root = folder();
    let session = Session::start(&root).unwrap();
    let source = session.path().to_owned();
    let id = Identity::default();
    journal::register(&id, "part", || {
        json!({"format":"soliddraft-part/2", "depth":12.0,
        "reference":{"kind":"part","path":"../private-design/plate.dwprt","pinned":"abc"},
        "name":"/home/example-person/My parts/plate.dwprt"})
    });
    let command = json!({"op":"rename", "name":"/home/example-person/My parts/plate.dwprt"});
    journal::begin(&id, || json!({"command_json":command.to_string(),
        "windows":"C:\\Users\\example-person\\Secret part.dwprt", "unc":"\\\\example-host\\private\\part.dwprt", "home":"~/private/part.dwprt"}))
        .finish(Err::<(), _>("no feature named `/home/example-person/My parts/plate.dwprt`" )).unwrap_err();
    drop(session);
    let report = root.join("report.jsonl");
    journal::export(&source, &report).unwrap();
    let text = std::fs::read_to_string(&report).unwrap();
    for secret in ["example-person", "example-host", "private-design", "Secret part", "My parts", "~/private"] {
        assert!(!text.contains(secret), "report leaked {secret}: {text}");
    }
    let records = journal::read(&report).unwrap();
    let Record::Document { state, .. } = &records[0] else { panic!() };
    assert_eq!(state["format"], "soliddraft-part/2");
    assert_eq!(state["depth"], 12.0);
    let Record::Command { command, .. } = &records[1] else { panic!() };
    let nested: serde_json::Value = serde_json::from_str(command["command_json"].as_str().unwrap()).unwrap();
    assert_eq!(nested["name"], state["name"], "one path keeps its identity in the report");
    let Record::Result { error: Some(error), .. } = &records[2] else { panic!() };
    assert_eq!(error, &format!("no feature named `{}`", state["name"].as_str().unwrap()));
    assert!(
        std::fs::read_to_string(&source).unwrap().contains("example-person"),
        "export must not edit the local replay source"
    );
    assert!(journal::export(&source, &report).is_err(), "an existing report is never overwritten");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn clean_close_removes_only_its_own_journal() {
    let root = folder();
    let crashed = Session::start(&root).unwrap();
    let kept = crashed.path().to_owned();
    drop(crashed);
    let clean = Session::start(&root).unwrap();
    let removed = clean.path().to_owned();
    clean.close();
    assert!(!removed.exists());
    assert_eq!(journal::abandoned(&root), [kept]);
    std::fs::remove_dir_all(root).unwrap();
}
