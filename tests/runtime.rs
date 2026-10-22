use msbjit::{
    project::Project,
    runtime::{self, RunReport, RuntimeOptions},
    value::ScratchValue,
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn project(blocks: Value) -> Project {
    serde_json::from_value(json!({"targets":[{
        "name":"Stage","isStage":true,"variables":{"n":["counter",0],"s":["text",""]},"blocks":blocks
    }]})).unwrap()
}
fn run(blocks: Value, turbo: bool) -> RunReport {
    runtime::spawn(
        Arc::new(project(blocks)),
        RuntimeOptions {
            turbo,
            time_limit: Some(Duration::from_secs(3)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap()
}
fn value<'a>(report: &'a RunReport, id: &str) -> &'a ScratchValue {
    &report.variables.iter().find(|v| v.id == id).unwrap().value
}
#[test]
fn clones_snapshot_local_variables_and_lists_and_share_stage_data() {
    let p: Project = serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true,"variables":{"sum":["sum",0]},"lists":{"results":["results",[]]}},
        {"name":"Sprite","variables":{"local":["local",0]},"lists":{"private":["private",[10]]},"blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"repeat"},
            "repeat":{"opcode":"control_repeat","inputs":{"TIMES":[1,[4,"3"]],"SUBSTACK":[2,"create"]},"next":"originalDelete"},
            "create":{"opcode":"control_create_clone_of","inputs":{"CLONE_OPTION":[1,[10,"_myself_"]]},"next":"increment"},
            "increment":{"opcode":"data_changevariableby","fields":{"VARIABLE":["local","local"]},"inputs":{"VALUE":[1,[4,"1"]]},"next":"replaceOriginal"},
            "replaceOriginal":{"opcode":"data_replaceitemoflist","fields":{"LIST":["private","private"]},"inputs":{"INDEX":[1,[4,"1"]],"ITEM":[3,[12,"local","local"],[4,"0"]]}},
            "originalDelete":{"opcode":"control_delete_this_clone","next":"originalContinues"},
            "originalContinues":{"opcode":"data_changevariableby","fields":{"VARIABLE":["sum","sum"]},"inputs":{"VALUE":[1,[4,"1000"]]}},
            "clone":{"opcode":"control_start_as_clone","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.01"]]},"next":"sum"},
            "sum":{"opcode":"data_changevariableby","fields":{"VARIABLE":["sum","sum"]},"inputs":{"VALUE":[3,[12,"local","local"],[4,"0"]]},"next":"save"},
            "save":{"opcode":"data_addtolist","fields":{"LIST":["results","results"]},"inputs":{"ITEM":[2,"item"]},"next":"replace"},
            "item":{"opcode":"data_itemoflist","fields":{"LIST":["private","private"]},"inputs":{"INDEX":[1,[4,"1"]]}},
            "replace":{"opcode":"data_replaceitemoflist","fields":{"LIST":["private","private"]},"inputs":{"INDEX":[1,[4,"1"]],"ITEM":[1,[4,"99"]]},"next":"append"},
            "append":{"opcode":"data_addtolist","fields":{"LIST":["private","private"]},"inputs":{"ITEM":[1,[10,"clone-only"]]},"next":"saveLength"},
            "saveLength":{"opcode":"data_changevariableby","fields":{"VARIABLE":["sum","sum"]},"inputs":{"VALUE":[2,"length"]},"next":"delete"},
            "length":{"opcode":"data_lengthoflist","fields":{"LIST":["private","private"]}},
            "delete":{"opcode":"control_delete_this_clone","next":"never"},
            "never":{"opcode":"data_changevariableby","fields":{"VARIABLE":["sum","sum"]},"inputs":{"VALUE":[1,[4,"99999"]]}}
        }}
    ]})).unwrap();
    let r = runtime::spawn(
        Arc::new(p),
        RuntimeOptions {
            turbo: true,
            time_limit: Some(Duration::from_secs(3)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert!(!r.interrupted);
    assert_eq!(value(&r, "sum").to_number(), 1009.0);
    assert_eq!(value(&r, "local").to_number(), 3.0);
    assert_eq!(
        r.lists.iter().find(|l| l.id == "private").unwrap().items,
        vec![ScratchValue::Number(3.0)]
    );
    let mut copies: Vec<_> = r
        .lists
        .iter()
        .find(|l| l.id == "results")
        .unwrap()
        .items
        .iter()
        .map(ScratchValue::to_number)
        .collect();
    copies.sort_by(f64::total_cmp);
    assert_eq!(copies, vec![1.0, 2.0, 10.0]);
    assert_eq!(r.sprites.iter().filter(|s| s.is_clone).count(), 3);
    assert!(r.sprites.iter().filter(|s| s.is_clone).all(|s| !s.active));
}

#[test]
fn broadcast_and_wait_runs_receivers_on_each_clone() {
    let p: Project = serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true,"variables":{"n":["counter",0]},"blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"create"},
            "create":{"opcode":"control_repeat","inputs":{"TIMES":[1,[4,"3"]],"SUBSTACK":[2,"clone"]},"next":"broadcast"},
            "clone":{"opcode":"control_create_clone_of","inputs":{"CLONE_OPTION":[1,[10,"Sprite"]]}},
            "broadcast":{"opcode":"event_broadcastandwait","inputs":{"BROADCAST_INPUT":[1,[11,"hello","hello"]]},"next":"multiply"},
            "multiply":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"product"]}},
            "product":{"opcode":"operator_multiply","inputs":{"NUM1":[3,[12,"counter","n"],[4,"0"]],"NUM2":[1,[4,"10"]]}}
        }},
        {"name":"Sprite","blocks":{
            "receive":{"opcode":"event_whenbroadcastreceived","topLevel":true,"fields":{"BROADCAST_OPTION":["hello","hello"]},"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.01"]]},"next":"inc"},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }}
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
    assert_eq!(value(&r, "n").to_number(), 40.0);
    assert_eq!(
        r.sprites.iter().filter(|s| s.is_clone && s.active).count(),
        3
    );
    assert!(
        r.sprites[2..]
            .iter()
            .all(|s| s.layer_order < r.sprites[1].layer_order)
    );
}

#[test]
fn clone_limit_is_silent_and_slots_are_reused() {
    let p: Project = serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true},
        {"name":"Sprite","blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"rounds"},
            "rounds":{"opcode":"control_repeat","inputs":{"TIMES":[1,[4,"2"]],"SUBSTACK":[2,"repeat"]}},
            "repeat":{"opcode":"control_repeat","inputs":{"TIMES":[1,[4,"305"]],"SUBSTACK":[2,"create"]},"next":"clean"},
            "create":{"opcode":"control_create_clone_of","inputs":{"CLONE_OPTION":[1,[10,"_myself_"]]}},
            "clean":{"opcode":"event_broadcastandwait","inputs":{"BROADCAST_INPUT":[1,[11,"clean","clean"]]}},
            "receive":{"opcode":"event_whenbroadcastreceived","topLevel":true,"fields":{"BROADCAST_OPTION":["clean","clean"]},"next":"delete"},
            "delete":{"opcode":"control_delete_this_clone"}
        }}
    ]})).unwrap();
    let r = runtime::spawn(
        Arc::new(p),
        RuntimeOptions {
            turbo: true,
            time_limit: Some(Duration::from_secs(5)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert!(!r.interrupted);
    assert_eq!(r.sprites.len(), 302);
    assert!(r.sprites[2..].iter().all(|s| !s.active));
    assert!(r.sprites[2..].iter().all(|s| s.id >= 302));
}

#[test]
fn glide_keeps_real_duration_in_turbo_and_resumes_at_destination() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"glide"},
            "glide":{"opcode":"motion_glidesecstoxy","inputs":{"SECS":[1,[4,"0.1"]],"X":[1,[4,"80"]],"Y":[1,[4,"-30"]]},"next":"done"},
            "done":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[1,[10,"done"]]}}
        }),
        true,
    );
    assert!(r.elapsed_seconds >= 0.1 && r.elapsed_seconds < 2.0);
    assert_eq!((r.sprites[0].x, r.sprites[0].y), (80.0, -30.0));
    assert_eq!(value(&r, "s").text(), "done");
}

#[test]
fn viewer_language_is_configurable_and_defaults_to_english() {
    let p = Arc::new(project(json!({
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"save"},
        "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[2,"language"]}},
        "language":{"opcode":"translate_getViewerLanguage"}
    })));
    for language in [runtime::DEFAULT_LANGUAGE, "Français"] {
        let r = runtime::spawn(
            p.clone(),
            RuntimeOptions {
                language: language.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .join()
        .unwrap();
        assert_eq!(value(&r, "s").text(), language);
    }
}
#[test]
fn sampled_checkpoints_preserve_exact_step_limits() {
    let p = Arc::new(project(json!({
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"loop"},
        "loop":{"opcode":"control_forever","inputs":{"SUBSTACK":[2,"inc"]}},
        "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
    })));
    for limit in [0, 1, 2, 63, 64, 65, 127, 128, 129, 1025] {
        let r = runtime::spawn(
            p.clone(),
            RuntimeOptions {
                turbo: true,
                max_steps: Some(limit),
                ..Default::default()
            },
        )
        .unwrap()
        .join()
        .unwrap();
        assert_eq!(r.steps, limit, "limit {limit}");
        assert!(r.interrupted);
    }
}

#[test]
fn reused_procedure_arguments_survive_yields_and_collection() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"loop"},
            "loop":{"opcode":"control_repeat","inputs":{"TIMES":[1,[6,"140"]],"SUBSTACK":[2,"call"]}},
            "call":{"opcode":"procedures_call","mutation":{"proccode":"save %s %s","argumentids":"[\"a\",\"b\"]"},"inputs":{"a":[2,"join"],"b":[3,[12,"counter","n"],[4,"0"]]},"next":"inc"},
            "join":{"opcode":"operator_join","inputs":{"STRING1":[1,[10,"v"]],"STRING2":[3,[12,"counter","n"],[10,""]]}},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}},
            "def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"proto"]},"next":"wait"},
            "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"save %s %s","argumentids":"[\"a\",\"b\"]","argumentnames":"[\"text\",\"index\"]","argumentdefaults":"[\"\",0]","warp":"false"}},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0"]]},"next":"set"},
            "arg":{"opcode":"argument_reporter_string_number","fields":{"VALUE":["text",null]}},
            "index":{"opcode":"argument_reporter_string_number","fields":{"VALUE":["index",null]}},
            "suffix":{"opcode":"operator_join","inputs":{"STRING1":[2,"arg"],"STRING2":[2,"index"]}},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[2,"suffix"]}}
        }),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(140.0));
    assert_eq!(value(&r, "s"), &ScratchValue::String("v139139".into()));
}
fn warp_project(body: Value) -> Value {
    let mut blocks = json!({
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"call"},
        "call":{"opcode":"procedures_call","mutation":{"proccode":"fast","argumentids":"[]"}},
        "def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"proto"]},"next":"loop"},
        "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"fast","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":"true"}}
    });
    blocks
        .as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    blocks
}
#[test]
fn native_arithmetic_dynamic_assignment_and_branch() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[10,"12"]]},"next":"change"},
            "change":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"mul"]},"next":"if"},
            "mul":{"opcode":"operator_multiply","inputs":{"NUM1":[1,[4,"3"]],"NUM2":[2,"add"]}},
            "add":{"opcode":"operator_add","inputs":{"NUM1":[1,[4,"4"]],"NUM2":[1,[4,"6"]]}},
            "if":{"opcode":"control_if_else","inputs":{"CONDITION":[2,"equal"],"SUBSTACK":[2,"yes"],"SUBSTACK2":[2,"no"]}},
            "equal":{"opcode":"operator_equals","inputs":{"OPERAND1":[3,[12,"counter","n"],[10,""]],"OPERAND2":[1,[10,"42"]]}},
            "yes":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[1,[10,"ok"]]}},
            "no":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[1,[10,"bad"]]}}
        }),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(42.0));
    assert_eq!(value(&r, "s"), &ScratchValue::String("ok".into()));
    assert!(!r.interrupted);
}
fn repeat_blocks() -> Value {
    json!({
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"loop"},
        "loop":{"opcode":"control_repeat","inputs":{"TIMES":[1,[6,"3"]],"SUBSTACK":[2,"inc"]}},
        "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
    })
}
#[test]
fn normal_loops_yield_and_turbo_removes_frame_boundaries() {
    let normal = run(repeat_blocks(), false);
    let turbo = run(repeat_blocks(), true);
    assert_eq!(value(&normal, "n"), &ScratchValue::Number(3.0));
    assert_eq!(value(&turbo, "n"), value(&normal, "n"));
    assert!(normal.ticks >= 4);
    assert_eq!(turbo.ticks, 1);
    assert!(normal.elapsed_seconds >= 0.08);
}
#[test]
fn nested_repeat_counts_are_independent() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"a"},
            "a":{"opcode":"control_repeat","inputs":{"TIMES":[1,[6,"4"]],"SUBSTACK":[2,"b"]}},
            "b":{"opcode":"control_repeat","inputs":{"TIMES":[1,[6,"5"]],"SUBSTACK":[2,"inc"]}},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(20.0));
}
#[test]
fn warp_removes_normal_loop_yields() {
    let mut body = repeat_blocks();
    body.as_object_mut().unwrap().remove("hat");
    let r = run(warp_project(body), false);
    assert_eq!(value(&r, "n"), &ScratchValue::Number(3.0));
    assert_eq!(r.ticks, 1);
}
#[test]
fn warp_watchdog_yields_and_time_limit_interrupts_empty_infinite_loop() {
    let p = project(warp_project(json!({"loop":{"opcode":"control_forever"}})));
    let now = Instant::now();
    let report = runtime::spawn(
        Arc::new(p),
        RuntimeOptions {
            warp_timeout: Duration::from_millis(20),
            time_limit: Some(Duration::from_millis(100)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert!(report.interrupted);
    assert!(
        report.yields >= 2,
        "warp should have yielded before the time limit"
    );
    assert!(now.elapsed() < Duration::from_secs(3));
}
#[test]
fn stop_request_interrupts_warp() {
    let p = project(warp_project(json!({"loop":{"opcode":"control_forever"}})));
    let running = runtime::spawn(Arc::new(p), RuntimeOptions::default()).unwrap();
    running.stop();
    assert!(running.join().unwrap().interrupted);
}
#[test]
fn explicit_waits_are_honored_in_turbo() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.04"]]},"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"7"]]}}
        }),
        true,
    );
    assert!(r.elapsed_seconds >= 0.035);
    assert_eq!(value(&r, "n"), &ScratchValue::Number(7.0));
}
#[test]
fn broadcast_and_wait_blocks_until_receiver_finishes() {
    let r = run(
        json!({
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"send"},
            "send":{"opcode":"event_broadcastandwait","inputs":{"BROADCAST_INPUT":[1,[11,"go","b"]]},"next":"copy"},
            "copy":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[3,[12,"counter","n"],[10,""]]}},
            "receive":{"opcode":"event_whenbroadcastreceived","topLevel":true,"fields":{"BROADCAST_OPTION":["go","b"]},"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.01"]]},"next":"inc"},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"9"]]}}
        }),
        true,
    );
    assert_eq!(value(&r, "s"), &ScratchValue::Number(9.0));
}
#[test]
fn custom_block_arguments_and_return() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"call"},
            "call":{"opcode":"procedures_call","mutation":{"proccode":"add %s","argumentids":"[\"a\"]"},"inputs":{"a":[1,[10,"17"]]},"next":"inc"},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}},
            "def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"proto"]},"next":"set"},
            "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"add %s","argumentids":"[\"a\"]","argumentnames":"[\"amount\"]","argumentdefaults":"[\"\"]","warp":"false"}},
            "arg":{"opcode":"argument_reporter_string_number","fields":{"VALUE":["amount",null]}},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"arg"]}}
        }),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(18.0));
}
#[test]
fn custom_blocks_ignore_surplus_defaults_and_stale_call_inputs() {
    for supplied in [false, true] {
        let mut blocks = json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"call"},
            "call":{"opcode":"procedures_call","mutation":{"proccode":"save %s","argumentids":"[\"a\"]"},"inputs":{"old_parameter":[1,[4,"999"]]}},
            "def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"proto"]},"next":"set"},
            "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"save %s","argumentids":"[\"a\"]","argumentnames":"[\"amount\"]","argumentdefaults":"[17,999]","warp":"true"}},
            "arg":{"opcode":"argument_reporter_string_number","fields":{"VALUE":["amount",null]}},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"arg"]}}
        });
        if supplied {
            blocks["call"]["inputs"]["a"] = json!([1, [4, "7"]]);
        }
        let report = run(blocks, true);
        assert_eq!(
            value(&report, "n"),
            &ScratchValue::Number(if supplied { 7.0 } else { 17.0 })
        );
    }
}
#[test]
fn missing_custom_block_definitions_do_not_abort_the_script() {
    let report = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"call"},
            "call":{"opcode":"procedures_call","mutation":{"proccode":"removed debug log %s"},"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"42"]]}}
        }),
        true,
    );
    assert_eq!(value(&report, "n"), &ScratchValue::Number(42.0));
}
#[test]
fn unused_custom_block_bodies_are_not_compiled() {
    let report = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"42"]]}},
            "def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"proto"]},"next":"bad"},
            "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"unused","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":"true"}},
            "bad":{"opcode":"unsupported_unused_extension"}
        }),
        true,
    );
    assert_eq!(value(&report, "n"), &ScratchValue::Number(42.0));
}
#[test]
fn stop_this_script_returns_only_from_the_innermost_custom_block() {
    for warp in [false, true] {
        let report = run(
            json!({
                "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"outer"},
                "outer":{"opcode":"procedures_call","mutation":{"proccode":"outer"},"next":"after"},
                "after":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"100"]]},"next":"root_stop"},
                "root_stop":{"opcode":"control_stop","fields":{"STOP_OPTION":["this script",null]},"next":"bad"},
                "bad":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"99999"]]}},
                "outer_def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"outer_proto"]},"next":"loop"},
                "outer_proto":{"opcode":"procedures_prototype","mutation":{"proccode":"outer","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":warp}},
                "loop":{"opcode":"control_repeat","inputs":{"TIMES":[1,[6,"2"]],"SUBSTACK":[2,"inner"]}},
                "inner":{"opcode":"procedures_call","mutation":{"proccode":"inner"},"next":"outer_after"},
                "outer_after":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"10"]]}},
                "inner_def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"inner_proto"]},"next":"inner_inc"},
                "inner_proto":{"opcode":"procedures_prototype","mutation":{"proccode":"inner","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":"false"}},
                "inner_inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]},"next":"stop"},
                "stop":{"opcode":"control_stop","fields":{"STOP_OPTION":["this script",null]},"next":"bad_inner"},
                "bad_inner":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"999"]]}}
            }),
            false,
        );
        assert_eq!(value(&report, "n"), &ScratchValue::Number(122.0));
    }
}

#[test]
fn for_each_has_an_independent_index_and_yields_in_normal_mode() {
    for turbo in [false, true] {
        let report = run(
            json!({
                "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"loop"},
                "loop":{"opcode":"control_for_each","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"2.5"]],"SUBSTACK":[2,"append"]}},
                "append":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[2,"joined"]},"next":"change"},
                "joined":{"opcode":"operator_join","inputs":{"STRING1":[3,[12,"text","s"],[10,""]],"STRING2":[3,[12,"counter","n"],[4,"0"]]}},
                "change":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"999"]]}}
            }),
            turbo,
        );
        assert_eq!(value(&report, "s"), &ScratchValue::String("123".into()));
        assert_eq!(value(&report, "n"), &ScratchValue::Number(999.0));
        assert_eq!(report.ticks > 1, !turbo);
    }
}

#[test]
fn days_since_2000_uses_the_wall_clock_and_missing_arguments_return_zero() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let report = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"days"]},"next":"argument"},
            "days":{"opcode":"sensing_dayssince2000"},
            "argument":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[2,"missing"]}},
            "missing":{"opcode":"argument_reporter_string_number","fields":{"VALUE":["removed parameter",null]}}
        }),
        true,
    );
    let ScratchValue::Number(days) = value(&report, "n") else {
        panic!("expected day count");
    };
    assert!((*days - (now - 946_684_800.0) / 86_400.0).abs() < 0.001);
    assert_eq!(value(&report, "s"), &ScratchValue::Number(0.0));
}
#[test]
fn live_scene_is_available_without_locking_execution() {
    let p = Arc::new(project(json!({
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"move"},
        "move":{"opcode":"motion_gotoxy","inputs":{"X":[1,[4,"100"]],"Y":[1,[4,"-20"]]}}
    })));
    let running = runtime::spawn(p.clone(), RuntimeOptions::default()).unwrap();
    let shared = running.scene.clone();
    running.join().unwrap();
    let state = shared.snapshot(&p);
    assert_eq!(state[0].x, 100.0);
    assert_eq!(state[0].y, -20.0);
}
#[test]
fn cyclic_graphs_and_unsupported_blocks_are_reported() {
    for blocks in [
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"loop"},
            "loop":{"opcode":"looks_show","next":"loop"}
        }),
        json!({"hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"bad"},"bad":{"opcode":"unknown_extension"}}),
    ] {
        assert!(runtime::spawn(Arc::new(project(blocks)), RuntimeOptions::default()).is_err());
    }
}
#[test]
fn strings_survive_yields_and_temporary_strings_are_collected() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"loop"},
            "loop":{"opcode":"control_repeat","inputs":{"TIMES":[1,[6,"5"]],"SUBSTACK":[2,"set"]}},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[2,"join"]}},
            "join":{"opcode":"operator_join","inputs":{"STRING1":[3,[12,"text","s"],[10,""]],"STRING2":[1,[10,"x"]]}}
        }),
        false,
    );
    assert_eq!(value(&r, "s"), &ScratchValue::String("xxxxx".into()));
}
#[test]
fn scratch_coercions() {
    assert_eq!(ScratchValue::String(" 0x10 ".into()).to_number(), 16.0);
    assert_eq!(ScratchValue::String("nonsense".into()).to_number(), 0.0);
    assert_eq!(ScratchValue::Boolean(true).to_number(), 1.0);
    assert!(!ScratchValue::String("FALSE".into()).to_bool());
    assert!(ScratchValue::String(" ".into()).to_bool());
    assert!(
        ScratchValue::String("hello".into())
            .compare(&ScratchValue::String("HELLO".into()))
            .is_eq()
    );
    assert!(
        !ScratchValue::String(" ".into())
            .compare(&ScratchValue::Number(0.0))
            .is_eq()
    );
}

#[test]
fn constant_numeric_coercion_preserves_assignment_and_string_semantics() {
    for literal in [
        "",
        "  ",
        "0x10",
        "-2.5",
        "NaN",
        "nonsense",
        "false",
        "Infinity",
        "-Infinity",
    ] {
        let r = run(
            json!({
                "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"save"},
                "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[1,[10,literal]]},"next":"arithmetic"},
                "arithmetic":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"sum"]}},
                "sum":{"opcode":"operator_add","inputs":{"NUM1":[1,[10,literal]],"NUM2":[1,[10,"2"]]}}
            }),
            true,
        );
        assert_eq!(value(&r, "s").text(), literal);
        assert_eq!(
            value(&r, "n").to_number(),
            ScratchValue::String(literal.into()).to_number() + 2.0,
            "{literal:?}"
        );
    }
}

#[test]
fn numeric_literal_comparisons_keep_whitespace_boolean_and_string_fallbacks() {
    let samples = [
        ScratchValue::Number(0.0),
        ScratchValue::Number(16.0),
        ScratchValue::Number(f64::NAN),
        ScratchValue::Boolean(true),
        ScratchValue::String(" ".into()),
        ScratchValue::String("".into()),
        ScratchValue::String("0x10".into()),
        ScratchValue::String("FALSE".into()),
    ];
    for literal in ["0", "16", "0x10", " ", "false"] {
        let right = ScratchValue::String(literal.into());
        for left in &samples {
            for (opcode, condition) in [
                ("operator_lt", left.compare(&right).is_lt()),
                ("operator_equals", left.compare(&right).is_eq()),
                ("operator_gt", left.compare(&right).is_gt()),
            ] {
                let mut p = project(json!({
                    "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"save"},
                    "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"compare"]}},
                    "compare":{"opcode":opcode,"inputs":{"OPERAND1":[3,[12,"text","s"],[10,""]],"OPERAND2":[1,[10,literal]]}}
                }));
                p.targets[0].variables.get_mut("s").unwrap().1 = left.clone();
                let r = runtime::spawn(Arc::new(p), RuntimeOptions::default())
                    .unwrap()
                    .join()
                    .unwrap();
                assert_eq!(
                    value(&r, "n"),
                    &ScratchValue::Boolean(condition),
                    "{left:?} {opcode} {right:?}"
                );
            }
        }
    }
}

#[test]
fn recursive_procedure_loops_survive_calls_returns_and_scheduler_resumption() {
    let blocks = json!({
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"rootLoop"},
        "rootLoop":{"opcode":"control_repeat","inputs":{"TIMES":[1,[4,"2"]],"SUBSTACK":[2,"rootCall"]}},
        "rootCall":{"opcode":"procedures_call","mutation":{"proccode":"visit %n","argumentids":"[\"depth\"]"},"inputs":{"depth":[1,[10,"3"]]}},
        "def":{"opcode":"procedures_definition","topLevel":true,"inputs":{"custom_block":[1,"proto"]},"next":"recursiveLoop"},
        "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"visit %n","argumentids":"[\"depth\"]","argumentnames":"[\"depth\"]","argumentdefaults":"[1]","warp":"false"}},
        "recursiveLoop":{"opcode":"control_repeat","inputs":{"TIMES":[1,[10,"2"]],"SUBSTACK":[2,"wait"]}},
        "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0"]]},"next":"increment"},
        "increment":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"depth"]},"next":"if"},
        "depth":{"opcode":"argument_reporter_string_number","fields":{"VALUE":["depth",null]}},
        "if":{"opcode":"control_if","inputs":{"CONDITION":[2,"more"],"SUBSTACK":[2,"recurse"]}},
        "more":{"opcode":"operator_gt","inputs":{"OPERAND1":[2,"depth"],"OPERAND2":[1,[4,"1"]]}},
        "recurse":{"opcode":"procedures_call","mutation":{"proccode":"visit %n","argumentids":"[\"depth\"]"},"inputs":{"depth":[2,"minus"]}},
        "minus":{"opcode":"operator_subtract","inputs":{"NUM1":[2,"depth"],"NUM2":[1,[4,"1"]]}}
    });
    for turbo in [false, true] {
        let r = run(blocks.clone(), turbo);
        assert!(!r.interrupted);
        assert_eq!(value(&r, "n").to_number(), 44.0);
    }
}

#[test]
fn compact_variable_and_list_reporters_load_and_execute() {
    let p: Project = serde_json::from_value(json!({"targets":[{
        "name":"Stage","isStage":true,"variables":{"n":["n",0],"s":["s","hello"]},
        "lists":{"letters":["letters",["a","b"]]},
        "blocks":{
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["n","n"]},"inputs":{"VALUE":[2,"variable"]},"next":"copy"},
            "copy":{"opcode":"data_setvariableto","fields":{"VARIABLE":["s","s"]},"inputs":{"VALUE":[2,"list"]}},
            "variable":[12,"s","s",20,30],
            "list":[13,"letters","letters"]
        }
    }]})).unwrap();
    let report = runtime::spawn(Arc::new(p), RuntimeOptions::default())
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(value(&report, "n"), &ScratchValue::String("hello".into()));
    assert_eq!(value(&report, "s"), &ScratchValue::String("ab".into()));
}

#[test]
fn nonwarp_callees_inherit_warp_mode() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"call"},
            "call":{"opcode":"procedures_call","mutation":{"proccode":"outer"}},
            "outer_def":{"opcode":"procedures_definition","topLevel":true,"next":"inner_call","inputs":{"custom_block":[1,"outer_proto"]}},
            "outer_proto":{"opcode":"procedures_prototype","mutation":{"proccode":"outer","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":"true"}},
            "inner_call":{"opcode":"procedures_call","mutation":{"proccode":"inner"}},
            "inner_def":{"opcode":"procedures_definition","topLevel":true,"next":"loop","inputs":{"custom_block":[1,"inner_proto"]}},
            "inner_proto":{"opcode":"procedures_prototype","mutation":{"proccode":"inner","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":"false"}},
            "loop":{"opcode":"control_repeat","inputs":{"TIMES":[1,[6,"3"]],"SUBSTACK":[2,"inc"]}},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }),
        false,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(3.0));
    assert_eq!(r.ticks, 1);
}
#[test]
fn recursive_custom_blocks_resume_their_callers() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"call"},
            "call":{"opcode":"procedures_call","mutation":{"proccode":"recurse"}},
            "def":{"opcode":"procedures_definition","topLevel":true,"next":"if","inputs":{"custom_block":[1,"proto"]}},
            "proto":{"opcode":"procedures_prototype","mutation":{"proccode":"recurse","argumentids":"[]","argumentnames":"[]","argumentdefaults":"[]","warp":"true"}},
            "if":{"opcode":"control_if","inputs":{"CONDITION":[2,"lt"],"SUBSTACK":[2,"inc"]}},
            "lt":{"opcode":"operator_lt","inputs":{"OPERAND1":[3,[12,"counter","n"],[10,""]],"OPERAND2":[1,[4,"5"]]}},
            "inc":{"opcode":"data_changevariableby","next":"again","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}},
            "again":{"opcode":"procedures_call","mutation":{"proccode":"recurse"}}
        }),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(5.0));
}
#[test]
fn cancellation_interrupts_an_executing_warp_loop() {
    let p = project(warp_project(json!({"loop":{"opcode":"control_forever"}})));
    let running = runtime::spawn(Arc::new(p), RuntimeOptions::default()).unwrap();
    let start = Instant::now();
    while running
        .scene
        .steps
        .load(std::sync::atomic::Ordering::Relaxed)
        == 0
        && start.elapsed() < Duration::from_secs(2)
    {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        running
            .scene
            .steps
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0,
        "warp did not start"
    );
    running.stop();
    assert!(running.join().unwrap().interrupted);
}
#[test]
fn sb3_assets_and_normal_demo_execute() {
    let p =
        Arc::new(Project::load(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/demo.sb3")).unwrap());
    assert_eq!(p.assets.len(), 2);
    let r = runtime::spawn(
        p,
        RuntimeOptions {
            max_steps: Some(100),
            turbo: true,
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert_eq!(r.steps, 100);
    assert!(r.interrupted);
    assert_ne!(r.sprites[1].x, -200.0);
}
#[test]
fn sprite_variables_and_stage_fallback_resolve_by_id() {
    let p:Project=serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true,"variables":{"global":["g",2]},"blocks":{}},
        {"name":"Sprite","variables":{"local":["l",3]},"blocks":{
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["l","local"]},"inputs":{"VALUE":[3,[12,"g","global"],[10,""]]},"next":"change"},
            "change":{"opcode":"data_changevariableby","fields":{"VARIABLE":["g","global"]},"inputs":{"VALUE":[1,[4,"4"]]}}
        }}
    ]})).unwrap();
    let r = runtime::spawn(Arc::new(p), RuntimeOptions::default())
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(value(&r, "global"), &ScratchValue::Number(6.0));
    assert_eq!(value(&r, "local"), &ScratchValue::Number(2.0));
}

#[test]
fn wait_until_allows_other_scripts_to_run_in_turbo() {
    let r = run(
        json!({
            "a_flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait_until","inputs":{"CONDITION":[2,"eq"]},"next":"copy"},
            "eq":{"opcode":"operator_equals","inputs":{"OPERAND1":[3,[12,"counter","n"],[10,""]],"OPERAND2":[1,[4,"1"]]}},
            "copy":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[1,[10,"ready"]]}},
            "z_flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }),
        true,
    );
    assert!(!r.interrupted);
    assert_eq!(value(&r, "s"), &ScratchValue::String("ready".into()));
}

#[test]
fn numeric_fast_path_handles_negative_numbers_booleans_and_nan() {
    let r = run(
        json!({
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set"},
            "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"nan"]},"next":"change"},
            "nan":{"opcode":"operator_divide","inputs":{"NUM1":[1,[4,"0"]],"NUM2":[1,[4,"0"]]}},
            "change":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[2,"sum"]}},
            "sum":{"opcode":"operator_add","inputs":{"NUM1":[1,[4,"-6"]],"NUM2":[2,"eq"]}},
            "eq":{"opcode":"operator_equals","inputs":{"OPERAND1":[1,[4,"1"]],"OPERAND2":[1,[10,"1"]]}}
        }),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(-5.0));
}
#[test]
fn native_math_boolean_and_comparison_reporters_preserve_coercions() {
    let cases = [
        (
            json!({"opcode":"operator_mathop","fields":{"OPERATOR":["abs",null]},"inputs":{"NUM":[1,[4,"-3.2"]]}}),
            ScratchValue::Number(3.2),
        ),
        (
            json!({"opcode":"operator_mathop","fields":{"OPERATOR":["floor",null]},"inputs":{"NUM":[1,[4,"-3.2"]]}}),
            ScratchValue::Number(-4.0),
        ),
        (
            json!({"opcode":"operator_mathop","fields":{"OPERATOR":["ceiling",null]},"inputs":{"NUM":[1,[10,"-3.2"]]}}),
            ScratchValue::Number(-3.0),
        ),
        (
            json!({"opcode":"operator_mathop","fields":{"OPERATOR":["sqrt",null]},"inputs":{"NUM":[1,[4,"81"]]}}),
            ScratchValue::Number(9.0),
        ),
        (
            json!({"opcode":"operator_round","inputs":{"NUM":[1,[4,"-1.5"]]}}),
            ScratchValue::Number(-1.0),
        ),
        (
            json!({"opcode":"operator_lt","inputs":{"OPERAND1":[1,[4,"-2"]],"OPERAND2":[1,[4,"1"]]}}),
            ScratchValue::Boolean(true),
        ),
        (
            json!({"opcode":"operator_gt","inputs":{"OPERAND1":[1,[4,"-2"]],"OPERAND2":[1,[4,"1"]]}}),
            ScratchValue::Boolean(false),
        ),
        (
            json!({"opcode":"operator_equals","inputs":{"OPERAND1":[1,[4,"0"]],"OPERAND2":[1,[10," "]]}}),
            ScratchValue::Boolean(false),
        ),
        (
            json!({"opcode":"operator_not","inputs":{"OPERAND":[1,[10,"FALSE"]]}}),
            ScratchValue::Boolean(true),
        ),
        (
            json!({"opcode":"operator_and","inputs":{"OPERAND1":[2,"truth"],"OPERAND2":[1,[4,"-1"]]}}),
            ScratchValue::Boolean(true),
        ),
        (
            json!({"opcode":"operator_or","inputs":{"OPERAND1":[1,[4,"0"]],"OPERAND2":[1,[10,"0"]]}}),
            ScratchValue::Boolean(false),
        ),
    ];
    let mut blocks = json!({
        "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"set0"},
        "truth":{"opcode":"operator_equals","inputs":{"OPERAND1":[1,[4,"1"]],"OPERAND2":[1,[4,"1"]]}}
    });
    let mut p = project(json!({}));
    // Build one straight-line project so both fast and coercing paths execute
    // in the same native function, with each result independently observable.
    for (i, (reporter, _)) in cases.iter().enumerate() {
        let id = format!("case{i}");
        p.targets[0]
            .variables
            .insert(id.clone(), (id.clone(), ScratchValue::Number(0.0)));
        blocks[format!("reporter{i}")] = reporter.clone();
        let mut set = json!({"opcode":"data_setvariableto","fields":{"VARIABLE":[id,id]},"inputs":{"VALUE":[2,format!("reporter{i}")]}});
        if i + 1 < cases.len() {
            set["next"] = json!(format!("set{}", i + 1));
        }
        blocks[format!("set{i}")] = set;
    }
    p.targets[0].blocks = serde_json::from_value(blocks).unwrap();
    let report = runtime::spawn(Arc::new(p), RuntimeOptions::default())
        .unwrap()
        .join()
        .unwrap();
    for (i, (_, expected)) in cases.iter().enumerate() {
        assert_eq!(
            value(&report, &format!("case{i}")),
            expected,
            "reporter {i}"
        );
    }
}
#[test]
fn restarted_broadcast_receiver_keeps_waiters_suspended() {
    let r = run(
        json!({
            "a_flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"send_wait"},
            "send_wait":{"opcode":"event_broadcastandwait","inputs":{"BROADCAST_INPUT":[1,[11,"go","b"]]},"next":"copy"},
            "copy":{"opcode":"data_setvariableto","fields":{"VARIABLE":["text","s"]},"inputs":{"VALUE":[3,[12,"counter","n"],[10,""]]}},
            "z_flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"delay"},
            "delay":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.01"]]},"next":"restart"},
            "restart":{"opcode":"event_broadcast","inputs":{"BROADCAST_INPUT":[1,[11,"go","b"]]}},
            "receiver":{"opcode":"event_whenbroadcastreceived","topLevel":true,"fields":{"BROADCAST_OPTION":["go","b"]},"next":"sleep"},
            "sleep":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.03"]]},"next":"inc"},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["counter","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }),
        true,
    );
    assert_eq!(value(&r, "s"), &ScratchValue::Number(1.0));
    assert_eq!(value(&r, "n"), &ScratchValue::Number(1.0));
}
