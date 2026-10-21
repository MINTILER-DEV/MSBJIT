//! Persistent premultiplied RGBA pen layer, independent of the graphics backend.
use crate::value::ScratchValue;
pub(crate) const WIDTH: usize = 480;
pub(crate) const HEIGHT: usize = 360;

#[derive(Clone)]
pub(crate) struct PenState {
    pub down: bool,
    pub size: f64,
    hue: f64,
    saturation: f64,
    brightness: f64,
    transparency: f64,
}
impl Default for PenState {
    fn default() -> Self {
        Self {
            down: false,
            size: 1.0,
            hue: 66.66,
            saturation: 100.0,
            brightness: 100.0,
            transparency: 0.0,
        }
    }
}
impl PenState {
    pub fn parameter(&mut self, name: &str, value: f64, change: bool) {
        let field = match name {
            "color" => &mut self.hue,
            "saturation" => &mut self.saturation,
            "brightness" => &mut self.brightness,
            "transparency" => &mut self.transparency,
            _ => return,
        };
        let value = if change { *field + value } else { value };
        *field = if name == "color" {
            value.rem_euclid(100.0)
        } else {
            value.clamp(0.0, 100.0)
        };
    }
    pub fn set_color(&mut self, value: &ScratchValue) {
        let rgb = match value {
            ScratchValue::String(s) if s.starts_with('#') => {
                u32::from_str_radix(&s[1..], 16).unwrap_or(0)
            }
            _ => value.to_number() as i64 as u32,
        };
        let (r, g, b) = (
            ((rgb >> 16) & 255) as f64 / 255.0,
            ((rgb >> 8) & 255) as f64 / 255.0,
            (rgb & 255) as f64 / 255.0,
        );
        let high = r.max(g).max(b);
        let low = r.min(g).min(b);
        let delta = high - low;
        self.brightness = high * 100.0;
        self.saturation = if high == 0.0 {
            0.0
        } else {
            delta / high * 100.0
        };
        self.hue = if delta == 0.0 {
            0.0
        } else if high == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if high == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        } * (100.0 / 6.0);
        self.transparency = 0.0;
    }
    fn rgba(&self) -> [f64; 4] {
        let h = self.hue / 100.0 * 6.0;
        let v = self.brightness / 100.0;
        let c = v * self.saturation / 100.0;
        let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
        let [r, g, b] = match h as u32 % 6 {
            0 => [c, x, 0.0],
            1 => [x, c, 0.0],
            2 => [0.0, c, x],
            3 => [0.0, x, c],
            4 => [x, 0.0, c],
            _ => [c, 0.0, x],
        };
        let m = v - c;
        [r + m, g + m, b + m, 1.0 - self.transparency / 100.0]
    }
}

#[derive(Default)]
pub(crate) struct PenCanvas {
    pub pixels: Vec<u8>,
    pub generation: u64,
}
impl PenCanvas {
    pub fn clear(&mut self) {
        self.pixels.clear();
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn line(&mut self, pen: &PenState, from: (f64, f64), to: (f64, f64)) {
        if ![from.0, from.1, to.0, to.1, pen.size]
            .iter()
            .all(|v| v.is_finite())
        {
            return;
        }
        let (ax, ay, bx, by) = (240.0 + from.0, 180.0 - from.1, 240.0 + to.0, 180.0 - to.1);
        let radius = pen.size.clamp(1.0, 1200.0) / 2.0;
        let left = (ax.min(bx) - radius - 1.0).floor().clamp(0.0, WIDTH as f64) as usize;
        let right = (ax.max(bx) + radius + 1.0).ceil().clamp(0.0, WIDTH as f64) as usize;
        let top = (ay.min(by) - radius - 1.0)
            .floor()
            .clamp(0.0, HEIGHT as f64) as usize;
        let bottom = (ay.max(by) + radius + 1.0).ceil().clamp(0.0, HEIGHT as f64) as usize;
        let (dx, dy) = (bx - ax, by - ay);
        let length = dx * dx + dy * dy;
        if left >= right || top >= bottom || !length.is_finite() {
            return;
        }
        self.pixels.resize(WIDTH * HEIGHT * 4, 0);
        let color = pen.rgba();
        for y in top..bottom {
            for x in left..right {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let t = if length == 0.0 {
                    0.0
                } else {
                    (((px - ax) * dx + (py - ay) * dy) / length).clamp(0.0, 1.0)
                };
                let distance = (px - ax - t * dx).hypot(py - ay - t * dy);
                let alpha = (radius + 0.5 - distance).clamp(0.0, 1.0) * color[3];
                if alpha <= 0.0 {
                    continue;
                }
                let pixel = &mut self.pixels[(y * WIDTH + x) * 4..][..4];
                for channel in 0..3 {
                    pixel[channel] = (color[channel] * alpha * 255.0
                        + pixel[channel] as f64 * (1.0 - alpha))
                        .round() as u8;
                }
                pixel[3] = (alpha * 255.0 + pixel[3] as f64 * (1.0 - alpha)).round() as u8;
            }
        }
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pen_draws_persistent_clipped_color_and_clear() {
        let mut canvas = PenCanvas::default();
        let mut pen = PenState {
            size: 4.0,
            ..Default::default()
        };
        pen.set_color(&ScratchValue::String("#ff0000".into()));
        canvas.line(&pen, (-10000.0, 0.0), (10000.0, 0.0));
        let pixel = (180 * WIDTH + 240) * 4;
        assert_eq!(&canvas.pixels[pixel..pixel + 4], &[255, 0, 0, 255]);
        pen.parameter("transparency", 50.0, false);
        pen.parameter("color", 100.0 / 3.0, false);
        canvas.line(&pen, (0.0, 0.0), (0.0, 0.0));
        assert!(canvas.pixels[pixel] >= 127 && canvas.pixels[pixel + 1] >= 127);
        let generation = canvas.generation;
        canvas.clear();
        assert!(canvas.pixels.is_empty() && canvas.generation > generation);
    }
}
