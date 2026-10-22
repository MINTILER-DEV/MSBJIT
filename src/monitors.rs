//! Variable monitor metadata and execution-thread bindings. UI changes never
//! write through the JIT's raw variable pointer.
use crate::{project::Project, value::ScratchValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Normal,
    Large,
    Slider,
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
}
impl Monitor {
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
pub(crate) fn create(project: &Project) -> Vec<Monitor> {
    let mut monitors = Vec::new();
    let mut order = Vec::new();
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
            order.push(saved_index.unwrap_or(project.monitors.len() + slot));
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
    // Preserve Scratch's stacking order for saved monitors.
    monitors.sort_by_key(|m| order[m.slot]);
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
