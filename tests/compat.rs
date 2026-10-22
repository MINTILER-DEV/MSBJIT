use msbjit::{
    project::Project,
    runtime::{self, RuntimeOptions},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[test]
fn named_motion_glides_sample_destination_once_and_property_reads_original() {
    let p:Project=serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true,"variables":{"visited":["visited",0],"property":["property",0]}},
        {"name":"A","blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"goto"},
            "goto":{"opcode":"motion_goto","inputs":{"TO":[1,[10,"B"]]},"next":"save"},
            "save":{"opcode":"data_setvariableto","fields":{"VARIABLE":["visited","visited"]},"inputs":{"VALUE":[2,"x"]},"next":"origin"},
            "x":{"opcode":"motion_xposition"},
            "origin":{"opcode":"motion_gotoxy","inputs":{"X":[1,[4,"0"]],"Y":[1,[4,"0"]]},"next":"point"},
            "point":{"opcode":"motion_pointtowards","inputs":{"TOWARDS":[1,[10,"B"]]},"next":"glide"},
            "glide":{"opcode":"motion_glideto","inputs":{"SECS":[1,[4,"0.1"]],"TO":[1,[10,"B"]]},"next":"read"},
            "read":{"opcode":"data_setvariableto","fields":{"VARIABLE":["property","property"]},"inputs":{"VALUE":[2,"otherX"]}},
            "otherX":{"opcode":"sensing_of","fields":{"PROPERTY":["x position",null]},"inputs":{"OBJECT":[1,[10,"B"]]}}
        }},
        {"name":"B","x":40,"y":60,"blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"wait"},
            "wait":{"opcode":"control_wait","inputs":{"DURATION":[1,[4,"0.02"]]},"next":"move"},
            "move":{"opcode":"motion_setx","inputs":{"X":[1,[4,"100"]]}}
        }}
    ]})).unwrap();
    let r = runtime::spawn(
        Arc::new(p),
        RuntimeOptions {
            turbo: true,
            time_limit: Some(Duration::from_secs(2)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap();
    assert!(!r.interrupted);
    assert_eq!((r.sprites[1].x, r.sprites[1].y), (40.0, 60.0));
    assert!((r.sprites[1].direction - 33.690067525979785).abs() < 1e-9);
    assert_eq!(
        r.variables
            .iter()
            .find(|v| v.id == "visited")
            .unwrap()
            .value
            .to_number(),
        40.0
    );
    assert_eq!(
        r.variables
            .iter()
            .find(|v| v.id == "property")
            .unwrap()
            .value
            .to_number(),
        100.0
    );
}
#[test]
fn relative_layers_move_across_hidden_sprites_and_clamp() {
    let p:Project=serde_json::from_value(json!({"targets":[
        {"name":"Stage","isStage":true},
        {"name":"A","layerOrder":1,"blocks":{
            "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"front"},
            "front":{"opcode":"looks_goforwardbackwardlayers","fields":{"FORWARD_BACKWARD":["forward",null]},"inputs":{"NUM":[1,[4,"2.9"]]}}
        }},
        {"name":"B","layerOrder":2,"visible":false},
        {"name":"C","layerOrder":3},
        {"name":"D","layerOrder":4}
    ]})).unwrap();
    let r = runtime::spawn(Arc::new(p), RuntimeOptions::default())
        .unwrap()
        .join()
        .unwrap();
    assert!(r.sprites[2].layer_order < r.sprites[3].layer_order);
    assert!(r.sprites[3].layer_order < r.sprites[1].layer_order);
    assert!(r.sprites[1].layer_order < r.sprites[4].layer_order);
}
#[test]
fn missing_costume_names_leave_current_costume_unchanged() {
    let p:Project=serde_json::from_value(json!({"targets":[{"name":"Stage","isStage":true,"costumes":[{"name":"first","md5ext":"1.svg"},{"name":"last","md5ext":"2.svg"}],"blocks":{
        "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"missing"},
        "missing":{"opcode":"looks_switchcostumeto","inputs":{"COSTUME":[1,[10,"no such costume"]]}}
    }}]})).unwrap();
    let r = runtime::spawn(Arc::new(p), RuntimeOptions::default())
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(r.sprites[0].costume, 0);
}
