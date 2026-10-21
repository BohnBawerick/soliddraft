//! Legacy settings are read through the CLI in an isolated config directory.

use std::path::Path;
use std::process::Command;

fn run(home: &Path, args: &[&str], vars: &[(&str, &str)]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_soliddraft"));
    for suffix in ["CONFIG_DIR", "PROVIDER", "MODEL", "NO_KEYCHAIN"] {
        for brand in ["SOLIDDRAFT", "DRAFTWELL"] {
            command.env_remove(format!("{brand}_{suffix}"));
        }
    }
    command
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("APPDATA", home.join("config"))
        .env("DRAFTWELL_NO_KEYCHAIN", "1")
        .env("ANTHROPIC_API_KEY", "test-placeholder")
        .env("OPENROUTER_API_KEY", "test-placeholder")
        .envs(vars.iter().copied());
    let output = command.output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn old_settings_and_environment_work_until_current_values_exist() {
    let home = std::env::temp_dir().join(format!("soliddraft-compatibility-{}", std::process::id()));
    let base = if cfg!(target_os = "macos") { home.join("Library/Application Support") } else { home.join("config") };
    let legacy = base.join("draftwell/settings.json");
    let current = base.join("soliddraft/settings.json");
    let old = r#"{"assistant":{"provider":"openrouter","openrouter_model":"old-saved-model"},"view":{"preset":"realistic"},"future_setting":true}"#;
    soliddraft_document::save_atomic(&legacy, old.as_bytes()).unwrap();
    let library = base.join("draftwell/libraries/my-library.toml");
    soliddraft_document::save_atomic(&library, b"format = \"draftwell-library/1\"\nname = \"Old stock\"\n").unwrap();
    assert!(run(&home, &["material", "libraries"], &[]).contains("Old stock"));
    assert!(run(&home, &["key", "status"], &[]).contains("OpenRouter with old-saved-model"));
    assert!(
        run(&home, &["key", "status"], &[("DRAFTWELL_PROVIDER", "anthropic"), ("DRAFTWELL_MODEL", "old-env-model")])
            .contains("Anthropic with old-env-model")
    );
    let both = [
        ("DRAFTWELL_PROVIDER", "anthropic"),
        ("DRAFTWELL_MODEL", "old-env-model"),
        ("SOLIDDRAFT_PROVIDER", "openrouter"),
        ("SOLIDDRAFT_MODEL", "current-env-model"),
    ];
    assert!(run(&home, &["key", "status"], &both).contains("OpenRouter with current-env-model"));
    assert!(
        run(&home, &["key", "status"], &[("DRAFTWELL_MODEL", "old-env-model"), ("SOLIDDRAFT_MODEL", "")])
            .contains("OpenRouter with old-saved-model")
    );

    run(&home, &["locations", "add", "referenced_documents", home.to_str().unwrap()], &[]);
    let mut saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&current).unwrap()).unwrap();
    assert_eq!(saved["assistant"]["openrouter_model"], "old-saved-model");
    assert_eq!(saved["view"]["preset"], "realistic");
    assert_eq!(saved["future_setting"], true);
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), old);
    saved["assistant"]["openrouter_model"] = "current-saved-model".into();
    std::fs::write(&current, saved.to_string()).unwrap();
    assert!(run(&home, &["key", "status"], &[]).contains("OpenRouter with current-saved-model"));

    let override_dir = home.join("override");
    soliddraft_document::save_atomic(&override_dir.join("settings.json"), old.as_bytes()).unwrap();
    let override_text = override_dir.to_str().unwrap();
    assert!(
        run(&home, &["key", "status"], &[("DRAFTWELL_CONFIG_DIR", override_text)])
            .contains("OpenRouter with old-saved-model")
    );
    let current_dir = current.parent().unwrap().to_str().unwrap();
    assert!(
        run(
            &home,
            &["key", "status"],
            &[("DRAFTWELL_CONFIG_DIR", override_text), ("SOLIDDRAFT_CONFIG_DIR", current_dir)]
        )
        .contains("OpenRouter with current-saved-model")
    );
    std::fs::remove_dir_all(home).unwrap();
}
