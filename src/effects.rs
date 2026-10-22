//! Scratch effect units, applied in mosaic/pixelate/whirl/fisheye then
//! color/brightness/ghost order. Values remain in block units so repeated
//! changes behave independently of the renderer's converted uniforms.
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct GraphicEffects {
    pub color: f64,
    pub fisheye: f64,
    pub whirl: f64,
    pub pixelate: f64,
    pub mosaic: f64,
    pub brightness: f64,
    pub ghost: f64,
}

impl GraphicEffects {
    pub(crate) fn values(self) -> [f64; 7] {
        [
            self.color,
            self.fisheye,
            self.whirl,
            self.pixelate,
            self.mosaic,
            self.brightness,
            self.ghost,
        ]
    }
    pub(crate) fn from_values(v: [f64; 7]) -> Self {
        Self {
            color: v[0],
            fisheye: v[1],
            whirl: v[2],
            pixelate: v[3],
            mosaic: v[4],
            brightness: v[5],
            ghost: v[6],
        }
    }
    pub(crate) fn update(&mut self, name: &str, amount: f64, change: bool) {
        let field = match name.to_ascii_lowercase().as_str() {
            "color" => &mut self.color,
            "fisheye" => &mut self.fisheye,
            "whirl" => &mut self.whirl,
            "pixelate" => &mut self.pixelate,
            "mosaic" => &mut self.mosaic,
            "brightness" => &mut self.brightness,
            "ghost" => &mut self.ghost,
            _ => return,
        };
        let n = if change { *field + amount } else { amount };
        *field = match name.to_ascii_lowercase().as_str() {
            "ghost" => n.clamp(0.0, 100.0),
            "brightness" => n.clamp(-100.0, 100.0),
            _ => n,
        };
    }
    pub(crate) fn mask(self) -> u32 {
        self.values()
            .into_iter()
            .enumerate()
            .fold(0, |mask, (i, n)| {
                mask | (u32::from(n != 0.0 && !n.is_nan()) << i)
            })
    }
    #[cfg(feature = "graphics")]
    pub(crate) fn uniforms(self) -> ([f32; 4], [f32; 4]) {
        (
            [
                ((self.color / 200.0) % 1.0) as f32,
                ((self.fisheye + 100.0) / 100.0).max(0.0) as f32,
                (-self.whirl.to_radians()) as f32,
                (self.pixelate.abs() / 10.0) as f32,
            ],
            [
                ((self.mosaic.abs() + 10.0) / 10.0)
                    .round()
                    .clamp(1.0, 512.0) as f32,
                (self.brightness.clamp(-100.0, 100.0) / 100.0) as f32,
                (1.0 - self.ghost.clamp(0.0, 100.0) / 100.0) as f32,
                0.0,
            ],
        )
    }
    pub(crate) fn changes_shape(self) -> bool {
        self.mask() & 30 != 0
    }
    /// Texture space uses the first uploaded row at v=0 on both GPU and CPU.
    pub(crate) fn point(self, mut uv: [f64; 2], size: [f64; 2]) -> [f64; 2] {
        let mask = self.mask();
        if mask & 16 != 0 {
            let tiles = ((self.mosaic.abs() + 10.0) / 10.0)
                .round()
                .clamp(1.0, 512.0);
            for x in &mut uv {
                *x = (*x * tiles).rem_euclid(1.0);
            }
        }
        if mask & 8 != 0 {
            for i in 0..2 {
                let cells = size[i] / (self.pixelate.abs() / 10.0);
                uv[i] = ((uv[i] * cells).floor() + 0.5) / cells;
            }
        }
        if mask & 4 != 0 {
            let (x, y) = (uv[0] - 0.5, uv[1] - 0.5);
            let angle = -self.whirl.to_radians() * (1.0 - 2.0 * x.hypot(y)).max(0.0).powi(2);
            let (sin, cos) = angle.sin_cos();
            uv = [0.5 + x * cos + y * sin, 0.5 - x * sin + y * cos];
        }
        if mask & 2 != 0 {
            let (x, y) = ((uv[0] - 0.5) * 2.0, (uv[1] - 0.5) * 2.0);
            let length = x.hypot(y);
            if length > 0.0 {
                let radius = length
                    .min(1.0)
                    .powf(((self.fisheye + 100.0) / 100.0).max(0.0))
                    * length.max(1.0);
                uv = [
                    0.5 + 0.5 * radius * x / length,
                    0.5 + 0.5 * radius * y / length,
                ];
            }
        }
        uv
    }
    /// Premultiplied color math corresponding to the display shader. Rounding
    /// happens only when writing the destination pixel, as with a GPU target.
    pub(crate) fn color(self, rgba: [f64; 4]) -> [f64; 4] {
        let mut rgb = [rgba[0], rgba[1], rgba[2]];
        let alpha = rgba[3];
        let mask = self.mask();
        if mask & 33 != 0 {
            let divisor = alpha + 0.001;
            rgb = rgb.map(|n| (n / divisor).clamp(0.0, 1.0));
            if mask & 1 != 0 {
                let [r, g, b] = rgb;
                let hi = r.max(g).max(b);
                let delta = hi - r.min(g).min(b);
                let denominator = 6.0 * delta + 0.001;
                let mut h = if r > g.max(b) {
                    if b > g {
                        1.0 - (b - g) / denominator
                    } else {
                        (g - b) / denominator
                    }
                } else if b > g {
                    2.0 / 3.0 + (r - g) / denominator
                } else {
                    1.0 / 3.0 + (b - r) / denominator
                };
                let (mut s, mut v) = (delta / (hi + 0.001), hi);
                if v < 0.055 {
                    h = 0.0;
                    s = 1.0;
                    v = 0.055;
                } else if s < 0.09 {
                    h = 0.0;
                    s = 0.09;
                }
                h = (h + self.color / 200.0 % 1.0).rem_euclid(1.0);
                let c = v * s;
                let x = c * (1.0 - ((h * 6.0).rem_euclid(2.0) - 1.0).abs());
                rgb = match (h * 6.0).floor() as u32 % 6 {
                    0 => [c, x, 0.0],
                    1 => [x, c, 0.0],
                    2 => [0.0, c, x],
                    3 => [0.0, x, c],
                    4 => [x, 0.0, c],
                    _ => [c, 0.0, x],
                }
                .map(|n| n + v - c);
            }
            let brightness = if mask & 32 != 0 {
                self.brightness.clamp(-100.0, 100.0) / 100.0
            } else {
                0.0
            };
            rgb = rgb.map(|n| (n + brightness).clamp(0.0, 1.0) * divisor);
        }
        let ghost = if mask & 64 != 0 {
            1.0 - self.ghost.clamp(0.0, 100.0) / 100.0
        } else {
            1.0
        };
        [
            rgb[0] * ghost,
            rgb[1] * ghost,
            rgb[2] * ghost,
            alpha * ghost,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinate_effects_match_scratch_renderer_reference() {
        let reference: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/effects/reference.json")).unwrap();
        for c in reference["cases"].as_array().unwrap() {
            let values = std::array::from_fn(|i| c["values"][i].as_f64().unwrap());
            let effects = GraphicEffects::from_values(values);
            assert_eq!(effects.mask(), c["mask"].as_u64().unwrap() as u32);
            let size = 64.0 / c["resolution"].as_f64().unwrap();
            for point in c["points"].as_array().unwrap() {
                if point["expected"][0].is_null() {
                    continue;
                } // Scratch's undefined center 0/0.
                let result = effects.point(
                    [
                        point["uv"][0].as_f64().unwrap(),
                        point["uv"][1].as_f64().unwrap(),
                    ],
                    [size; 2],
                );
                for (i, n) in result.into_iter().enumerate() {
                    assert!(
                        (n - point["expected"][i].as_f64().unwrap()).abs() < 2e-5,
                        "{} point {point}: {result:?}",
                        c["name"]
                    );
                }
            }
        }
        // Color=200 is enabled even though its hue offset wraps back to zero.
        let effects = GraphicEffects {
            color: 200.0,
            ..Default::default()
        };
        assert_eq!(effects.mask(), 1);
        assert!(effects.color([0.0, 0.0, 0.0, 1.0])[0] > 0.05);
    }
}
