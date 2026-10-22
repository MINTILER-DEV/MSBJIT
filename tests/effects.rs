use msbjit::{
    effects::GraphicEffects,
    input::InputEvent,
    project::Project,
    runtime::{self, RuntimeOptions},
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn blocks(actions: Vec<Value>) -> Value {
    let mut blocks = json!({"hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"a0"}});
    let count = actions.len();
    for (i, mut action) in actions.into_iter().enumerate() {
        if i + 1 < count {
            action["next"] = json!(format!("a{}", i + 1));
        }
        blocks[format!("a{i}")] = action;
    }
    blocks
}
fn effect(name: &str, value: Value, change: bool) -> Value {
    let mut b = json!({"opcode":if change {"looks_changeeffectby"} else {"looks_seteffectto"},"fields":{"EFFECT":[name]},"inputs":{}});
    b["inputs"][if change { "CHANGE" } else { "VALUE" }] =
        json!([1, [10, value.to_string().trim_matches('"')]]);
    b
}
#[test]
fn effects_coerce_clamp_accumulate_and_clear_per_target() {
    let p: Project=serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true,"blocks":blocks(vec![effect("brightness",json!(-40),false)])},
        {"name":"Sprite","blocks":blocks(vec![
            effect("CoLoR",json!("-425"),false),effect("color",json!(20),true),
            effect("fisheye",json!(-200),false),effect("whirl",json!(1080),false),
            effect("pixelate",json!(-25),false),effect("mosaic",json!(99999),false),
            effect("brightness",json!(200),false),effect("brightness",json!(-50),true),
            effect("ghost",json!(150),false),effect("ghost",json!(-25),true),
            effect("unknown",json!(999),false)
        ])},
        {"name":"Cleared","blocks":blocks(vec![effect("ghost",json!(95),false),json!({"opcode":"looks_cleargraphiceffects"})])}
    ]})).unwrap();
    let r = runtime::spawn(
        Arc::new(p),
        RuntimeOptions {
            turbo: true,
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert_eq!(r.sprites[0].effects.brightness, -40.0);
    assert_eq!(
        r.sprites[1].effects,
        GraphicEffects {
            color: -405.0,
            fisheye: -200.0,
            whirl: 1080.0,
            pixelate: -25.0,
            mosaic: 99999.0,
            brightness: 50.0,
            ghost: 75.0
        }
    );
    assert_eq!(r.sprites[2].effects, GraphicEffects::default());
}
#[test]
fn green_flag_and_stop_all_reset_effects_but_script_completion_does_not() {
    let p: Arc<Project>=Arc::new(serde_json::from_value(json!({"targets":[{"name":"Stage","isStage":true,"blocks":{
        "a":{"opcode":"event_whenkeypressed","topLevel":true,"fields":{"KEY_OPTION":["a"]},"next":"ghost"},
        "ghost":{"opcode":"looks_seteffectto","fields":{"EFFECT":["ghost"]},"inputs":{"VALUE":[1,[4,"75"]]},"next":"end"},
        "end":{"opcode":"control_stop","fields":{"STOP_OPTION":["this script"]}},
        "s":{"opcode":"event_whenkeypressed","topLevel":true,"fields":{"KEY_OPTION":["s"]},"next":"all"},
        "all":{"opcode":"control_stop","fields":{"STOP_OPTION":["all"]}}
    }}]})).unwrap());
    let run = runtime::spawn_interactive(
        p.clone(),
        RuntimeOptions {
            turbo: true,
            ..Default::default()
        },
    )
    .unwrap();
    let wait = |ghost: f64| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while run.scene.snapshot(&p)[0].effects.ghost != ghost {
            assert!(Instant::now() < deadline, "effects failed to reach {ghost}");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    for reset in [
        InputEvent::Flag,
        InputEvent::Stop,
        InputEvent::Key {
            key: "s".into(),
            down: true,
        },
    ] {
        run.scene.input.send(InputEvent::Release);
        run.scene.input.send(InputEvent::Key {
            key: "a".into(),
            down: true,
        });
        wait(75.0);
        run.scene.input.send(reset);
        wait(0.0);
    }
    run.stop();
    run.join().unwrap();
}
