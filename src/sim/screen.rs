//! Software rasteriser for the `screen.*` drawing API.

use super::font::{self, ADVANCE, GLYPH_H, LINE_H};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Color {
    pub fn rgba(r: f64, g: f64, b: f64, a: f64) -> Self {
        let c = |v: f64| v.clamp(0.0, 255.0);
        Color {
            r: c(r),
            g: c(g),
            b: c(b),
            a: c(a),
        }
    }
}

#[derive(Clone, Copy)]
pub struct MapColors {
    pub ocean: Color,
    pub shallows: Color,
    pub land: Color,
    pub grass: Color,
    pub sand: Color,
    pub snow: Color,
    pub rock: Color,
    pub gravel: Color,
}

impl Default for MapColors {
    fn default() -> Self {
        let c = |r, g, b| Color::rgba(r, g, b, 255.0);
        MapColors {
            ocean: c(5.0, 15.0, 30.0),
            shallows: c(20.0, 40.0, 60.0),
            land: c(75.0, 75.0, 70.0),
            grass: c(60.0, 85.0, 50.0),
            sand: c(130.0, 125.0, 95.0),
            snow: c(200.0, 200.0, 200.0),
            rock: c(90.0, 85.0, 80.0),
            gravel: c(100.0, 95.0, 85.0),
        }
    }
}

pub struct Screen {
    pub w: usize,
    pub h: usize,
    /// RGBA pixels, row major.
    pub pixels: Vec<u8>,
    pub color: Color,
    pub map_colors: MapColors,
}

impl Screen {
    pub fn new(w: usize, h: usize) -> Self {
        Screen {
            w,
            h,
            pixels: vec![0; w * h * 4],
            color: Color::rgba(255.0, 255.0, 255.0, 255.0),
            map_colors: MapColors::default(),
        }
    }

    /// Clears to black; the game starts every frame from a blank screen.
    pub fn reset(&mut self) {
        self.pixels
            .iter_mut()
            .enumerate()
            .for_each(|(i, p)| *p = if i % 4 == 3 { 255 } else { 0 });
        self.color = Color::rgba(255.0, 255.0, 255.0, 255.0);
    }

    fn blend(&mut self, x: i64, y: i64, c: Color) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        let a = c.a / 255.0;
        let p = &mut self.pixels[i..i + 4];
        p[0] = (p[0] as f64 * (1.0 - a) + c.r * a).round() as u8;
        p[1] = (p[1] as f64 * (1.0 - a) + c.g * a).round() as u8;
        p[2] = (p[2] as f64 * (1.0 - a) + c.b * a).round() as u8;
        p[3] = 255;
    }

    fn plot(&mut self, x: i64, y: i64) {
        self.blend(x, y, self.color);
    }

    pub fn clear(&mut self) {
        let c = self.color;
        for y in 0..self.h as i64 {
            for x in 0..self.w as i64 {
                // drawClear replaces rather than blends.
                let i = (y as usize * self.w + x as usize) * 4;
                self.pixels[i] = c.r as u8;
                self.pixels[i + 1] = c.g as u8;
                self.pixels[i + 2] = c.b as u8;
                self.pixels[i + 3] = 255;
            }
        }
    }

    pub fn rect_fill(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let (x0, y0) = (x.round() as i64, y.round() as i64);
        let (x1, y1) = ((x + w).round() as i64, (y + h).round() as i64);
        for py in y0.max(0)..y1.min(self.h as i64) {
            for px in x0.max(0)..x1.min(self.w as i64) {
                self.plot(px, py);
            }
        }
    }

    /// Outline; like the game it covers (w+1) x (h+1) pixels.
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let (x0, y0) = (x.round() as i64, y.round() as i64);
        let (x1, y1) = ((x + w).round() as i64, (y + h).round() as i64);
        for px in x0..=x1 {
            self.plot(px, y0);
            if y1 != y0 {
                self.plot(px, y1);
            }
        }
        for py in y0 + 1..y1 {
            self.plot(x0, py);
            if x1 != x0 {
                self.plot(x1, py);
            }
        }
    }

    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        let (mut x, mut y) = (x1.floor() as i64, y1.floor() as i64);
        let (ex, ey) = (x2.floor() as i64, y2.floor() as i64);
        let dx = (ex - x).abs();
        let dy = -(ey - y).abs();
        let sx = if x < ex { 1 } else { -1 };
        let sy = if y < ey { 1 } else { -1 };
        let mut err = dx + dy;
        // The end point itself is not drawn.
        while x != ex || y != ey {
            self.plot(x, y);
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    pub fn circle(&mut self, cx: f64, cy: f64, r: f64) {
        if r < 0.0 {
            return;
        }
        // Plot the outline as the boundary of the filled disc.
        let (x0, x1) = ((cx - r - 1.0).floor() as i64, (cx + r + 1.0).ceil() as i64);
        let (y0, y1) = ((cy - r - 1.0).floor() as i64, (cy + r + 1.0).ceil() as i64);
        let inside = |x: i64, y: i64| {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            dx * dx + dy * dy <= r * r
        };
        for y in y0..=y1 {
            for x in x0..=x1 {
                if inside(x, y)
                    && (!inside(x - 1, y)
                        || !inside(x + 1, y)
                        || !inside(x, y - 1)
                        || !inside(x, y + 1))
                {
                    self.plot(x, y);
                }
            }
        }
    }

    pub fn circle_fill(&mut self, cx: f64, cy: f64, r: f64) {
        let (x0, x1) = ((cx - r).floor() as i64, (cx + r).ceil() as i64);
        let (y0, y1) = ((cy - r).floor() as i64, (cy + r).ceil() as i64);
        for y in y0.max(0)..=y1.min(self.h as i64) {
            for x in x0.max(0)..=x1.min(self.w as i64) {
                let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
                if dx * dx + dy * dy <= r * r {
                    self.plot(x, y);
                }
            }
        }
    }

    pub fn triangle(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) {
        self.line(x1, y1, x2, y2);
        self.line(x2, y2, x3, y3);
        self.line(x3, y3, x1, y1);
    }

    pub fn triangle_fill(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) {
        let minx = x1.min(x2).min(x3).floor().max(0.0) as i64;
        let maxx = x1.max(x2).max(x3).ceil().min(self.w as f64) as i64;
        let miny = y1.min(y2).min(y3).floor().max(0.0) as i64;
        let maxy = y1.max(y2).max(y3).ceil().min(self.h as f64) as i64;
        let edge = |ax: f64, ay: f64, bx: f64, by: f64, px: f64, py: f64| {
            (bx - ax) * (py - ay) - (by - ay) * (px - ax)
        };
        let area = edge(x1, y1, x2, y2, x3, y3);
        if area == 0.0 {
            return;
        }
        for y in miny..maxy {
            for x in minx..maxx {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = edge(x2, y2, x3, y3, px, py) * area.signum();
                let w1 = edge(x3, y3, x1, y1, px, py) * area.signum();
                let w2 = edge(x1, y1, x2, y2, px, py) * area.signum();
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    self.plot(x, y);
                }
            }
        }
    }

    pub fn text(&mut self, x: f64, y: f64, text: &str) {
        let (x0, y0) = (x.floor() as i64, y.floor() as i64);
        let mut cx = x0;
        let mut cy = y0;
        for ch in text.chars() {
            if ch == '\n' {
                cx = x0;
                cy += LINE_H as i64;
                continue;
            }
            self.glyph(cx, cy, ch);
            cx += ADVANCE as i64;
        }
    }

    fn glyph(&mut self, x: i64, y: i64, ch: char) {
        let rows = font::glyph(ch);
        for (r, bits) in rows.iter().enumerate() {
            for c in 0..font::GLYPH_W {
                if bits & (1 << (3 - c)) != 0 {
                    self.plot(x + c as i64, y + r as i64);
                }
            }
        }
    }

    /// `drawTextBox`: word-wrapped text inside a box. Alignment is -1 (left /
    /// top), 0 (centre) or 1 (right / bottom).
    #[allow(clippy::too_many_arguments)]
    pub fn text_box(
        &mut self,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        text: &str,
        h_align: f64,
        v_align: f64,
    ) {
        let max_chars = ((w + 1.0) / ADVANCE as f64).floor().max(1.0) as usize;
        let lines = wrap(text, max_chars);
        let text_h = lines.len() as f64 * LINE_H as f64 - 1.0;
        let ty = y + (h - text_h) * (v_align.clamp(-1.0, 1.0) + 1.0) / 2.0;
        for (i, line) in lines.iter().enumerate() {
            let lw = line.chars().count() as f64 * ADVANCE as f64 - 1.0;
            let lx = x + (w - lw) * (h_align.clamp(-1.0, 1.0) + 1.0) / 2.0;
            let ly = ty + (i as i32 * LINE_H) as f64;
            if ly < y - 0.5 || ly + GLYPH_H as f64 > y + h + 0.5 {
                continue;
            }
            self.text(lx.floor(), ly.floor(), line);
        }
    }

    /// No world map data is available offline, so the map is drawn as ocean
    /// with a 1 km grid to make panning and zooming visible.
    pub fn map(&mut self, map_x: f64, map_y: f64, zoom: f64) {
        let ocean = self.map_colors.ocean;
        let grid = self.map_colors.shallows;
        let zoom = zoom.clamp(0.1, 50.0);
        for py in 0..self.h as i64 {
            for px in 0..self.w as i64 {
                let (wx, wy) = screen_to_map(
                    map_x,
                    map_y,
                    zoom,
                    self.w as f64,
                    self.h as f64,
                    px as f64,
                    py as f64,
                );
                let (nx, ny) = screen_to_map(
                    map_x,
                    map_y,
                    zoom,
                    self.w as f64,
                    self.h as f64,
                    px as f64 + 1.0,
                    py as f64 + 1.0,
                );
                let on_grid = (wx / 1000.0).floor() != (nx / 1000.0).floor()
                    || (wy / 1000.0).floor() != (ny / 1000.0).floor();
                self.blend(px, py, if on_grid { grid } else { ocean });
            }
        }
    }
}

fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let mut word = word.to_string();
            loop {
                let need = if cur.is_empty() {
                    word.chars().count()
                } else {
                    cur.chars().count() + 1 + word.chars().count()
                };
                if need <= max {
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(&word);
                    break;
                }
                if !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                    continue;
                }
                // A single word longer than the box is split.
                let head: String = word.chars().take(max).collect();
                word = word.chars().skip(max).collect();
                lines.push(head);
                if word.is_empty() {
                    break;
                }
            }
        }
        lines.push(cur);
    }
    lines
}

/// Metres per pixel are `zoom * 1000 / screen width`; world Y points north.
pub fn screen_to_map(
    map_x: f64,
    map_y: f64,
    zoom: f64,
    sw: f64,
    sh: f64,
    px: f64,
    py: f64,
) -> (f64, f64) {
    let scale = zoom * 1000.0 / sw;
    (
        map_x + (px - sw / 2.0) * scale,
        map_y - (py - sh / 2.0) * scale,
    )
}

pub fn map_to_screen(
    map_x: f64,
    map_y: f64,
    zoom: f64,
    sw: f64,
    sh: f64,
    wx: f64,
    wy: f64,
) -> (f64, f64) {
    let scale = zoom * 1000.0 / sw;
    (
        sw / 2.0 + (wx - map_x) / scale,
        sh / 2.0 - (wy - map_y) / scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words() {
        assert_eq!(wrap("hello world foo", 11), vec!["hello world", "foo"]);
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn map_roundtrip() {
        let (px, py) = map_to_screen(100.0, 200.0, 2.0, 96.0, 64.0, 350.0, -40.0);
        let (wx, wy) = screen_to_map(100.0, 200.0, 2.0, 96.0, 64.0, px, py);
        assert!((wx - 350.0).abs() < 1e-9 && (wy + 40.0).abs() < 1e-9);
    }

    #[test]
    fn rect_sizes() {
        let mut s = Screen::new(8, 8);
        s.reset();
        s.rect_fill(0.0, 0.0, 2.0, 2.0);
        let lit = |s: &Screen| s.pixels.chunks(4).filter(|p| p[0] == 255).count();
        assert_eq!(lit(&s), 4);
        s.reset();
        s.rect(0.0, 0.0, 2.0, 2.0);
        assert_eq!(lit(&s), 8);
    }
}
