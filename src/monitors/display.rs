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
    if let Some(list) = &m.list {
        return Layout {
            width: list.width,
            height: list.height,
            label: 0.,
            value: 0.,
            row: 24.,
        };
    }
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
    if w <= 0. || h <= 0. {
        return;
    }
    let r = r.min(w.min(h) / 2.).max(0.);
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
    text_in(p, text, [cx, top], size, bold, color, [0., 0., 480., 360.]);
}
fn text_in(
    p: &mut Pixmap,
    text: &str,
    [cx, top]: [f64; 2],
    size: f32,
    bold: bool,
    color: [u8; 3],
    clip: [f64; 4],
) {
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
                    if !(0..480).contains(&px)
                        || !(0..360).contains(&py)
                        || (px as f64) < clip[0]
                        || (py as f64) < clip[1]
                        || (px as f64) >= clip[2]
                        || (py as f64) >= clip[3]
                    {
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
fn list_monitor(p: &mut Pixmap, m: &Monitor) {
    let list = m.list.as_ref().unwrap();
    let (x, y, w, h) = (m.x, m.y, list.width, list.height);
    rounded(p, x, y, w, h, 4., [195, 204, 217]);
    rounded(p, x + 1., y + 1., w - 2., h - 2., 3., [229, 240, 255]);
    let body_bottom = y + h - 22.;
    let gutter = width(&list.len.max(1).to_string(), 12., true).max(7.) + 10.;
    let left = x + gutter + 2.;
    let right = x + w - 14.;
    for (row, item) in list.items.iter().enumerate() {
        let index = list.first + row;
        let ry = y + 22. + row as f64 * 24.;
        if ry >= body_bottom {
            break;
        }
        let clip = [left + 5., ry + 1., right - 3., body_bottom.min(ry + 24.)];
        text_in(
            p,
            &(index + 1).to_string(),
            [x + gutter / 2., ry + 3.],
            12.,
            true,
            [87, 94, 117],
            [x + 1., y + 22., left, body_bottom],
        );
        let active = list.editor.as_ref().filter(|e| e.index == index);
        let height = (body_bottom - ry - 2.).clamp(0., 22.);
        rounded(
            p,
            left,
            ry + 1.,
            (right - left).max(1.),
            height,
            3.,
            [217, 87, 22],
        );
        rounded(
            p,
            left + 1.,
            ry + 2.,
            (right - left - 2.).max(1.),
            (height - 2.).max(0.),
            2.,
            [255, 102, 26],
        );
        let label = active.and_then(|e| e.text.as_deref()).unwrap_or(item);
        let line = label.lines().next().unwrap_or("");
        if active.is_some_and(|e| e.select_all) {
            rounded(
                p,
                left + 4.,
                ry + 3.,
                width(line, 12., false).min((right - left - 18.).max(0.)),
                17.,
                1.,
                [60, 115, 190],
            );
        }
        let end = if active.is_some() {
            right - 16.
        } else {
            right - 3.
        };
        text_in(
            p,
            line,
            [left + 5. + width(line, 12., false) / 2., ry + 3.],
            12.,
            false,
            [255; 3],
            [clip[0], clip[1], end, clip[3]],
        );
        if let Some(editor) = active {
            if !editor.select_all
                && let Some(value) = &editor.text
            {
                let cursor = editor.cursor.min(value.len());
                let prefix = value.get(..cursor).unwrap_or("");
                let caret = (left + 5. + width(prefix, 12., false)).min(end - 1.);
                rounded(p, caret, ry + 4., 1., 15., 0., [255; 3]);
            }
            text_in(
                p,
                "×",
                [right - 8., ry + 2.],
                14.,
                true,
                [255; 3],
                [left, ry, right, clip[3]],
            );
        }
    }
    if list.len == 0 {
        text(p, "(empty)", x + w / 2., y + 28., 12., false, [87, 94, 117]);
    }
    if list.max_first() > 0 {
        let track = h - 46.;
        rounded(p, x + w - 11., y + 23., 8., track, 4., [218, 228, 240]);
        let thumb = (track * list.rows() as f64 / list.len as f64)
            .max(20.)
            .min(track);
        let top = y + 23. + (track - thumb) * list.first as f64 / list.max_first() as f64;
        rounded(p, x + w - 10., top, 6., thumb, 3., [150, 163, 182]);
    }
    rounded(p, x + 1., y + 1., w - 2., 20., 3., [255; 3]);
    text_in(
        p,
        &m.label,
        [x + w / 2., y + 3.],
        12.,
        true,
        [87, 94, 117],
        [x + 4., y + 1., x + w - 4., y + 21.],
    );
    rounded(p, x + 1., body_bottom, w - 2., 21., 3., [255; 3]);
    text(p, "+", x + 10., body_bottom + 1., 16., true, [87, 94, 117]);
    text(
        p,
        &format!("length {}", list.len),
        x + w / 2.,
        body_bottom + 3.,
        12.,
        true,
        [87, 94, 117],
    );
    text(
        p,
        "=",
        x + w - 10.,
        body_bottom + 1.,
        16.,
        true,
        [87, 94, 117],
    );
}
#[derive(Default)]
pub(crate) struct Overlay {
    previous: Vec<Monitor>,
    pixels: Vec<u8>,
}

pub(crate) fn question_overlay(scene: &SharedScene, rgba: &mut [u8]) {
    let question = scene
        .question
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let Some(question) = question else {
        return;
    };
    let mut p = Pixmap::new(480, 360).unwrap();
    rounded(&mut p, 8.0, 292.0, 464.0, 60.0, 8.0, [133, 92, 214]);
    rounded(&mut p, 10.0, 294.0, 460.0, 56.0, 6.0, [255; 3]);
    let label: String = question.text.chars().take(64).collect();
    text(&mut p, &label, 240.0, 296.0, 12.0, true, [70; 3]);
    rounded(&mut p, 18.0, 318.0, 418.0, 25.0, 12.0, [220, 211, 245]);
    rounded(&mut p, 20.0, 320.0, 414.0, 21.0, 10.0, [255; 3]);
    let mut chars: Vec<_> = question.draft.chars().rev().take(45).collect();
    chars.reverse();
    let draft: String = chars.into_iter().collect();
    text(
        &mut p,
        &format!("{}|", draft),
        227.0,
        320.0,
        13.0,
        false,
        [60; 3],
    );
    rounded(&mut p, 440.0, 319.0, 24.0, 24.0, 12.0, [133, 92, 214]);
    text(&mut p, ">", 452.0, 320.0, 16.0, true, [255; 3]);
    for (dst, src) in rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(p.data().as_chunks::<4>().0)
    {
        let inv = 255 - src[3] as u32;
        for c in 0..3 {
            dst[c] = (src[c] as u32 + (dst[c] as u32 * inv + 127) / 255).min(255) as u8;
        }
    }
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
                if m.list.is_some() {
                    list_monitor(&mut p, m);
                    continue;
                }
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
        for (dst, src) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(self.pixels.as_chunks::<4>().0.iter())
        {
            let inv = 255 - src[3] as u32;
            for c in 0..3 {
                dst[c] = (src[c] as u32 + (dst[c] as u32 * inv + 127) / 255).min(255) as u8;
            }
        }
    }
}

enum Drag {
    Slider {
        slot: usize,
        start: f64,
        end: f64,
    },
    Panel {
        slot: usize,
        list: bool,
        dx: f64,
        dy: f64,
    },
    Resize {
        slot: usize,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    Scroll {
        slot: usize,
        top: f64,
        travel: f64,
        offset: f64,
    },
}
fn commit_editor(scene: &SharedScene, m: &mut Monitor) {
    if let Some(editor) = m.list.as_mut().and_then(|l| l.editor.take())
        && editor.dirty
        && let Some(text) = editor.text
    {
        scene.input.send(InputEvent::ListMutation {
            slot: m.slot,
            index: editor.index,
            op: 0,
            text,
        });
    }
}
#[derive(Default)]
pub(crate) struct Pointer {
    down: bool,
    drag: Option<Drag>,
    click: Option<(usize, Instant, f64, f64)>,
}
impl Pointer {
    #[cfg(feature = "streaming")]
    pub fn editor_target(&self, scene: &SharedScene) -> Option<(usize, u64)> {
        scene
            .monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|m| m.visible)
            .find_map(|m| Some((m.slot, m.list.as_ref()?.editor.as_ref()?.revision)))
    }
    pub fn editing(&self, scene: &SharedScene) -> bool {
        scene
            .monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|m| m.visible && m.list.as_ref().is_some_and(|l| l.editor.is_some()))
    }
    pub fn finish_editing(&mut self, scene: &SharedScene) {
        for m in scene
            .monitors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
        {
            commit_editor(scene, m);
        }
    }
    pub fn text(&mut self, scene: &SharedScene, text: &str, replace: bool) -> bool {
        let mut monitors = scene.monitors.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = monitors
            .iter_mut()
            .filter(|m| m.visible)
            .find_map(|m| m.list.as_mut().and_then(|l| l.editor.as_mut()))
        else {
            return false;
        };
        let value = e.text.get_or_insert_default();
        if replace || e.select_all {
            value.clear();
            e.cursor = 0;
        }
        let input: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(16384)
            .collect();
        value.insert_str(e.cursor.min(value.len()), &input);
        super::truncate_text(value);
        e.cursor = (e.cursor + input.len()).min(value.len());
        while !value.is_char_boundary(e.cursor) {
            e.cursor -= 1;
        }
        e.select_all = false;
        e.dirty = true;
        true
    }
    pub fn key(&mut self, scene: &SharedScene, key: &str, shift: bool) -> bool {
        let mut monitors = scene.monitors.lock().unwrap_or_else(|e| e.into_inner());
        let Some(m) = monitors
            .iter_mut()
            .find(|m| m.visible && m.list.as_ref().is_some_and(|l| l.editor.is_some()))
        else {
            return false;
        };
        let list = m.list.as_mut().unwrap();
        let mut editor = list.editor.take().unwrap();
        if key == "escape" {
            return true;
        }
        if key == "commit" {
            if editor.dirty
                && let Some(text) = editor.text
            {
                scene.input.send(InputEvent::ListMutation {
                    slot: m.slot,
                    index: editor.index,
                    op: 0,
                    text,
                });
            }
            return true;
        }
        if matches!(key, "enter" | "tab" | "up" | "down") {
            if editor.dirty
                && let Some(text) = editor.text
            {
                scene.input.send(InputEvent::ListMutation {
                    slot: m.slot,
                    index: editor.index,
                    op: 0,
                    text,
                });
            }
            if key == "enter" {
                scene.input.send(InputEvent::ListMutation {
                    slot: m.slot,
                    index: editor.index + usize::from(!shift),
                    op: 1,
                    text: String::new(),
                });
            } else if list.len > 0 {
                let backwards = key == "up" || key == "tab" && shift;
                list.activate(if backwards {
                    (editor.index + list.len - 1) % list.len
                } else {
                    (editor.index + 1) % list.len
                });
            }
            return true;
        }
        if editor.text.is_none() {
            if editor.select_all && matches!(key, "backspace" | "delete") {
                editor.text = Some(String::new());
                editor.cursor = 0;
                editor.select_all = false;
                editor.dirty = true;
            }
            list.editor = Some(editor);
            return true;
        }
        let value = editor.text.as_mut().unwrap();
        editor.cursor = editor.cursor.min(value.len());
        match key {
            "select all" => editor.select_all = true,
            "backspace" | "delete" => {
                if editor.select_all {
                    value.clear();
                    editor.cursor = 0;
                } else if key == "backspace" && editor.cursor > 0 {
                    let previous = value[..editor.cursor]
                        .char_indices()
                        .next_back()
                        .map_or(0, |(i, _)| i);
                    value.drain(previous..editor.cursor);
                    editor.cursor = previous;
                } else if key == "delete" && editor.cursor < value.len() {
                    let next =
                        editor.cursor + value[editor.cursor..].chars().next().unwrap().len_utf8();
                    value.drain(editor.cursor..next);
                }
                editor.select_all = false;
                editor.dirty = true;
            }
            "home" => {
                editor.cursor = 0;
                editor.select_all = false;
            }
            "end" => {
                editor.cursor = value.len();
                editor.select_all = false;
            }
            "left" => {
                editor.cursor = if editor.select_all {
                    0
                } else {
                    value[..editor.cursor]
                        .char_indices()
                        .next_back()
                        .map_or(0, |(i, _)| i)
                };
                editor.select_all = false;
            }
            "right" => {
                editor.cursor = if editor.select_all {
                    value.len()
                } else {
                    editor.cursor
                        + value[editor.cursor..]
                            .chars()
                            .next()
                            .map_or(0, char::len_utf8)
                };
                editor.select_all = false;
            }
            _ => {}
        }
        list.editor = Some(editor);
        true
    }
    pub fn wheel(&mut self, scene: &SharedScene, x: f64, y: f64, delta: f64) -> bool {
        if ![x, y, delta].into_iter().all(f64::is_finite) {
            return false;
        }
        let (x, y) = (x + 240., 180. - y);
        let mut monitors = scene.monitors.lock().unwrap_or_else(|e| e.into_inner());
        let Some(m) = monitors.iter_mut().rev().find(|m| {
            let l = layout(m);
            m.visible && x >= m.x && x <= m.x + l.width && y >= m.y && y <= m.y + l.height
        }) else {
            return false;
        };
        if m.list.is_some() {
            commit_editor(scene, m);
            let list = m.list.as_mut().unwrap();
            let change = (delta * 3.).round().clamp(-10000., 10000.) as i64;
            list.first = (list.first as i64 + change).clamp(0, list.max_first() as i64) as usize;
        }
        true
    }
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
        let remove = hit.and_then(|i| {
            let m = &monitors[i];
            let list = m.list.as_ref()?;
            let index = list.first + ((y - m.y - 22.) / 24.).floor().max(0.) as usize;
            (y >= m.y + 22.
                && y < m.y + list.height - 22.
                && x >= m.x + list.width - 30.
                && x < m.x + list.width - 14.
                && list.editor.as_ref().is_some_and(|e| e.index == index))
            .then_some(index)
        });
        if pressed {
            for m in monitors.iter_mut() {
                commit_editor(scene, m);
            }
        }
        if pressed && let Some(i) = hit {
            let m = &mut monitors[i];
            let l = layout(m);
            if let Some(list) = &mut m.list {
                if y >= m.y + list.height - 22. && x >= m.x + list.width - 22. {
                    self.drag = Some(Drag::Resize {
                        slot: m.slot,
                        x,
                        y,
                        width: list.width,
                        height: list.height,
                    });
                } else if y >= m.y + list.height - 22. && x <= m.x + 22. {
                    scene.input.send(InputEvent::ListMutation {
                        slot: m.slot,
                        index: list.len,
                        op: 1,
                        text: String::new(),
                    });
                } else if y < m.y + 22. {
                    self.drag = Some(Drag::Panel {
                        slot: m.slot,
                        list: true,
                        dx: x - m.x,
                        dy: y - m.y,
                    });
                } else if y < m.y + list.height - 22.
                    && x >= m.x + list.width - 14.
                    && list.max_first() > 0
                {
                    let track = list.height - 46.;
                    let thumb = (track * list.rows() as f64 / list.len as f64)
                        .max(20.)
                        .min(track);
                    let top = m.y + 23.;
                    let thumb_y =
                        top + (track - thumb) * list.first as f64 / list.max_first() as f64;
                    self.drag = Some(Drag::Scroll {
                        slot: m.slot,
                        top,
                        travel: track - thumb,
                        offset: if y >= thumb_y && y <= thumb_y + thumb {
                            y - thumb_y
                        } else {
                            thumb / 2.
                        },
                    });
                } else if y < m.y + list.height - 22. {
                    let index = list.first + ((y - m.y - 22.) / 24.).floor() as usize;
                    if let Some(index) = remove {
                        scene.input.send(InputEvent::ListMutation {
                            slot: m.slot,
                            index,
                            op: 2,
                            text: String::new(),
                        });
                    } else if index < list.len {
                        list.activate(index);
                    }
                }
                self.click = None;
            } else if m.mode == Mode::Slider && y >= m.y + l.row + 8.0 {
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
                        Mode::List => Mode::List,
                    };
                    self.click = None;
                } else {
                    self.click = Some((m.slot, Instant::now(), x, y));
                }
                self.drag = Some(Drag::Panel {
                    slot: m.slot,
                    list: false,
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
                Some(Drag::Panel { slot, list, dx, dy }) => {
                    if let Some(m) = monitors
                        .iter_mut()
                        .find(|m| m.slot == slot && m.visible && m.list.is_some() == list)
                    {
                        let l = layout(m);
                        m.x = (x - dx).clamp(0.0, (480.0 - l.width).max(0.0));
                        m.y = (y - dy).clamp(0.0, (360.0 - l.height).max(0.0));
                    }
                }
                Some(Drag::Resize {
                    slot,
                    x: px,
                    y: py,
                    width,
                    height,
                }) => {
                    if let Some(m) = monitors
                        .iter_mut()
                        .find(|m| m.slot == slot && m.visible && m.list.is_some())
                    {
                        let list = m.list.as_mut().unwrap();
                        list.width = (width + x - px).clamp(100., 480.);
                        list.height = (height + y - py).clamp(60., 360.);
                        list.first = list.first.min(list.max_first());
                    }
                }
                Some(Drag::Scroll {
                    slot,
                    top,
                    travel,
                    offset,
                }) => {
                    if let Some(list) = monitors
                        .iter_mut()
                        .find(|m| m.slot == slot && m.visible && m.list.is_some())
                        .and_then(|m| m.list.as_mut())
                    {
                        list.first = (((y - top - offset) / travel.max(1.)).clamp(0., 1.)
                            * list.max_first() as f64)
                            .round() as usize;
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

    #[test]
    fn list_panel_scroll_resize_edit_and_native_list_views_stay_in_sync() {
        let project: Arc<Project> = Arc::new(serde_json::from_value(serde_json::json!({"targets":[{
            "name":"Stage","isStage":true,"variables":{"v":["score",42]},
            "lists":{"l":["output",(0..100).map(|i|format!("row {i}")).collect::<Vec<_>>()]} ,
            "blocks":{
                "key":{"opcode":"event_whenkeypressed","topLevel":true,"fields":{"KEY_OPTION":["space"]},"next":"set"},
                "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["score","v"]},"inputs":{"VALUE":[2,"item"]}},
                "item":{"opcode":"data_itemoflist","fields":{"LIST":["output","l"]},"inputs":{"INDEX":[1,[4,"1"]]}}
            }
        }],"monitors":[
            {"id":"v","opcode":"data_variable","visible":true,"x":260,"y":10},
            {"id":"l","opcode":"data_listcontents","visible":true,"x":20,"y":20,"width":180,"height":140}
        ]})).unwrap());
        let run = runtime::spawn_interactive(
            project,
            RuntimeOptions {
                turbo: true,
                ..Default::default()
            },
        )
        .unwrap();
        let scene = &run.scene;
        let mut pointer = Pointer::default();
        assert!(pointer.wheel(scene, -170., 110., 5.));
        wait_for(scene, |m| {
            m.iter().any(|m| {
                m.list.as_ref().is_some_and(|l| {
                    l.first == 15 && l.items.first().is_some_and(|s| s == "row 15")
                })
            })
        });
        assert!(pointer.update(scene, -45., 28., true)); // bottom right resize handle
        pointer.update(scene, -5., -12., true);
        pointer.update(scene, -5., -12., false);
        wait_for(scene, |m| {
            m.iter().any(|m| {
                m.list
                    .as_ref()
                    .is_some_and(|l| l.width == 220. && l.height == 180.)
            })
        });
        pointer.wheel(scene, -170., 110., -100.);
        wait_for(scene, |m| {
            m.iter().any(|m| {
                m.list
                    .as_ref()
                    .is_some_and(|l| l.first == 0 && l.items[0] == "row 0")
            })
        });
        assert!(pointer.update(scene, -160., 130., true)); // first item
        pointer.update(scene, -160., 130., false);
        assert!(pointer.editing(scene));
        pointer.text(scene, "edited é", false);
        pointer.key(scene, "backspace", false);
        pointer.text(scene, "猫", false);
        pointer.key(scene, "commit", false);
        wait_for(scene, |m| {
            m.iter().any(|m| {
                m.list
                    .as_ref()
                    .is_some_and(|l| l.items.first().is_some_and(|s| s == "edited 猫"))
            })
        });
        // Append and remove through the same host-only mutation path; the JIT
        // subsequently reads the refreshed list view after buffer reallocation.
        scene.input.send(InputEvent::ListMutation {
            slot: 0,
            index: 100,
            op: 1,
            text: String::new(),
        });
        wait_for(scene, |m| {
            m.iter()
                .any(|m| m.list.as_ref().is_some_and(|l| l.len == 101))
        });
        scene.input.send(InputEvent::ListMutation {
            slot: 0,
            index: 100,
            op: 2,
            text: String::new(),
        });
        wait_for(scene, |m| {
            m.iter()
                .any(|m| m.list.as_ref().is_some_and(|l| l.len == 100))
        });
        scene.input.send(InputEvent::Key {
            key: "space".into(),
            down: true,
        });
        wait_for(scene, |m| {
            m.iter().any(|m| m.list.is_none() && m.text == "edited 猫")
        });
        let mut overlay = Overlay::default();
        let mut frame = vec![255; 480 * 360 * 4];
        overlay.composite(scene, &mut frame);
        if let Ok(path) = std::env::var("MSBJIT_LIST_CAPTURE") {
            image::save_buffer(path, &frame, 480, 360, image::ColorType::Rgba8).unwrap();
        }
        assert_eq!(&frame[(23 * 480 + 23) * 4..][..3], &[255; 3]); // header
        assert_eq!(&frame[(48 * 480 + 180) * 4..][..3], &[255, 102, 26]); // orange row
        assert_eq!(&frame[(50 * 480 + 245) * 4..][..3], &[255; 3]); // clipped outside panel
        run.stop();
        let report = run.join().unwrap();
        assert_eq!(
            report.variables[0].value,
            ScratchValue::String("edited 猫".into())
        );
        assert_eq!(report.lists[0].items.len(), 100);
    }

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
    fn question_overlay_renders_text_and_clears_after_submission() {
        let running = runtime::spawn(project(), RuntimeOptions::default()).unwrap();
        let scene = running.scene.clone();
        running.join().unwrap();
        *scene.question.lock().unwrap() = Some(crate::runtime::QuestionPrompt {
            id: 1,
            text: "What is your name?".into(),
            draft: "Player1".into(),
        });
        let mut rgba = vec![255; 480 * 360 * 4];
        question_overlay(&scene, &mut rgba);
        assert_eq!(&rgba[(300 * 480 + 9) * 4..][..3], &[133, 92, 214]);
        assert!(
            rgba[(296 * 480) * 4..(344 * 480) * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[..3].iter().all(|c| *c < 120))
                .count()
                > 100
        );
        if let Ok(path) = std::env::var("MSBJIT_QUESTION_CAPTURE") {
            image::save_buffer(path, &rgba, 480, 360, image::ColorType::Rgba8).unwrap();
        }
        *scene.question.lock().unwrap() = None;
        let mut clear = vec![255; 480 * 360 * 4];
        question_overlay(&scene, &mut clear);
        assert!(clear.iter().all(|v| *v == 255));
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
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] == [87, 94, 117])
        );
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
