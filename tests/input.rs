use msbjit::{
    input::InputEvent,
    project::Project,
    runtime::{self, RunningProject, RuntimeOptions},
};
use serde_json::json;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn wait_for(run: &RunningProject, project: &Project, expected: impl Fn(f64, f64) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = &run.scene.snapshot(project)[0];
        if expected(state.x, state.y) {
            return;
        }
        assert!(
            Instant::now() < deadline && !run.is_finished(),
            "input did not produce expected scene: {state:?}"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn idle_runtime_accepts_key_click_and_repeated_green_flags() {
    let project: Arc<Project> = Arc::new(serde_json::from_value(json!({"targets":[{
        "name":"Stage", "isStage":true, "variables":{"n":["runs",0]}, "blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"inc"},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["runs","n"]},"inputs":{"VALUE":[1,[4,"1"]]},"next":"flagx"},
            "flagx":{"opcode":"motion_setx","inputs":{"X":[3,[12,"runs","n"],[4,"0"]]},"next":"stopall"},
            "stopall":{"opcode":"control_stop","fields":{"STOP_OPTION":["all"]}},
            "key":{"opcode":"event_whenkeypressed","topLevel":true,"fields":{"KEY_OPTION":["space"]},"next":"keyx"},
            "keyx":{"opcode":"motion_setx","inputs":{"X":[1,[4,"50"]]}},
            "click":{"opcode":"event_whenstageclicked","topLevel":true,"next":"mousex"},
            "mousex":{"opcode":"motion_setx","inputs":{"X":[2,"mx"]},"next":"mousey"},
            "mousey":{"opcode":"motion_sety","inputs":{"Y":[2,"my"]}},
            "mx":{"opcode":"sensing_mousex"}, "my":{"opcode":"sensing_mousey"}
        }
    }]})).unwrap());
    let run = runtime::spawn_interactive(project.clone(), RuntimeOptions::default()).unwrap();
    assert!(!run.is_finished());
    run.scene.input.send(InputEvent::Flag);
    wait_for(&run, &project, |x, _| x == 1.0);
    run.scene.input.send(InputEvent::Key {
        key: " ".into(),
        down: true,
    });
    wait_for(&run, &project, |x, _| x == 50.0);
    run.scene.input.send(InputEvent::Mouse {
        x: 120.0,
        y: -75.0,
        down: true,
    });
    run.scene.input.send(InputEvent::Click { target: 0 });
    wait_for(&run, &project, |x, y| x == 120.0 && y == -75.0);
    run.scene.input.send(InputEvent::Flag);
    wait_for(&run, &project, |x, _| x == 2.0);
    run.stop();
    assert!(run.join().unwrap().interrupted);
}

#[test]
fn key_and_mouse_reporters_release_after_focus_loss() {
    let project: Arc<Project> = Arc::new(serde_json::from_value(json!({"targets":[{"name":"Stage","isStage":true,"blocks":{
        "key":{"opcode":"event_whenkeypressed","topLevel":true,"fields":{"KEY_OPTION":["up arrow"]},"next":"if"},
        "if":{"opcode":"control_if","inputs":{"CONDITION":[2,"and"],"SUBSTACK":[2,"set"]},"next":"wait"},
        "and":{"opcode":"operator_and","inputs":{"OPERAND1":[2,"pressed"],"OPERAND2":[2,"mouse"]}},
        "pressed":{"opcode":"sensing_keypressed","inputs":{"KEY_OPTION":[1,"menu"]}},
        "menu":{"opcode":"sensing_keyoptions","fields":{"KEY_OPTION":["any"]}},
        "mouse":{"opcode":"sensing_mousedown"},
        "set":{"opcode":"motion_setx","inputs":{"X":[1,[4,"1"]]}},
        "wait":{"opcode":"control_wait_until","inputs":{"CONDITION":[2,"not"]},"next":"done"},
        "not":{"opcode":"operator_not","inputs":{"OPERAND":[2,"or"]}},
        "or":{"opcode":"operator_or","inputs":{"OPERAND1":[2,"pressed"],"OPERAND2":[2,"mouse"]}},
        "done":{"opcode":"motion_setx","inputs":{"X":[1,[4,"2"]]}}
    }}]})).unwrap());
    let run = runtime::spawn_interactive(project.clone(), RuntimeOptions::default()).unwrap();
    run.scene.input.send(InputEvent::Mouse {
        x: 0.0,
        y: 0.0,
        down: true,
    });
    run.scene.input.send(InputEvent::Key {
        key: "ArrowUp".into(),
        down: true,
    });
    wait_for(&run, &project, |x, _| x == 1.0);
    run.scene.input.send(InputEvent::Release);
    wait_for(&run, &project, |x, _| x == 2.0);
    run.stop();
    run.join().unwrap();
}

#[test]
fn stop_cancels_scripts_and_green_flag_can_start_them_again() {
    let project: Arc<Project> = Arc::new(
        serde_json::from_value(json!({"targets":[{"name":"Stage","isStage":true,"blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"loop"},
            "loop":{"opcode":"control_forever","inputs":{"SUBSTACK":[2,"move"]}},
            "move":{"opcode":"motion_changexby","inputs":{"DX":[1,[4,"1"]]}}
        }}]}))
        .unwrap(),
    );
    let run = runtime::spawn_interactive(project.clone(), RuntimeOptions::default()).unwrap();
    run.scene.input.send(InputEvent::Flag);
    wait_for(&run, &project, |x, _| x >= 3.0);
    run.scene.input.send(InputEvent::Stop);
    thread::sleep(Duration::from_millis(100));
    let stopped_x = run.scene.snapshot(&project)[0].x;
    thread::sleep(Duration::from_millis(100));
    assert_eq!(run.scene.snapshot(&project)[0].x, stopped_x);
    assert!(!run.is_finished());
    run.scene.input.send(InputEvent::Flag);
    wait_for(&run, &project, |x, _| x > stopped_x);
    run.stop();
    run.join().unwrap();
}
