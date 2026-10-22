//! Costume transforms used by persistent stamps and edge sensing.
use crate::{
    pen::{HEIGHT, PenCanvas, WIDTH},
    runtime::SpriteState,
};

pub(crate) struct CostumePixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub center: [f64; 2],
    pub resolution: f64,
    pub svg: bool,
    bounds: Option<[f64; 4]>,
}

impl CostumePixels {
    #[cfg(any(feature = "graphics", test))]
    pub fn new(width: u32, height: u32, rgba: Vec<u8>, center: [f64; 2], resolution: f64) -> Self {
        let (width, height) = (width as usize, height as usize);
        let mut bounds = [width as f64, height as f64, 0.0, 0.0];
        let mut opaque = false;
        for (i, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] == 0 {
                continue;
            }
            opaque = true;
            let (x, y) = ((i % width) as f64, (i / width) as f64);
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x + 1.0);
            bounds[3] = bounds[3].max(y + 1.0);
        }
        Self {
            width,
            height,
            rgba,
            center,
            resolution,
            svg: false,
            bounds: opaque.then_some(bounds),
        }
    }
    fn transform(&self, state: &SpriteState) -> Option<(f64, f64, f64, f64)> {
        let scale = state.size / 100.0 / self.resolution;
        if ![scale, state.x, state.y, state.direction]
            .iter()
            .all(|v| v.is_finite())
            || scale <= 0.0
        {
            return None;
        }
        let angle = if state.rotation_style == "all around" {
            (90.0 - state.direction).to_radians()
        } else {
            0.0
        };
        let (sin, cos) = angle.sin_cos();
        let flip = if state.rotation_style == "left-right" && state.direction < 0.0 {
            -1.0
        } else {
            1.0
        };
        Some((scale, sin, cos, flip))
    }
    pub fn world_bounds(&self, state: &SpriteState) -> Option<[f64; 4]> {
        if state.effects.changes_shape() {
            return self.effect_bounds(state);
        }
        let [left, top, right, bottom] = self.bounds?;
        let (scale, sin, cos, flip) = self.transform(state)?;
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for (px, py) in [(left, top), (left, bottom), (right, top), (right, bottom)] {
            let x = (px - self.center[0]) * scale * flip;
            let y = (self.center[1] - py) * scale;
            let (x, y) = (state.x + x * cos - y * sin, state.y + x * sin + y * cos);
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x);
            bounds[3] = bounds[3].max(y);
        }
        Some(bounds)
    }
    fn linear(&self, state: &SpriteState) -> bool {
        self.svg
            && (state.effects.changes_shape()
                || (!state.is_stage
                    && state.rotation_style == "all around"
                    && state.direction % 90.0 != 0.0)
                || (state.size - 100.0).abs() >= 0.001)
    }
    fn sample(&self, uv: [f64; 2], linear: bool) -> [f64; 4] {
        if !uv.into_iter().all(f64::is_finite) {
            return [0.0; 4];
        }
        let read = |x: f64, y: f64| {
            let x = x.clamp(0.0, (self.width - 1) as f64) as usize;
            let y = y.clamp(0.0, (self.height - 1) as f64) as usize;
            std::array::from_fn::<_, 4, _>(|c| {
                self.rgba[(y * self.width + x) * 4 + c] as f64 / 255.0
            })
        };
        if !linear {
            return read(
                (uv[0] * self.width as f64).floor(),
                (uv[1] * self.height as f64).floor(),
            );
        }
        let (x, y) = (
            uv[0] * self.width as f64 - 0.5,
            uv[1] * self.height as f64 - 0.5,
        );
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let (a, b, c, d) = (
            read(x.floor(), y.floor()),
            read(x.floor() + 1.0, y.floor()),
            read(x.floor(), y.floor() + 1.0),
            read(x.floor() + 1.0, y.floor() + 1.0),
        );
        std::array::from_fn(|i| {
            (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
        })
    }
    fn effect_bounds(&self, state: &SpriteState) -> Option<[f64; 4]> {
        let (scale, sin, cos, flip) = self.transform(state)?;
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        let size = [
            self.width as f64 / self.resolution,
            self.height as f64 / self.resolution,
        ];
        let opaque = |x: usize, y: usize| {
            let uv = state.effects.point(
                [
                    (x as f64 + 0.5) / self.width as f64,
                    (y as f64 + 0.5) / self.height as f64,
                ],
                size,
            );
            self.sample(uv, self.linear(state))[3] > 0.0
        };
        for y in 0..self.height {
            let Some(left) = (0..self.width).find(|&x| opaque(x, y)) else {
                continue;
            };
            let right = (left..self.width).rfind(|&x| opaque(x, y)).unwrap() + 1;
            for (px, py) in [(left, y), (left, y + 1), (right, y), (right, y + 1)] {
                let x = (px as f64 - self.center[0]) * scale * flip;
                let y = (self.center[1] - py as f64) * scale;
                let (x, y) = (state.x + x * cos - y * sin, state.y + x * sin + y * cos);
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x);
                bounds[3] = bounds[3].max(y);
            }
        }
        bounds[0].is_finite().then_some(bounds)
    }
    pub fn touching_edge(&self, state: &SpriteState) -> bool {
        self.world_bounds(state)
            .is_some_and(|[l, b, r, t]| l < -240.0 || b < -180.0 || r > 240.0 || t > 180.0)
    }
    pub fn stamp(&self, state: &SpriteState, canvas: &mut PenCanvas) {
        let Some([l, b, r, t]) = self.world_bounds(state) else {
            return;
        };
        let Some((scale, sin, cos, flip)) = self.transform(state) else {
            return;
        };
        let left = (l + 240.0).floor().clamp(0.0, WIDTH as f64) as usize;
        let right = (r + 240.0).ceil().clamp(0.0, WIDTH as f64) as usize;
        let top = (180.0 - t).floor().clamp(0.0, HEIGHT as f64) as usize;
        let bottom = (180.0 - b).ceil().clamp(0.0, HEIGHT as f64) as usize;
        if left >= right || top >= bottom {
            return;
        }
        canvas.pixels.resize(WIDTH * HEIGHT * 4, 0);
        for y in top..bottom {
            for x in left..right {
                let (dx, dy) = (
                    x as f64 + 0.5 - 240.0 - state.x,
                    180.0 - y as f64 - 0.5 - state.y,
                );
                let px = (dx * cos + dy * sin) / scale / flip + self.center[0];
                let py = self.center[1] - (-dx * sin + dy * cos) / scale;
                if px < 0.0 || py < 0.0 || px >= self.width as f64 || py >= self.height as f64 {
                    continue;
                }
                let uv = state.effects.point(
                    [px / self.width as f64, py / self.height as f64],
                    [
                        self.width as f64 / self.resolution,
                        self.height as f64 / self.resolution,
                    ],
                );
                let source = state.effects.color(self.sample(uv, self.linear(state)));
                let target = &mut canvas.pixels[(y * WIDTH + x) * 4..][..4];
                for channel in 0..4 {
                    target[channel] = (source[channel] * 255.0
                        + target[channel] as f64 * (1.0 - source[3]))
                        .round_ties_even()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
        canvas.generation = canvas.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn svg_sampling_respects_rotation_style_when_direction_is_not_axis_aligned() {
        let mut pixels = CostumePixels::new(1, 1, vec![255; 4], [0.0, 0.0], 1.0);
        pixels.svg = true;
        let mut sprite = state();
        sprite.direction = -45.0;
        assert!(pixels.linear(&sprite));
        sprite.rotation_style = "left-right".into();
        assert!(!pixels.linear(&sprite));
        sprite.effects.whirl = 10.0;
        assert!(pixels.linear(&sprite));
    }
    #[test]
    fn edge_sensing_uses_distorted_alpha_and_ignores_ghost() {
        let mut rgba = vec![0; 16];
        rgba[4..8].fill(255);
        let pixels = CostumePixels::new(4, 1, rgba, [2.0, 0.0], 1.0);
        let mut sprite = state();
        sprite.x = 240.0;
        assert!(!pixels.touching_edge(&sprite));
        sprite.effects.mosaic = 50.0;
        assert!(pixels.touching_edge(&sprite));
        sprite.effects.ghost = 100.0;
        assert!(pixels.touching_edge(&sprite));
    }
    #[cfg(feature = "graphics")]
    #[test]
    fn stamps_apply_all_effects_and_bitmap_resolution_like_scratch() -> anyhow::Result<()> {
        let (w, h, rgba) = crate::render::decode(
            "skin.png",
            include_bytes!("../tests/fixtures/effects/skin.png"),
        )?;
        let reference: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/effects/reference.json"))?;
        for case in reference["cases"].as_array().unwrap() {
            let mut pixels = CostumePixels::new(
                w,
                h,
                rgba.clone(),
                [32.0, 32.0],
                case["resolution"].as_f64().unwrap(),
            );
            pixels.svg = case["svg"].as_bool().unwrap_or(false);
            let mut sprite = state();
            sprite.effects =
                crate::effects::GraphicEffects::from_values(std::array::from_fn(|i| {
                    case["values"][i].as_f64().unwrap()
                }));
            sprite.size = 100.0 * pixels.resolution;
            let mut canvas = PenCanvas::default();
            pixels.stamp(&sprite, &mut canvas);
            let expected = image::open(format!(
                "{}/tests/fixtures/effects/{}.png",
                env!("CARGO_MANIFEST_DIR"),
                case["name"].as_str().unwrap()
            ))?
            .to_rgba8();
            let mut max = 0;
            for y in 0..64 {
                for x in 0..64 {
                    let i = ((y + 148) * 480 + x + 208) * 4;
                    let p = canvas.pixels.get(i..i + 4).unwrap_or(&[0; 4]);
                    for (c, want) in expected
                        .get_pixel(x as u32, y as u32)
                        .0
                        .into_iter()
                        .take(3)
                        .enumerate()
                    {
                        let got = p[c].saturating_add(255 - p[3]);
                        max = max.max(got.abs_diff(want));
                    }
                }
            }
            assert!(
                max <= 2,
                "{} stamp differs from Scratch: max={max}",
                case["name"]
            );
        }
        Ok(())
    }
    fn state() -> SpriteState {
        SpriteState {
            effects: Default::default(),
            name: "sprite".into(),
            is_stage: false,
            x: 0.0,
            y: 0.0,
            direction: 90.0,
            size: 100.0,
            visible: false,
            costume: 0,
            rotation_style: "all around".into(),
        }
    }
    #[test]
    fn stamps_preserve_premultiplied_alpha_and_survive_sprite_moves() {
        let pixels = CostumePixels::new(1, 1, vec![128, 0, 0, 128], [0.0, 0.0], 1.0);
        let mut canvas = PenCanvas::default();
        let mut sprite = state();
        pixels.stamp(&sprite, &mut canvas);
        assert_eq!(
            &canvas.pixels[(180 * WIDTH + 240) * 4..][..4],
            &[128, 0, 0, 128]
        );
        pixels.stamp(&sprite, &mut canvas);
        assert_eq!(
            &canvas.pixels[(180 * WIDTH + 240) * 4..][..4],
            &[192, 0, 0, 192]
        );
        sprite.x = 10.0;
        pixels.stamp(&sprite, &mut canvas);
        assert_eq!(
            &canvas.pixels[(180 * WIDTH + 240) * 4..][..4],
            &[192, 0, 0, 192]
        );
        assert_eq!(
            &canvas.pixels[(180 * WIDTH + 250) * 4..][..4],
            &[128, 0, 0, 128]
        );
    }
    #[test]
    fn edge_sensing_applies_scale_rotation_centers_and_mirroring() {
        let pixels = CostumePixels::new(2, 1, vec![255; 8], [0.0, 0.0], 1.0);
        let mut sprite = state();
        sprite.x = 239.0;
        assert!(pixels.touching_edge(&sprite));
        sprite.rotation_style = "left-right".into();
        sprite.direction = -90.0;
        assert!(!pixels.touching_edge(&sprite));
        sprite.rotation_style = "all around".into();
        sprite.direction = 0.0;
        sprite.x = 0.0;
        sprite.y = 179.0;
        assert!(pixels.touching_edge(&sprite));
    }
    #[cfg(feature = "graphics")]
    #[test]
    fn native_stamp_and_edge_blocks_use_costume_pixels() -> anyhow::Result<()> {
        use crate::{
            project::Project,
            runtime::{self, RuntimeOptions},
            value::ScratchValue,
        };
        let mut project: Project = serde_json::from_value(serde_json::json!({"targets":[{
            "name":"Stage","isStage":true,"variables":{"edge":["edge",0]},
            "costumes":[{"name":"red","md5ext":"red.svg","rotationCenterX":0,"rotationCenterY":0}],
            "blocks":{
                "flag":{"opcode":"event_whenflagclicked","topLevel":true,"next":"stamp"},
                "stamp":{"opcode":"pen_stamp","next":"move"},
                "move":{"opcode":"motion_gotoxy","inputs":{"X":[1,[4,"240"]],"Y":[1,[4,"0"]]},"next":"set"},
                "set":{"opcode":"data_setvariableto","fields":{"VARIABLE":["edge","edge"]},"inputs":{"VALUE":[2,"edge"]}},
                "edge":{"opcode":"sensing_touchingobject","inputs":{"TOUCHINGOBJECTMENU":[1,"menu"]}},
                "menu":{"opcode":"sensing_touchingobjectmenu","fields":{"TOUCHINGOBJECTMENU":["_edge_",null]}}
            }
        }]}))?;
        project.assets.insert("red.svg".into(),br##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><path fill="#f00" d="M0 0H1V1H0z"/></svg>"##.to_vec());
        let running = runtime::spawn(std::sync::Arc::new(project), RuntimeOptions::default())?;
        let scene = running.scene.clone();
        let report = running.join()?;
        assert_eq!(report.variables[0].value, ScratchValue::Boolean(true));
        assert_eq!(
            &scene.pen.lock().unwrap().pixels[(180 * WIDTH + 240) * 4..][..4],
            &[255, 0, 0, 255]
        );
        Ok(())
    }
}
