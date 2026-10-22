//! Scratch-sized monitor overlay, shared by the desktop and streamed stage.
use super::{Mode, Monitor};
use crate::{input::InputEvent, runtime::SharedScene};
use fontdue::{Font, FontSettings};
use resvg::tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

fn font(bold: bool) -> &'static Font {
    static REGULAR: OnceLock<Font> = OnceLock::new();
    static BOLD: OnceLock<Font> = OnceLock::new();
    let (cell, bytes): (_, &[u8]) = if bold {
        (&BOLD, include_bytes!("../../assets/fonts/Arimo-Bold.ttf"))
    } else {
        (
            &REGULAR,
            include_bytes!("../../assets/fonts/Arimo-Regular.ttf"),
        )
    };
    cell.get_or_init(|| {
        Font::from_bytes(bytes, FontSettings::default()).expect("bundled Arimo font")
    })
}
fn width(text: &str, size: f32, bold: bool) -> f64 {
    let f = font(bold);
    text.lines()
        .map(|line| {
            line.chars()
                .take(1024)
                .map(|c| f.metrics(c, size).advance_width as f64)
                .sum::<f64>()
        })
        .fold(0.0, f64::max)
}
#[derive(Clone, Copy)]
struct Layout {
    width: f64,
    height: f64,
    label: f64,
    value: f64,
    row: f64,
}
fn layout(m: &Monitor) -> Layout {
    if m.mode == Mode::Large {
        return Layout {
            width: (width(&m.text, 16.0, false) + 8.0).max(56.0) + 2.0,
            height: (m.text.lines().count().max(1) as f64 * 18.0).max(22.4) + 5.2,
            label: 0.0,
            value: 0.0,
            row: 0.0,
        };
    }
    let label = width(&m.label, 12.0, true);
    let value = width(&m.text, 12.0, false).max(40.0) + 4.0;
    let row = m.text.lines().count().clamp(1, 24) as f64 * 14.0;
    Layout {
        width: label + value + 28.0,
        height: row + 8.0 + if m.mode == Mode::Slider { 20.0 } else { 0.0 },
        label,
        value,
        row,
    }
}
fn rounded(p: &mut Pixmap, x: f64, y: f64, w: f64, h: f64, r: f64, color: [u8; 3]) {
    let (x, y, w, h, r) = (x as f32, y as f32, w as f32, h as f32, r as f32);
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.quad_to(x + w, y, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.quad_to(x + w, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.quad_to(x, y + h, x, y + h - r);
    path.line_to(x, y + r);
    path.quad_to(x, y, x + r, y);
    path.close();
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], 255);
    p.fill_path(
        &path.finish().unwrap(),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}
fn text(p: &mut Pixmap, text: &str, cx: f64, top: f64, size: f32, bold: bool, color: [u8; 3]) {
    let f = font(bold);
    for (line_number, line) in text.lines().take(24).enumerate() {
        let mut x = cx - width(line, size, bold) / 2.0;
        let baseline = top + size as f64 + line_number as f64 * (size as f64 + 2.0);
        for c in line.chars().take(1024) {
            let (metrics, bitmap) = f.rasterize(c, size);
            let left = x.round() as i32 + metrics.xmin;
            let top = baseline.round() as i32 - metrics.ymin - metrics.height as i32;
            for gy in 0..metrics.height {
                for gx in 0..metrics.width {
                    let (px, py) = (left + gx as i32, top + gy as i32);
                    if !(0..480).contains(&px) || !(0..360).contains(&py) {
                        continue;
                    }
                    let alpha = bitmap[gy * metrics.width + gx] as u32;
                    let i = (py as usize * 480 + px as usize) * 4;
                    let dst = &mut p.data_mut()[i..i + 4];
                    for channel in 0..3 {
                        dst[channel] = ((color[channel] as u32 * alpha
                            + dst[channel] as u32 * (255 - alpha)
                            + 127)
                            / 255) as u8;
                    }
                    dst[3] = (alpha + (dst[3] as u32 * (255 - alpha) + 127) / 255) as u8;
                }
            }
            x += metrics.advance_width as f64;
        }
    }
}
#[derive(Default)]
pub(crate) struct Overlay {
    previous: Vec<Monitor>,
    pixels: Vec<u8>,
}
impl Overlay {
    pub fn composite(&mut self, scene: &SharedScene, rgba: &mut [u8]) {
        let monitors: Vec<_> = scene
            .monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|m| m.visible)
            .cloned()
            .collect();
        if monitors.is_empty() {
            self.previous.clear();
            self.pixels.clear();
            return;
        }
        if monitors != self.previous {
            let mut p = Pixmap::new(480, 360).unwrap();
            for m in &monitors {
                let l = layout(m);
                let (x, y) = (m.x, m.y);
                // 15% black border over Scratch's #E5F0FF monitor panel.
                rounded(&mut p, x, y, l.width, l.height, 4.0, [195, 204, 217]);
                if m.mode == Mode::Large {
                    rounded(
                        &mut p,
                        x + 1.0,
                        y + 1.0,
                        l.width - 2.0,
                        l.height - 2.0,
                        3.0,
                        [255, 140, 26],
                    );
                    text(
                        &mut p,
                        &m.text,
                        x + l.width / 2.0,
                        y + 2.0,
                        16.0,
                        false,
                        [255; 3],
                    );
                } else {
                    rounded(
                        &mut p,
                        x + 1.0,
                        y + 1.0,
                        l.width - 2.0,
                        l.height - 2.0,
                        3.0,
                        [229, 240, 255],
                    );
                    text(
                        &mut p,
                        &m.label,
                        x + 9.0 + l.label / 2.0,
                        y + 3.0,
                        12.0,
                        true,
                        [87, 94, 117],
                    );
                    let vx = x + 19.0 + l.label;
                    rounded(&mut p, vx, y + 4.0, l.value, l.row, 4.0, [255, 140, 26]);
                    text(
                        &mut p,
                        &m.text,
                        vx + l.value / 2.0,
                        y + 3.0,
                        12.0,
                        false,
                        [255; 3],
                    );
                    if m.mode == Mode::Slider {
                        let start = x + 11.0;
                        let end = x + l.width - 11.0;
                        let ratio = if m.max > m.min {
                            ((m.number - m.min) / (m.max - m.min)).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        let knob = start + ratio * (end - start);
                        let cy = y + l.row + 17.0;
                        rounded(
                            &mut p,
                            start - 5.0,
                            cy - 3.0,
                            end - start + 10.0,
                            6.0,
                            3.0,
                            [178; 3],
                        );
                        rounded(
                            &mut p,
                            start - 4.0,
                            cy - 2.0,
                            end - start + 8.0,
                            4.0,
                            2.0,
                            [239; 3],
                        );
                        rounded(&mut p, knob - 6.0, cy - 6.0, 12.0, 12.0, 6.0, [150; 3]);
                        rounded(&mut p, knob - 5.0, cy - 5.0, 10.0, 10.0, 5.0, [255; 3]);
                    }
                }
            }
            self.pixels = p.take();
            self.previous = monitors;
        }
        for (dst, src) in rgba.chunks_exact_mut(4).zip(self.pixels.chunks_exact(4)) {
            let inv = 255 - src[3] as u32;
            for c in 0..3 {
                dst[c] = (src[c] as u32 + (dst[c] as u32 * inv + 127) / 255).min(255) as u8;
            }
        }
    }
}

enum Drag {
    Slider { slot: usize, start: f64, end: f64 },
    Panel { slot: usize, dx: f64, dy: f64 },
}
#[derive(Default)]
pub(crate) struct Pointer {
    down: bool,
    drag: Option<Drag>,
    click: Option<(usize, Instant, f64, f64)>,
}
impl Pointer {
    pub fn release(&mut self) {
        self.down = false;
        self.drag = None;
    }
    /// Scratch coordinates in, monitor consumes the pointer before sprite picking.
    pub fn update(&mut self, scene: &SharedScene, x: f64, y: f64, down: bool) -> bool {
        if !x.is_finite() || !y.is_finite() {
            self.release();
            return false;
        }
        let (x, y) = (x + 240.0, 180.0 - y);
        let pressed = down && !self.down;
        self.down = down;
        let mut monitors = scene.monitors.lock().unwrap_or_else(|e| e.into_inner());
        let hit = monitors.iter().rposition(|m| {
            let l = layout(m);
            m.visible && x >= m.x && x <= m.x + l.width && y >= m.y && y <= m.y + l.height
        });
        if pressed && let Some(i) = hit {
            let m = &mut monitors[i];
            let l = layout(m);
            if m.mode == Mode::Slider && y >= m.y + l.row + 8.0 {
                self.drag = Some(Drag::Slider {
                    slot: m.slot,
                    start: m.x + 11.0,
                    end: m.x + l.width - 11.0,
                });
            } else {
                if self.click.is_some_and(|(slot, time, px, py)| {
                    slot == m.slot
                        && time.elapsed() < Duration::from_millis(400)
                        && (x - px).abs() < 4.0
                        && (y - py).abs() < 4.0
                }) {
                    m.mode = match m.mode {
                        Mode::Normal => Mode::Large,
                        Mode::Large => Mode::Slider,
                        Mode::Slider => Mode::Normal,
                    };
                    self.click = None;
                } else {
                    self.click = Some((m.slot, Instant::now(), x, y));
                }
                self.drag = Some(Drag::Panel {
                    slot: m.slot,
                    dx: x - m.x,
                    dy: y - m.y,
                });
            }
        }
        let consumed = hit.is_some() || self.drag.is_some();
        if down {
            match self.drag {
                Some(Drag::Slider { slot, start, end }) => {
                    if let Some(m) = monitors
                        .iter()
                        .find(|m| m.slot == slot && m.visible && m.mode == Mode::Slider)
                    {
                        let value =
                            m.min + ((x - start) / (end - start)).clamp(0.0, 1.0) * (m.max - m.min);
                        scene.input.send(InputEvent::MonitorSlider { slot, value });
                    }
                }
                Some(Drag::Panel { slot, dx, dy }) => {
                    if let Some(m) = monitors.iter_mut().find(|m| m.slot == slot && m.visible) {
                        let l = layout(m);
                        m.x = (x - dx).clamp(0.0, (480.0 - l.width).max(0.0));
                        m.y = (y - dy).clamp(0.0, (360.0 - l.height).max(0.0));
                    }
                }
                None => {}
            }
        } else {
            self.drag = None;
        }
        consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::tests::wait_for;
    use crate::{
        project::Project,
        runtime::{self, RuntimeOptions},
        value::ScratchValue,
    };
    use std::sync::Arc;

    fn project() -> Arc<Project> {
        Arc::new(serde_json::from_value(serde_json::json!({"targets":[{
            "name":"Stage","isStage":true,"variables":{"a":["score",123],"b":["large",42],"c":["speed",15],"d":["decimal",0]}
        }],"monitors":[
            {"id":"a","opcode":"data_variable","mode":"default","x":10,"y":10,"visible":true},
            {"id":"b","opcode":"data_variable","mode":"large","x":10,"y":50,"visible":true},
            {"id":"c","opcode":"data_variable","mode":"slider","x":10,"y":100,"visible":true,"sliderMin":10,"sliderMax":20},
            {"id":"d","opcode":"data_variable","mode":"slider","x":210,"y":100,"visible":true,"sliderMin":-1,"sliderMax":1,"isDiscrete":false}
        ]})).unwrap())
    }
    #[test]
    fn overlay_matches_scratch_geometry_and_clears_hidden_monitors() {
        let run = runtime::spawn_interactive(project(), RuntimeOptions::default()).unwrap();
        let mut overlay = Overlay::default();
        let mut rgba = vec![255; 480 * 360 * 4];
        overlay.composite(&run.scene, &mut rgba);
        let pixel = |x: usize, y: usize| &rgba[(y * 480 + x) * 4..][..4];
        assert_eq!(pixel(15, 15), &[229, 240, 255, 255]);
        assert_eq!(pixel(15, 55), &[255, 140, 26, 255]);
        assert_eq!(pixel(200, 200), &[255; 4]);
        assert!(rgba.chunks_exact(4).any(|p| p[..3] == [87, 94, 117]));
        if let Ok(path) = std::env::var("MSBJIT_MONITOR_CAPTURE") {
            image::save_buffer(path, &rgba, 480, 360, image::ColorType::Rgba8).unwrap();
        }
        for m in run.scene.monitors.lock().unwrap().iter_mut() {
            m.visible = false;
        }
        rgba.fill(255);
        overlay.composite(&run.scene, &mut rgba);
        assert!(rgba.iter().all(|b| *b == 255));
        run.stop();
        run.join().unwrap();
    }
    #[test]
    fn pointer_drags_sliders_updates_jit_variables_and_captures_outside_panel() {
        let run = runtime::spawn_interactive(
            project(),
            RuntimeOptions {
                turbo: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut pointer = Pointer::default();
        let slider = run.scene.monitors.lock().unwrap()[2].clone();
        let l = layout(&slider);
        let (x, y) = (
            slider.x + l.width - 11.0 - 240.0,
            180.0 - (slider.y + l.row + 17.0),
        );
        assert!(pointer.update(&run.scene, x, y, true));
        wait_for(&run.scene, |m| m[2].number == 20.0);
        assert!(pointer.update(&run.scene, -400.0, y, true));
        wait_for(&run.scene, |m| m[2].number == 10.0);
        pointer.update(&run.scene, -400.0, y, false);
        assert!(!pointer.update(&run.scene, 200.0, -100.0, false));
        // Continuous sliders use Scratch's 0.01 step, and can update an idle VM.
        let slider = run.scene.monitors.lock().unwrap()[3].clone();
        let l = layout(&slider);
        let x = slider.x + 11.0 + 0.617 * (l.width - 22.0) - 240.0;
        assert!(pointer.update(&run.scene, x, y, true));
        wait_for(&run.scene, |m| (m[3].number - 0.23).abs() < 1e-10);
        pointer.release();
        // Double-click a header to change normal -> large; drag it to move it.
        pointer.update(&run.scene, -220.0, 165.0, true);
        pointer.update(&run.scene, -220.0, 165.0, false);
        pointer.update(&run.scene, -220.0, 165.0, true);
        assert_eq!(run.scene.monitors.lock().unwrap()[0].mode, Mode::Large);
        pointer.update(&run.scene, -180.0, 125.0, true);
        assert_eq!(run.scene.monitors.lock().unwrap()[0].x, 50.0);
        run.stop();
        let report = run.join().unwrap();
        assert_eq!(
            report.variables.iter().find(|v| v.id == "c").unwrap().value,
            ScratchValue::Number(10.0)
        );
    }
}
