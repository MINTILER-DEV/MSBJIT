use msbjit::{
    input::InputEvent,
    project::Project,
    runtime::{self, QuestionPrompt, RuntimeOptions, SharedScene},
};
use serde_json::json;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn project() -> Arc<Project> {
    Arc::new(serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true,"variables":{"a":["a",0],"b":["b",0],"other":["other",0]},"blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"ask"},
            "ask":{"opcode":"sensing_askandwait","inputs":{"QUESTION":[1,[10,"First?"]]},"next":"save"},
            "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["a","a"]},"inputs":{"VALUE":[2,"answer"]}},
            "answer":{"opcode":"sensing_answer"},
            "flag2":{"opcode":"event_whenflagclicked","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.02"]]},"next":"other"},
            "other":{"opcode":"data_setvariableto","fields":{"VARIABLE":["other","other"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }},
        {"name":"Sprite","blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"ask"},
            "ask":{"opcode":"sensing_askandwait","inputs":{"QUESTION":[1,[10,"Second?"]]},"next":"save"},
            "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["b","b"]},"inputs":{"VALUE":[2,"answer"]}},
            "answer":{"opcode":"sensing_answer"}
        }}
    ]})).unwrap())
}
fn prompt(scene: &SharedScene, previous: Option<u64>) -> QuestionPrompt {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(q) = scene.question.lock().unwrap().clone()
            && Some(q.id) != previous
        {
            return q;
        }
        assert!(Instant::now() < deadline, "question not published");
        thread::sleep(Duration::from_millis(2));
    }
}
fn options() -> RuntimeOptions {
    RuntimeOptions {
        turbo: true,
        time_limit: Some(Duration::from_secs(3)),
        ..Default::default()
    }
}

#[test]
fn questions_queue_yield_and_preserve_unicode_answers() {
    let running = runtime::spawn_interactive(project(), options()).unwrap();
    running.scene.input.send(InputEvent::Flag);
    let first = prompt(&running.scene, None);
    assert_eq!(first.text, "First?");
    thread::sleep(Duration::from_millis(40));
    running.scene.input.send(InputEvent::Answer {
        id: first.id,
        text: "Player 星".into(),
    });
    let second = prompt(&running.scene, Some(first.id));
    assert_eq!(second.text, "Second?");
    thread::sleep(Duration::from_millis(10));
    running.scene.input.send(InputEvent::Answer {
        id: second.id,
        text: "42".into(),
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while running.scene.question.lock().unwrap().is_some() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    thread::sleep(Duration::from_millis(10));
    running.stop();
    let report = running.join().unwrap();
    for (id, expected) in [("a", "Player 星"), ("b", "42"), ("other", "1")] {
        assert_eq!(
            report
                .variables
                .iter()
                .find(|v| v.id == id)
                .unwrap()
                .value
                .text(),
            expected
        );
    }
}

#[test]
fn flag_cancels_old_prompts_and_stale_submissions_are_ignored() {
    let running = runtime::spawn_interactive(project(), options()).unwrap();
    running.scene.input.send(InputEvent::Flag);
    let old = prompt(&running.scene, None);
    running.scene.input.send(InputEvent::Flag);
    let fresh = prompt(&running.scene, Some(old.id));
    running.scene.input.send(InputEvent::Answer {
        id: old.id,
        text: "stale".into(),
    });
    thread::sleep(Duration::from_millis(20));
    assert_eq!(
        running.scene.question.lock().unwrap().as_ref().unwrap().id,
        fresh.id
    );
    running.scene.input.send(InputEvent::Answer {
        id: fresh.id,
        text: "new".into(),
    });
    let next = prompt(&running.scene, Some(fresh.id));
    running.scene.input.send(InputEvent::Answer {
        id: next.id,
        text: String::new(),
    });
    thread::sleep(Duration::from_millis(20));
    running.stop();
    let report = running.join().unwrap();
    assert_eq!(
        report
            .variables
            .iter()
            .find(|v| v.id == "a")
            .unwrap()
            .value
            .text(),
        "new"
    );
    assert_eq!(
        report
            .variables
            .iter()
            .find(|v| v.id == "b")
            .unwrap()
            .value
            .text(),
        ""
    );
}

#[test]
fn headless_questions_wait_for_input_instead_of_fabricating_answers() {
    let report = runtime::spawn(
        project(),
        RuntimeOptions {
            time_limit: Some(Duration::from_millis(70)),
            ..options()
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert!(report.interrupted);
    assert_eq!(
        report
            .variables
            .iter()
            .find(|v| v.id == "a")
            .unwrap()
            .value
            .to_number(),
        0.0
    );
}

#[test]
fn distance_reports_mouse_sprite_and_missing_targets() {
    let p:Project=serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true,"variables":{"mouse":["mouse",0],"sprite":["sprite",0],"missing":["missing",0]},"blocks":{}},
        {"name":"A","x":3,"y":4,"blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"s1"},
            "s1":{"opcode":"data_setvariableto","fields":{"VARIABLE":["mouse","mouse"]},"inputs":{"VALUE":[2,"m"]},"next":"s2"},
            "m":{"opcode":"sensing_distanceto","inputs":{"DISTANCETOMENU":[1,[10,"_mouse_"]]}},
            "s2":{"opcode":"data_setvariableto","fields":{"VARIABLE":["sprite","sprite"]},"inputs":{"VALUE":[2,"b"]},"next":"s3"},
            "b":{"opcode":"sensing_distanceto","inputs":{"DISTANCETOMENU":[1,[10,"B"]]}},
            "s3":{"opcode":"data_setvariableto","fields":{"VARIABLE":["missing","missing"]},"inputs":{"VALUE":[2,"bad"]}},
            "bad":{"opcode":"sensing_distanceto","inputs":{"DISTANCETOMENU":[1,[10,"missing"]]}}
        }},{"name":"B","x":6,"y":8}
    ]})).unwrap();
    let report = runtime::spawn(Arc::new(p), options())
        .unwrap()
        .join()
        .unwrap();
    for (id, expected) in [("mouse", 5.0), ("sprite", 5.0), ("missing", 10000.0)] {
        assert_eq!(
            report
                .variables
                .iter()
                .find(|v| v.id == id)
                .unwrap()
                .value
                .to_number(),
            expected
        );
    }
}
