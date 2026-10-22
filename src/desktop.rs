//! Native stage window; no media encoder or browser is required.
use crate::{input::InputEvent, project::Project, render::Renderer, runtime::SharedScene};
use anyhow::{Context, Result};
use minifb::{Key, KeyRepeat, MouseButton, MouseMode, Scale, ScaleMode, Window, WindowOptions};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

const WIDTH: usize = 480;
const HEIGHT: usize = 392;
const TOOLBAR: usize = 32;

struct TextInput(Arc<Mutex<Vec<char>>>);
impl minifb::InputCallback for TextInput {
    fn add_char(&mut self, code: u32) {
        if let Some(c) = char::from_u32(code).filter(|c| !c.is_control()) {
            let mut input = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if input.len() < 4096 {
                input.push(c);
            }
        }
    }
}

pub fn show(
    project: &Project,
    scene: Arc<SharedScene>,
    fps: u32,
    duration: Option<Duration>,
) -> Result<()> {
    let mut window = Window::new(
        "MSBJIT | Green flag: run again | Red square: stop",
        WIDTH,
        HEIGHT,
        WindowOptions {
            resize: true,
            scale: Scale::X2,
            scale_mode: ScaleMode::AspectRatioStretch,
            ..Default::default()
        },
    )
    .context("open stage window")?;
    window.set_target_fps(fps as usize);
    let text_input = Arc::new(Mutex::new(Vec::new()));
    window.set_input_callback(Box::new(TextInput(text_input.clone())));
    let mut question_id = None;
    let mut draft = String::new();
    let mut renderer = pollster::block_on(Renderer::new(project))?;
    scene.input.send(InputEvent::Flag);
    let mut buffer = vec![0x1f2937; WIDTH * HEIGHT];
    toolbar(&mut buffer);
    let mut held = HashSet::new();
    let mut mouse_down = false;
    let mut monitor_pointer = crate::monitors::display::Pointer::default();
    let mut last_mouse = (0.0, 0.0);
    let start = Instant::now();
    while window.is_open()
        && !scene.stopped.load(Ordering::Relaxed)
        && duration.is_none_or(|d| start.elapsed() < d)
    {
        let states = scene.snapshot(project);
        let rgba = renderer.capture_scene(&scene, &states)?;
        for (pixel, rgba) in buffer[WIDTH * TOOLBAR..]
            .iter_mut()
            .zip(rgba.as_chunks::<4>().0.iter())
        {
            *pixel = (rgba[0] as u32) << 16 | (rgba[1] as u32) << 8 | rgba[2] as u32;
        }
        window.update_with_buffer(&buffer, WIDTH, HEIGHT)?;
        if !window.is_active() {
            scene.input.send(InputEvent::Release);
            held.clear();
            mouse_down = false;
            monitor_pointer.release();
            continue;
        }
        let question = scene
            .question
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let answering = question.is_some();
        let characters = std::mem::take(&mut *text_input.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(question) = &question {
            if question_id != Some(question.id) {
                question_id = Some(question.id);
                draft.clear();
            } else {
                for c in characters {
                    if draft.len() + c.len_utf8() <= 16384 {
                        draft.push(c);
                    }
                }
            }
            if window.is_key_pressed(Key::Backspace, KeyRepeat::Yes) {
                draft.pop();
            }
            if window.is_key_pressed(Key::Enter, KeyRepeat::No) {
                scene.input.send(InputEvent::Answer {
                    id: question.id,
                    text: draft.clone(),
                });
            }
            if let Some(prompt) = scene
                .question
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                && prompt.id == question.id
            {
                prompt.draft = draft.clone();
            }
        } else {
            question_id = None;
        }
        let keys: HashSet<String> = if answering {
            HashSet::new()
        } else {
            window.get_keys().into_iter().filter_map(key_name).collect()
        };
        for key in keys.difference(&held) {
            scene.input.send(InputEvent::Key {
                key: key.clone(),
                down: true,
            });
        }
        for key in held.difference(&keys) {
            scene.input.send(InputEvent::Key {
                key: key.clone(),
                down: false,
            });
        }
        held = keys;
        let down = window.get_mouse_down(MouseButton::Left);
        let pressed = down && !mouse_down;
        let point = window
            .get_unscaled_mouse_pos(MouseMode::Pass)
            .and_then(|(x, y)| buffer_point(window.get_size(), x as f64, y as f64));
        if let Some((x, y)) = point {
            if y < TOOLBAR as f64 {
                monitor_pointer.release();
                scene.input.send(InputEvent::Mouse {
                    x: x - 240.0,
                    y: 180.0,
                    down: false,
                });
                if pressed && (8.0..40.0).contains(&x) {
                    scene.input.send(InputEvent::Flag);
                }
                if pressed && (48.0..80.0).contains(&x) {
                    scene.input.send(InputEvent::Stop);
                }
            } else {
                let (x, y) = (x - 240.0, 180.0 - (y - TOOLBAR as f64));
                last_mouse = (x, y);
                let consumed = monitor_pointer.update(&scene, x, y, down);
                if pressed
                    && answering
                    && (200.0..224.0).contains(&x)
                    && (-164.0..-139.0).contains(&y)
                    && let Some(question) = &question
                {
                    scene.input.send(InputEvent::Answer {
                        id: question.id,
                        text: draft.clone(),
                    });
                }
                scene.input.send(InputEvent::Mouse {
                    x,
                    y,
                    down: down && !consumed && !answering,
                });
                if pressed
                    && !consumed
                    && !answering
                    && let Some(target) = renderer.hit_test(&states, x, y)
                {
                    scene.input.send(InputEvent::Click { target });
                }
            }
        } else {
            monitor_pointer.release();
            // Clear a held button when the pointer leaves the stage/window.
            scene.input.send(InputEvent::Mouse {
                x: last_mouse.0,
                y: last_mouse.1,
                down: false,
            });
        }
        mouse_down = down;
        if scene.finished.load(Ordering::Acquire) {
            break;
        }
    }
    scene.input.send(InputEvent::Release);
    scene.stopped.store(true, Ordering::Relaxed);
    Ok(())
}

fn buffer_point((width, height): (usize, usize), x: f64, y: f64) -> Option<(f64, f64)> {
    let scale = (width as f64 / WIDTH as f64).min(height as f64 / HEIGHT as f64);
    if scale <= 0.0 {
        return None;
    }
    let x = (x - (width as f64 - WIDTH as f64 * scale) / 2.0) / scale;
    let y = (y - (height as f64 - HEIGHT as f64 * scale) / 2.0) / scale;
    ((0.0..WIDTH as f64).contains(&x) && (0.0..HEIGHT as f64).contains(&y)).then_some((x, y))
}

fn toolbar(buffer: &mut [u32]) {
    for y in 6..26 {
        for x in 16..19 {
            buffer[y * WIDTH + x] = 0xe5e7eb;
        }
    }
    for y in 7_usize..21 {
        for x in 19..(34 - y.abs_diff(14)) {
            buffer[y * WIDTH + x] = 0x22c55e;
        }
    }
    for y in 8..24 {
        for x in 56..72 {
            buffer[y * WIDTH + x] = 0xef4444;
        }
    }
}

fn key_name(key: Key) -> Option<String> {
    Some(match key {
        Key::Space => "space".into(),
        Key::Up => "up arrow".into(),
        Key::Down => "down arrow".into(),
        Key::Left => "left arrow".into(),
        Key::Right => "right arrow".into(),
        Key::Enter => "enter".into(),
        _ => {
            let name = format!("{key:?}").to_ascii_lowercase();
            let name = name.strip_prefix("key").unwrap_or(&name);
            if name.len() != 1 || !name.as_bytes()[0].is_ascii_alphanumeric() {
                return None;
            }
            name.into()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pointer_mapping_accounts_for_scaling_toolbar_and_letterboxing() {
        assert_eq!(buffer_point((960, 784), 480.0, 424.0), Some((240.0, 212.0)));
        assert_eq!(
            buffer_point((1200, 784), 600.0, 424.0),
            Some((240.0, 212.0))
        );
        assert_eq!(buffer_point((1200, 784), 30.0, 100.0), None);
        assert_eq!(buffer_point((0, 0), 0.0, 0.0), None);
    }
}
