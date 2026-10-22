use msbjit::{
    project::Project,
    runtime::{self, RuntimeOptions},
};
use serde_json::{Value, json};
use std::{io::Cursor, sync::Arc, time::Duration};

fn fixture(blocks: Value, seconds: f64) -> Arc<Project> {
    let mut p:Project=serde_json::from_value(json!({"targets":[{"name":"Stage","isStage":true,"variables":{"done":["done",0],"other":["other",0]},"sounds":[{"name":"tone","md5ext":"tone.wav"}],"blocks":blocks}]})).unwrap();
    let mut data = Cursor::new(Vec::new());
    {
        let mut wav = hound::WavWriter::new(
            &mut data,
            hound::WavSpec {
                channels: 1,
                sample_rate: 8000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..(seconds * 8000.0) as usize {
            wav.write_sample(0i16).unwrap();
        }
        wav.finalize().unwrap();
    }
    p.assets.insert("tone.wav".into(), data.into_inner());
    Arc::new(p)
}
fn execute(p: Arc<Project>) -> runtime::RunReport {
    runtime::spawn(
        p,
        RuntimeOptions {
            turbo: true,
            time_limit: Some(Duration::from_secs(3)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap()
}
fn value(r: &runtime::RunReport, id: &str) -> f64 {
    r.variables
        .iter()
        .find(|v| v.id == id)
        .unwrap()
        .value
        .to_number()
}

#[test]
fn play_until_done_waits_in_turbo_without_blocking_other_scripts() {
    let p = fixture(
        json!({
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"play"},
            "play":{"opcode":"sound_playuntildone","inputs":{"SOUND_MENU":[1,[10,"tone"]]},"next":"done"},
            "done":{"opcode":"data_setvariableto","fields":{"VARIABLE":["done","done"]},"inputs":{"VALUE":[1,[4,"1"]]}},
            "otherFlag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.02"]]},"next":"save"},
            "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["other","other"]},"inputs":{"VALUE":[3,[12,"done","done"],[4,"0"]]}}
        }),
        0.08,
    );
    let r = execute(p);
    assert!(!r.interrupted);
    assert!(r.elapsed_seconds >= 0.075 && r.elapsed_seconds < 1.0);
    assert_eq!(value(&r, "done"), 1.0);
    assert_eq!(value(&r, "other"), 0.0);
}
#[test]
fn missing_sound_is_immediate_and_start_sound_does_not_wait() {
    for opcode in ["sound_playuntildone", "sound_play"] {
        let p = fixture(
            json!({
                "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"play"},
                "play":{"opcode":opcode,"inputs":{"SOUND_MENU":[1,[10,if opcode=="sound_play" {"tone"}else{"missing"}]]},"next":"done"},
                "done":{"opcode":"data_setvariableto","fields":{"VARIABLE":["done","done"]},"inputs":{"VALUE":[1,[4,"1"]]}}
            }),
            1.0,
        );
        let r = execute(p);
        assert!(r.elapsed_seconds < 0.2);
        assert_eq!(value(&r, "done"), 1.0);
    }
}
#[test]
fn stop_all_sounds_releases_sound_waiters() {
    let p = fixture(
        json!({
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"play"},
            "play":{"opcode":"sound_playuntildone","inputs":{"SOUND_MENU":[1,[10,"tone"]]},"next":"done"},
            "done":{"opcode":"data_setvariableto","fields":{"VARIABLE":["done","done"]},"inputs":{"VALUE":[1,[4,"1"]]}},
            "stopFlag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.02"]]},"next":"stop"},
            "stop":{"opcode":"sound_stopallsounds"}
        }),
        1.0,
    );
    let r = execute(p);
    assert!(r.elapsed_seconds < 0.3);
    assert_eq!(value(&r, "done"), 1.0);
}
#[test]
fn a_long_wait_in_another_script_does_not_delay_audio_completion() {
    let p = fixture(
        json!({
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"play"},
            "play":{"opcode":"sound_playuntildone","inputs":{"SOUND_MENU":[1,[10,"tone"]]},"next":"save"},
            "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["done","done"]},"inputs":{"VALUE":[2,"time"]}},
            "time":{"opcode":"sensing_timer"},
            "otherFlag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.3"]]}}
        }),
        0.08,
    );
    let r = execute(p);
    assert!(value(&r, "done") >= 0.075 && value(&r, "done") < 0.2);
}
#[test]
fn volume_clamps_pitch_changes_duration_and_numeric_sound_indices_wrap() {
    let p = fixture(
        json!({
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"volume"},
            "volume":{"opcode":"sound_setvolumeto","inputs":{"VOLUME":[1,[4,"200"]]},"next":"pitch"},
            "pitch":{"opcode":"sound_seteffectto","fields":{"EFFECT":["PITCH",null]},"inputs":{"VALUE":[1,[4,"120"]]},"next":"play"},
            "play":{"opcode":"sound_playuntildone","inputs":{"SOUND_MENU":[1,[10,"0tail"]]},"next":"save"},
            "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["done","done"]},"inputs":{"VALUE":[2,"read"]}},
            "read":{"opcode":"sound_volume"}
        }),
        0.2,
    );
    let r = execute(p);
    assert!(r.elapsed_seconds >= 0.09 && r.elapsed_seconds < 0.3);
    assert_eq!(value(&r, "done"), 100.0);
}
#[test]
#[cfg(windows)]
#[ignore = "requires a Windows audio output device"]
fn native_audio_completion_resumes_the_script() {
    let p = fixture(
        json!({
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"play"},
            "play":{"opcode":"sound_playuntildone","inputs":{"SOUND_MENU":[1,[10,"tone"]]},"next":"done"},
            "done":{"opcode":"data_setvariableto","fields":{"VARIABLE":["done","done"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }),
        0.08,
    );
    let running = runtime::spawn_interactive(
        p,
        RuntimeOptions {
            turbo: true,
            time_limit: Some(Duration::from_millis(500)),
            ..Default::default()
        },
    )
    .unwrap();
    running.scene.input.send(msbjit::input::InputEvent::Flag);
    let report = running.join().unwrap();
    assert_eq!(
        report
            .variables
            .iter()
            .find(|v| v.id == "done")
            .unwrap()
            .value,
        msbjit::value::ScratchValue::Number(1.0)
    );
}
