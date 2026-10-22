//! Bounded input shared by local windows, browser sessions, and embedders.
use serde::Deserialize;
use std::{
    collections::{HashSet, VecDeque},
    sync::Mutex,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    Answer {
        id: u64,
        text: String,
    },
    Key {
        key: String,
        down: bool,
    },
    Mouse {
        x: f64,
        y: f64,
        down: bool,
    },
    Click {
        target: usize,
    },
    // Host-selected slider binding, never accepted directly from a browser.
    #[serde(skip)]
    MonitorSlider {
        slot: usize,
        value: f64,
    },
    #[serde(skip)]
    ListMutation {
        slot: usize,
        index: usize,
        op: u8,
        text: String,
    },
    MonitorText {
        slot: usize,
        revision: u64,
        text: String,
        #[serde(default)]
        replace: bool,
    },
    MonitorKey {
        slot: usize,
        revision: u64,
        key: String,
        #[serde(default)]
        shift: bool,
    },
    Wheel {
        x: f64,
        y: f64,
        delta: f64,
    },
    Flag,
    Stop,
    Release,
}

#[derive(Clone, Default)]
pub(crate) struct InputState {
    pub keys: HashSet<String>,
    pub x: f64,
    pub y: f64,
    pub down: bool,
}
impl InputState {
    pub fn key_down(&self, key: &str) -> bool {
        if key.eq_ignore_ascii_case("any") {
            !self.keys.is_empty()
        } else {
            normalize_key(key).is_some_and(|key| self.keys.contains(&key))
        }
    }
}

pub fn normalize_key(key: &str) -> Option<String> {
    let key = key.to_ascii_lowercase();
    Some(match key.as_str() {
        " " | "space" | "spacebar" => "space".into(),
        "arrowup" | "up arrow" => "up arrow".into(),
        "arrowdown" | "down arrow" => "down arrow".into(),
        "arrowleft" | "left arrow" => "left arrow".into(),
        "arrowright" | "right arrow" => "right arrow".into(),
        "enter" => "enter".into(),
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => key,
        _ => return None,
    })
}

#[derive(Default)]
struct Pending {
    state: InputState,
    events: VecDeque<InputEvent>,
}
#[derive(Default)]
pub struct InputController(Mutex<Pending>);
impl InputController {
    /// Submit input without blocking on JIT execution. Excess events are dropped;
    /// held-state updates and release/control messages still take effect.
    pub fn send(&self, event: InputEvent) {
        let mut pending = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let event = match event {
            InputEvent::ListMutation {
                slot,
                index,
                op,
                mut text,
            } => {
                crate::monitors::truncate_text(&mut text);
                InputEvent::ListMutation {
                    slot,
                    index,
                    op,
                    text,
                }
            }
            InputEvent::Answer { id, mut text } => {
                if text.len() > 16384 {
                    let mut end = 16384;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                }
                InputEvent::Answer { id, text }
            }
            InputEvent::Key { key, down } => {
                let Some(key) = normalize_key(&key) else {
                    return;
                };
                if down {
                    if !pending.state.keys.insert(key.clone()) {
                        return;
                    }
                } else {
                    pending.state.keys.remove(&key);
                    return;
                }
                InputEvent::Key { key, down }
            }
            InputEvent::Mouse { x, y, down } => {
                if !x.is_finite() || !y.is_finite() {
                    return;
                }
                pending.state.x = x.clamp(-240.0, 240.0);
                pending.state.y = y.clamp(-180.0, 180.0);
                pending.state.down = down;
                return;
            }
            InputEvent::Release => {
                pending.state.keys.clear();
                pending.state.down = false;
                return;
            }
            InputEvent::Flag | InputEvent::Stop => {
                pending.events.clear();
                event
            }
            InputEvent::MonitorSlider { slot, value } => {
                if !value.is_finite() {
                    return;
                }
                pending.events.retain(
                    |e| !matches!(e, InputEvent::MonitorSlider { slot: old, .. } if *old == slot),
                );
                event
            }
            _ => event,
        };
        if pending.events.len() < 256 {
            pending.events.push_back(event);
        }
    }
    pub(crate) fn drain(&self) -> (InputState, Vec<InputEvent>) {
        let mut pending = self.0.lock().unwrap_or_else(|e| e.into_inner());
        (pending.state.clone(), pending.events.drain(..).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_invalid_input_and_releases_are_preserved() {
        let input = InputController::default();
        for target in 0..1024 {
            input.send(InputEvent::Click { target });
        }
        input.send(InputEvent::Key {
            key: "a".into(),
            down: true,
        });
        input.send(InputEvent::Mouse {
            x: 1000.0,
            y: -1000.0,
            down: true,
        });
        input.send(InputEvent::Mouse {
            x: f64::NAN,
            y: 0.0,
            down: false,
        });
        let (state, events) = input.drain();
        assert_eq!(events.len(), 256);
        assert!(state.key_down("a"));
        assert!(state.down);
        assert_eq!((state.x, state.y), (240.0, -180.0));
        input.send(InputEvent::Release);
        let (state, _) = input.drain();
        assert!(!state.key_down("any") && !state.down);
    }
}
