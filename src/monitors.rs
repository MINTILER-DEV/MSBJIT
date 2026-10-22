//! Variable monitor metadata and execution-thread bindings. UI changes never
//! write through the JIT's raw variable pointer.
use crate::{project::Project, value::ScratchValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Normal,
    Large,
    Slider,
    List,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListEditor {
    pub revision: u64,
    pub index: usize,
    pub text: Option<String>,
    pub cursor: usize,
    pub select_all: bool,
    pub dirty: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListMonitor {
    revision: u64,
    pub width: f64,
    pub height: f64,
    pub len: usize,
    pub first: usize,
    pub items: Vec<String>,
    pub editor: Option<ListEditor>,
}
impl ListMonitor {
    pub fn rows(&self) -> usize {
        ((self.height - 44.) / 24.).ceil().max(1.) as usize
    }
    pub fn max_first(&self) -> usize {
        self.len.saturating_sub(self.rows())
    }
    pub fn activate(&mut self, index: usize) {
        self.revision = self.revision.wrapping_add(1);
        self.editor = Some(ListEditor {
            revision: self.revision,
            index,
            text: None,
            cursor: 0,
            select_all: true,
            dirty: false,
        });
        if index < self.first {
            self.first = index;
        }
        if index >= self.first + self.rows() {
            self.first = index + 1 - self.rows();
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Monitor {
    pub slot: usize,
    pub label: String,
    pub mode: Mode,
    pub x: f64,
    pub y: f64,
    pub visible: bool,
    pub min: f64,
    pub max: f64,
    pub discrete: bool,
    pub text: String,
    pub number: f64,
    pub list: Option<ListMonitor>,
    order: usize,
}
impl Monitor {
    pub fn update_list(&mut self, values: &[u64], arena: &crate::value::Values) {
        let Some(list) = &mut self.list else {
            return;
        };
        list.len = values.len();
        list.first = list.first.min(list.max_first());
        list.items = values
            .iter()
            .skip(list.first)
            .take(list.rows())
            .map(|v| arena.monitor_text(*v, 1024))
            .collect();
        if let Some(editor) = &mut list.editor {
            if editor.index >= values.len() {
                list.editor = None;
            } else if editor.text.is_none() {
                let mut text = arena.monitor_text(values[editor.index], 16384);
                truncate_text(&mut text);
                editor.cursor = text.len();
                editor.text = Some(text);
            }
        }
    }
    pub fn slider_value(&self, value: f64) -> Option<f64> {
        if !self.visible || self.mode != Mode::Slider || !value.is_finite() {
            return None;
        }
        let step = if self.discrete { 1.0 } else { 0.01 };
        Some(
            (self.min + ((value.clamp(self.min, self.max) - self.min) / step).round() * step)
                .clamp(self.min, self.max),
        )
    }
    pub fn update(&mut self, value: ScratchValue) {
        self.number = value.to_number();
        self.text = match value {
            ScratchValue::Number(n) if n.is_finite() && n.abs() < 1e21 => {
                // Scratch GUI rounds numeric monitor values to six decimals,
                // while numeric strings retain their original spelling.
                let text = format!("{n:.6}");
                let text = text.trim_end_matches('0').trim_end_matches('.');
                if text == "-0" {
                    "0".into()
                } else {
                    text.into()
                }
            }
            _ => value.text(),
        };
    }
}
pub(crate) fn truncate_text(text: &mut String) {
    let mut end = text.len().min(16384);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}
pub(crate) fn create(project: &Project) -> Vec<Monitor> {
    let mut monitors = Vec::new();
    let mut slot = 0;
    for target in &project.targets {
        for (id, (name, value)) in &target.variables {
            let saved_index = project.monitors.iter().position(|m| {
                m.opcode == "data_variable"
                    && &m.id == id
                    && m.sprite_name
                        .as_deref()
                        .map_or(target.is_stage, |s| s == target.name)
            });
            let saved = saved_index.map(|i| &project.monitors[i]);
            let mut min = saved.map_or(0.0, |m| m.slider_min);
            let mut max = saved.map_or(100.0, |m| m.slider_max);
            if !min.is_finite() || !max.is_finite() {
                (min, max) = (0.0, 100.0);
            }
            if min > max {
                std::mem::swap(&mut min, &mut max);
            }
            let mut monitor = Monitor {
                slot,
                label: if target.is_stage {
                    name.clone()
                } else {
                    format!("{}: {name}", target.name)
                },
                mode: match saved.map(|m| m.mode.as_str()) {
                    Some("large") => Mode::Large,
                    Some("slider") => Mode::Slider,
                    _ => Mode::Normal,
                },
                x: saved.map_or(5.0, |m| m.x).clamp(0.0, 479.0),
                y: saved
                    .map_or(5.0 + (slot % 12) as f64 * 27.0, |m| m.y)
                    .clamp(0.0, 359.0),
                visible: saved.is_some_and(|m| m.visible),
                min,
                max,
                discrete: saved.is_none_or(|m| m.is_discrete),
                text: String::new(),
                number: 0.0,
                list: None,
                order: saved_index.unwrap_or(project.monitors.len() + slot),
            };
            if !monitor.x.is_finite() {
                monitor.x = 5.0;
            }
            if !monitor.y.is_finite() {
                monitor.y = 5.0;
            }
            monitor.update(value.clone());
            monitors.push(monitor);
            slot += 1;
        }
    }
    let variable_count = slot;
    slot = 0;
    for target in &project.targets {
        for (id, (name, items)) in &target.lists {
            let saved_index = project.monitors.iter().position(|m| {
                m.opcode == "data_listcontents"
                    && &m.id == id
                    && m.sprite_name
                        .as_deref()
                        .map_or(target.is_stage, |s| s == target.name)
            });
            let saved = saved_index.map(|i| &project.monitors[i]);
            let dimension = |value: f64, default: f64, min: f64, max: f64| {
                if value.is_finite() && value > 0. {
                    value.clamp(min, max)
                } else {
                    default
                }
            };
            let mut list = ListMonitor {
                revision: 0,
                width: dimension(saved.map_or(0., |m| m.width), 100., 100., 480.),
                height: dimension(saved.map_or(0., |m| m.height), 200., 60., 360.),
                len: items.len(),
                first: 0,
                items: Vec::new(),
                editor: None,
            };
            list.items = items
                .iter()
                .take(list.rows())
                .map(|v| v.text().chars().take(1024).collect())
                .collect();
            monitors.push(Monitor {
                slot,
                label: if target.is_stage {
                    name.clone()
                } else {
                    format!("{}: {name}", target.name)
                },
                mode: Mode::List,
                x: saved.map_or(5., |m| m.x).clamp(0., 479.),
                y: saved
                    .map_or(5. + (slot % 5) as f64 * 27., |m| m.y)
                    .clamp(0., 359.),
                visible: saved.is_some_and(|m| m.visible),
                min: 0.,
                max: 0.,
                discrete: true,
                text: String::new(),
                number: 0.,
                list: Some(list),
                order: saved_index.unwrap_or(project.monitors.len() + variable_count + slot),
            });
            slot += 1;
        }
    }
    // Preserve stacking order across both saved variable and list monitors.
    monitors.sort_by_key(|m| m.order);
    monitors
}

#[cfg(feature = "graphics")]
pub(crate) mod display;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        input::InputEvent,
        runtime::{self, RuntimeOptions},
    };
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    #[test]
    fn list_visibility_slots_and_live_contents_are_independent_of_variables() {
        let project: Arc<Project> = Arc::new(serde_json::from_value(serde_json::json!({"targets":[{
            "name":"Stage","isStage":true,"variables":{"same":["score",7]},"lists":{"same":["output",["saved"]]},"blocks":{
                "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"hide"},
                "hide":{"opcode":"data_hidelist","fields":{"LIST":["output","same"]},"next":"add"},
                "add":{"opcode":"data_addtolist","fields":{"LIST":["output","same"]},"inputs":{"ITEM":[1,[10,"live"]]},"next":"show"},
                "show":{"opcode":"data_showlist","fields":{"LIST":["output","same"]}}
            }
        },{"name":"Cat","lists":{"same":["local",["cat"]]},"blocks":{
            "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"show"},
            "show":{"opcode":"data_showlist","fields":{"LIST":["local","same"]}}
        }}],"monitors":[
            {"id":"same","opcode":"data_listcontents","mode":"list","visible":true,"width":480,"height":360,"x":0,"y":0},
            {"id":"same","opcode":"data_variable","visible":true},
            {"id":"same","opcode":"data_listcontents","mode":"list","spriteName":"Cat","visible":false,"width":0,"height":0}
        ]})).unwrap());
        let run = runtime::spawn(
            project,
            RuntimeOptions {
                turbo: true,
                ..Default::default()
            },
        )
        .unwrap();
        let scene = run.scene.clone();
        run.join().unwrap();
        let monitors = scene.monitors.lock().unwrap();
        assert_eq!(monitors[0].label, "output");
        assert_eq!(monitors[0].list.as_ref().unwrap().items, ["saved", "live"]);
        assert_eq!(monitors[0].list.as_ref().unwrap().width, 480.);
        assert!(
            monitors
                .iter()
                .any(|m| m.label == "score" && m.visible && m.text == "7" && m.list.is_none())
        );
        let local = monitors.iter().find(|m| m.label == "Cat: local").unwrap();
        assert!(local.visible);
        assert_eq!(local.list.as_ref().unwrap().items, ["cat"]);
        assert_eq!(local.list.as_ref().unwrap().width, 100.);
        assert_eq!(local.list.as_ref().unwrap().height, 200.);
    }

    #[test]
    fn list_snapshots_bound_large_values_to_the_visible_window() {
        let project: Project=serde_json::from_value(serde_json::json!({"targets":[{"name":"Stage","isStage":true,"lists":{"l":["big",[]]}}],"monitors":[{"id":"l","opcode":"data_listcontents","visible":true,"height":100}]})).unwrap();
        let mut m = create(&project).remove(0);
        let mut values = crate::value::Values::default();
        let long = values.encode(ScratchValue::String("x".repeat(100_000)));
        let items = vec![long; 200_000];
        m.list.as_mut().unwrap().first = 199_999;
        m.update_list(&items, &values);
        let list = m.list.as_ref().unwrap();
        assert_eq!(list.len, 200_000);
        assert_eq!(list.first, 199_997);
        assert_eq!(list.items.len(), 3);
        assert!(list.items.iter().all(|s| s.len() == 1024));
        m.update_list(&[], &values);
        assert_eq!(m.list.as_ref().unwrap().first, 0);
        assert!(m.list.as_ref().unwrap().items.is_empty());
    }

    pub(super) fn wait_for(
        scene: &crate::runtime::SharedScene,
        predicate: impl Fn(&[Monitor]) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if predicate(&scene.monitors.lock().unwrap()) {
                return;
            }
            assert!(Instant::now() < deadline, "monitor state did not update");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn show_hide_and_live_values_resolve_stage_and_sprite_ids() {
        let project: Arc<Project> = Arc::new(serde_json::from_value(serde_json::json!({"targets":[{
            "name":"Stage","isStage":true,"variables":{"same":["global",1],"new":["new monitor",2]},"blocks":{
                "hat":{"opcode":"event_whenflagclicked","topLevel":true,"next":"hide"},
                "hide":{"opcode":"data_hidevariable","fields":{"VARIABLE":["global","same"]},"next":"show"},
                "show":{"opcode":"data_showvariable","fields":{"VARIABLE":["new monitor","new"]},"next":"set"},
                "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["new monitor","new"]},"inputs":{"VALUE":[1,[10,"live string"]]}}
            }
        },{"name":"Cat","variables":{"same":["local",3]},"blocks":{
            "key":{"opcode":"event_whenkeypressed","topLevel":true,"fields":{"KEY_OPTION":["space"]},"next":"show"},
            "show":{"opcode":"data_showvariable","fields":{"VARIABLE":["local","same"]}}
        }}],"monitors":[
            {"id":"same","opcode":"data_variable","mode":"default","visible":true},
            {"id":"same","opcode":"data_variable","mode":"large","spriteName":"Cat","visible":false}
        ]})).unwrap());
        let run = runtime::spawn_interactive(
            project,
            RuntimeOptions {
                turbo: true,
                ..Default::default()
            },
        )
        .unwrap();
        run.scene.input.send(InputEvent::Flag);
        run.scene.input.send(InputEvent::Key {
            key: "space".into(),
            down: true,
        });
        wait_for(&run.scene, |m| {
            m.iter()
                .any(|m| m.label == "new monitor" && m.visible && m.text == "live string")
                && m.iter()
                    .any(|m| m.label == "Cat: local" && m.visible && m.mode == Mode::Large)
                && m.iter().any(|m| m.label == "global" && !m.visible)
        });
        run.stop();
        let report = run.join().unwrap();
        assert_eq!(
            report
                .variables
                .iter()
                .find(|v| v.name == "local")
                .unwrap()
                .value,
            ScratchValue::Number(3.0)
        );
    }
    #[test]
    fn monitor_formatting_and_slider_steps_match_scratch() {
        let project: Project = serde_json::from_value(serde_json::json!({"targets":[{
            "name":"Stage","isStage":true,"variables":{"v":["value",0]}
        }],"monitors":[{"id":"v","opcode":"data_variable","mode":"slider","visible":true,"sliderMin":-2,"sliderMax":2,"isDiscrete":false}]})).unwrap();
        let mut m = create(&project).remove(0);
        assert!((m.slider_value(1.237).unwrap() - 1.24).abs() < 1e-10);
        assert_eq!(m.slider_value(10.0), Some(2.0));
        assert_eq!(m.slider_value(f64::NAN), None);
        m.update(ScratchValue::Number(1.23456789));
        assert_eq!(m.text, "1.234568");
        m.update(ScratchValue::String("1.23456789".into()));
        assert_eq!(m.text, "1.23456789");
        m.visible = false;
        assert_eq!(m.slider_value(1.0), None);
    }
}
