use msbjit::{
    project::Project,
    runtime::{self, RunReport, RuntimeOptions},
    value::ScratchValue,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

fn run(mut commands: Vec<Value>, reporters: Value, items: Value, turbo: bool) -> RunReport {
    let mut blocks = reporters.as_object().unwrap().clone();
    blocks.insert(
        "flag".into(),
        json!({"opcode":"event_whenflagclicked","topLevel":true,"next":"cmd0"}),
    );
    for i in 0..commands.len() {
        if i + 1 < commands.len() {
            commands[i]["next"] = json!(format!("cmd{}", i + 1));
        }
        blocks.insert(format!("cmd{i}"), commands[i].clone());
    }
    let project: Project = serde_json::from_value(json!({"targets":[{
        "name":"Stage","isStage":true,"variables":{"out":["out",0],"n":["n",0],"bad":["bad",0]},
        "lists":{"l":["letters",items]},"blocks":blocks
    }]}))
    .unwrap();
    runtime::spawn(
        Arc::new(project),
        RuntimeOptions {
            turbo,
            time_limit: Some(Duration::from_secs(5)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap()
}
fn list(op: &str, inputs: Value) -> Value {
    json!({"opcode":op,"fields":{"LIST":["letters","l"]},"inputs":inputs})
}
fn set(id: &str, reporter: &str) -> Value {
    json!({"opcode":"data_setvariableto","fields":{"VARIABLE":[id,id]},"inputs":{"VALUE":[2,reporter]}})
}
fn value<'a>(r: &'a RunReport, id: &str) -> &'a ScratchValue {
    &r.variables.iter().find(|v| v.id == id).unwrap().value
}
#[test]
fn native_numeric_string_cache_and_large_handle_fallback_preserve_coercions() {
    let r = run(
        vec![set("out", "sum"), set("n", "boolsum"), set("bad", "last")],
        json!({
            "sum":{"opcode":"operator_add","inputs":{"NUM1":[2,"first"],"NUM2":[2,"last"]}},
            "first":list("data_itemoflist",json!({"INDEX":[1,[10," 1.9 "]]})),
            "last":list("data_itemoflist",json!({"INDEX":[1,[10,"last"]]})),
            "boolsum":{"opcode":"operator_add","inputs":{"NUM1":[2,"bool"],"NUM2":[1,[4,"5"]]}},
            "bool":{"opcode":"operator_equals","inputs":{"OPERAND1":[1,[10,"1"]],"OPERAND2":[1,[10,"1"]]}}
        }),
        json!((0..65_538).map(|i| i.to_string()).collect::<Vec<_>>()),
        true,
    );
    assert_eq!(value(&r, "out"), &ScratchValue::Number(65_537.));
    assert_eq!(value(&r, "n"), &ScratchValue::Number(6.));
    assert_eq!(value(&r, "bad"), &ScratchValue::String("65537".into()));
    assert_eq!(r.lists[0].items[0], ScratchValue::String("0".into()));
}

#[test]
fn native_list_views_follow_reallocations_and_string_transitions() {
    let mut commands = vec![];
    for i in 0..40 {
        commands.push(list(
            "data_addtolist",
            json!({"ITEM":[1,[4,i.to_string()]]}),
        ));
    }
    commands.extend([
        list(
            "data_replaceitemoflist",
            json!({"INDEX":[1,[4,"40.9"]],"ITEM":[2,"joined"]}),
        ),
        json!({"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0"]]}}),
        set("out", "last"),
        list(
            "data_replaceitemoflist",
            json!({"INDEX":[1,[4,"40"]],"ITEM":[1,[4,"123"]]}),
        ),
        list(
            "data_insertatlist",
            json!({"INDEX":[1,[4,"1"]],"ITEM":[1,[4,"-1"]]}),
        ),
        list("data_deleteoflist", json!({"INDEX":[1,[4,"1"]]})),
        set("n", "last"),
        list("data_deletealloflist", json!({})),
        list("data_addtolist", json!({"ITEM":[1,[4,"77"]]})),
        set("bad", "first"),
    ]);
    let r = run(
        commands,
        json!({
            "joined":{"opcode":"operator_join","inputs":{"STRING1":[1,[10,"new"]],"STRING2":[1,[10," string"]]}},
            "last":list("data_itemoflist",json!({"INDEX":[1,[4,"40.9"]]})),
            "first":list("data_itemoflist",json!({"INDEX":[1,[4,"1"]]}))
        }),
        json!([]),
        true,
    );
    assert_eq!(value(&r, "out"), &ScratchValue::String("new string".into()));
    assert_eq!(value(&r, "n"), &ScratchValue::Number(123.0));
    assert_eq!(value(&r, "bad"), &ScratchValue::Number(77.0));
    assert_eq!(r.lists[0].items, vec![ScratchValue::Number(77.0)]);
}

#[test]
fn list_mutations_and_reporters_execute_native_code() {
    let r = run(
        vec![
            list("data_addtolist", json!({"ITEM":[1,[10,"Hi"]]})),
            list(
                "data_replaceitemoflist",
                json!({"INDEX":[1,[4,"2.9"]],"ITEM":[1,[10,"B"]]}),
            ),
            list(
                "data_insertatlist",
                json!({"INDEX":[1,[10,"last"]],"ITEM":[1,[4,"99"]]}),
            ),
            list("data_deleteoflist", json!({"INDEX":[1,[4,"0"]]})),
            list("data_deleteoflist", json!({"INDEX":[1,[4,"1"]]})),
            set("out", "contents"),
            set("n", "index"),
            set("bad", "invalid"),
        ],
        json!({
            "contents":list("data_listcontents",json!({})),
            "index":list("data_itemnumoflist",json!({"ITEM":[1,[10,"hi"]]})),
            "invalid":list("data_itemoflist",json!({"INDEX":[1,[10,"Infinity"]]}))
        }),
        json!(["a", "b", 3]),
        true,
    );
    assert_eq!(value(&r, "out"), &ScratchValue::String("B 3 Hi 99".into()));
    assert_eq!(value(&r, "n"), &ScratchValue::Number(3.0));
    assert_eq!(value(&r, "bad"), &ScratchValue::String("".into()));
    assert_eq!(r.lists[0].items.len(), 4);
}

#[test]
fn list_contents_comparison_and_special_indices() {
    let r = run(
        vec![
            set("out", "contents"),
            set("n", "contains"),
            set("bad", "random"),
        ],
        json!({
            "contents":list("data_listcontents",json!({})),
            "contains":list("data_listcontainsitem",json!({"ITEM":[1,[10,"A"]]})),
            "random":list("data_itemoflist",json!({"INDEX":[1,[10,"any"]]}))
        }),
        json!(["a", "b"]),
        true,
    );
    assert_eq!(value(&r, "out"), &ScratchValue::String("ab".into()));
    assert_eq!(value(&r, "n"), &ScratchValue::Boolean(true));
    assert!(r.lists[0].items.contains(value(&r, "bad")));
    let r = run(
        vec![
            list("data_deleteoflist", json!({"INDEX":[1,[10,"all"]]})),
            set("out", "last"),
            set("n", "len"),
        ],
        json!({
            "last":list("data_itemoflist",json!({"INDEX":[1,[10,"last"]]})),
            "len":list("data_lengthoflist",json!({}))
        }),
        json!([1, 2]),
        true,
    );
    assert_eq!(value(&r, "out"), &ScratchValue::String("".into()));
    assert_eq!(value(&r, "n"), &ScratchValue::Number(0.0));
}

#[test]
fn dynamically_created_list_strings_survive_scheduler_collection() {
    let r = run(
        vec![
            list("data_addtolist", json!({"ITEM":[2,"join"]})),
            json!({"opcode":"control_repeat","inputs":{"TIMES":[1,[4,"3"]]}}),
            set("out", "item"),
        ],
        json!({
            "join":{"opcode":"operator_join","inputs":{"STRING1":[1,[10,"hello"]],"STRING2":[1,[10,"!"]]}},
            "item":list("data_itemoflist",json!({"INDEX":[1,[4,"1"]]}))
        }),
        json!([]),
        false,
    );
    assert_eq!(value(&r, "out"), &ScratchValue::String("hello!".into()));
    assert!(r.ticks >= 3);
}

#[test]
fn list_limit_matches_scratch_and_insert_drops_last_item() {
    let r = run(
        vec![
            list("data_addtolist", json!({"ITEM":[1,[4,"8"]]})),
            list(
                "data_insertatlist",
                json!({"INDEX":[1,[4,"1"]],"ITEM":[1,[4,"7"]]}),
            ),
            set("n", "len"),
        ],
        json!({"len":list("data_lengthoflist",json!({}))}),
        json!(vec![0; 200_000]),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(200_000.0));
    assert_eq!(r.lists[0].items[0], ScratchValue::Number(7.0));
    assert_eq!(r.lists[0].items.last(), Some(&ScratchValue::Number(0.0)));
}

#[test]
fn while_rechecks_condition_and_timed_speech_waits() {
    let r = run(
        vec![
            json!({"opcode":"control_while","inputs":{"CONDITION":[2,"less"],"SUBSTACK":[2,"inc"]}}),
            json!({"opcode":"looks_sayforsecs","inputs":{"MESSAGE":[1,[10,"done"]],"SECS":[1,[4,"0.03"]]}}),
        ],
        json!({
            "less":{"opcode":"operator_lt","inputs":{"OPERAND1":[3,[12,"n","n"],[4,"0"]],"OPERAND2":[1,[4,"3"]]}},
            "inc":{"opcode":"data_changevariableby","fields":{"VARIABLE":["n","n"]},"inputs":{"VALUE":[1,[4,"1"]]}}
        }),
        json!([]),
        true,
    );
    assert_eq!(value(&r, "n"), &ScratchValue::Number(3.0));
    assert_eq!(r.messages, vec!["Stage: done"]);
    assert!(r.elapsed_seconds >= 0.025);
}
