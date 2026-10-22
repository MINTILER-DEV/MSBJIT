use serde_json::{Value, json};
use std::{
    fs::{File, remove_file},
    io::Write,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = remove_file(&self.0);
    }
}

#[test]
fn cli_username_reaches_native_reporter_with_default_override_and_empty_value() {
    let path = std::env::temp_dir().join(format!(
        "msbjit-username-{}-{}.json",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut file = File::create_new(&path).unwrap();
    let fixture = Fixture(path);
    // Read again after a scheduler yield to exercise the string value ABI.
    let project = json!({"targets":[{"name":"Stage","isStage":true,"variables":{"who":["who",""]},"blocks":{
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
        "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["who","who"]},"inputs":{"VALUE":[2,"username"]},"next":"wait"},
        "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0"]]},"next":"read"},
        "read":{"opcode":"data_setvariableto","fields":{"VARIABLE":["who","who"]},"inputs":{"VALUE":[2,"username"]}},
        "username":{"opcode":"sensing_username"}
    }}]});
    write!(file, "{project}").unwrap();
    drop(file);
    for name in [None, Some("Caf\u{e9} Player"), Some("")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_msbjit"));
        command
            .arg(&fixture.0)
            .args(["--headless", "--turbo", "--json"]);
        if let Some(name) = name {
            command.args(["--username", name]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["variables"][0]["value"], name.unwrap_or("Player1"));
    }
}
