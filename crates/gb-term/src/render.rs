//! Framebuffer -> half-block cells -> ANSI escape sequences.

use gb_core::{SCREEN_HEIGHT, SCREEN_WIDTH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Upper pixel (foreground of `▀`), 0x00RRGGBB.
    pub top: u32,
    /// Lower pixel (background), 0x00RRGGBB.
    pub bot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Gray,
    DmgGreen,
    Pocket,
}

impl Palette {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "gray" | "grey" => Some(Palette::Gray),
            "dmg-green" => Some(Palette::DmgGreen),
            "pocket" => Some(Palette::Pocket),
            _ => None,
        }
    }

    /// Map one of the core's four exact shades (0xFFFFFF, 0xAAAAAA, 0x555555, 0x000000) to a colour.
    /// Other values pass through unchanged.
    pub fn map(self, px: u32) -> u32 {
        let shade = match px & 0xFF_FFFF {
            0xFFFFFF => 0,
            0xAAAAAA => 1,
            0x555555 => 2,
            0x000000 => 3,
            other => return other,
        };
        let table: [u32; 4] = match self {
            Palette::Gray => return px & 0xFF_FFFF,
            Palette::DmgGreen => [0x9BBC0F, 0x8BAC0F, 0x306230, 0x0F380F],
            Palette::Pocket => [0xC4CFA1, 0x8B956D, 0x4D533C, 0x1F1F1F],
        };
        table[shade]
    }
}

/// Output size in cells, fitted (aspect preserved, never upscaled) into `cols` x `rows` terminal cells.
/// A cell is 1 pixel wide and 2 pixels tall, so pixels stay square.
pub fn fit(cols: usize, rows: usize) -> (usize, usize) {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let full_h = SCREEN_HEIGHT / 2;
    if cols >= SCREEN_WIDTH && rows >= full_h {
        return (SCREEN_WIDTH, full_h);
    }
    // scale s = out_w / 160, limited by both dimensions
    let s = (cols as f64 / SCREEN_WIDTH as f64).min((rows * 2) as f64 / SCREEN_HEIGHT as f64);
    let w = ((SCREEN_WIDTH as f64 * s) as usize).clamp(1, cols);
    let h = (((SCREEN_HEIGHT as f64 * s) / 2.0).round() as usize).clamp(1, rows);
    (w, h)
}

/// Nearest-neighbour sample a `SCREEN_WIDTH x SCREEN_HEIGHT` framebuffer into `w x h` cells (`2h` pixel rows).
pub fn to_cells(fb: &[u32], w: usize, h: usize, palette: Palette) -> Vec<Cell> {
    to_cells_sized(fb, SCREEN_WIDTH, SCREEN_HEIGHT, w, h, palette)
}

pub fn to_cells_sized(fb: &[u32], sw: usize, sh: usize, w: usize, h: usize, palette: Palette) -> Vec<Cell> {
    let px_h = h * 2;
    let sample = |x: usize, y: usize| palette.map(fb[(y * sh / px_h).min(sh - 1) * sw + (x * sw / w).min(sw - 1)]);
    let mut cells = Vec::with_capacity(w * h);
    for cy in 0..h {
        for x in 0..w {
            cells.push(Cell {
                top: sample(x, cy * 2),
                bot: sample(x, cy * 2 + 1),
            });
        }
    }
    cells
}

fn push_color(out: &mut Vec<u8>, base: u8, rgb: u32) {
    out.extend_from_slice(format!("\x1b[{base};2;{};{};{}m", rgb >> 16 & 0xFF, rgb >> 8 & 0xFF, rgb & 0xFF).as_bytes());
}

const HALF_BLOCK: &str = "\u{2580}";

/// Emits only the cells that changed since the previous call.
#[derive(Default)]
pub struct Encoder {
    prev: Vec<Cell>,
    dims: (usize, usize),
    ox: usize,
    oy: usize,
}

impl Encoder {
    /// Forget the previous frame (e.g. after a resize / screen clear): the next encode is a full redraw.
    pub fn invalidate(&mut self) {
        self.prev.clear();
    }

    /// Append the escape sequences that bring the screen from the previous frame to `cells`
    /// (`w x h`, drawn with its top-left at 0-based screen cell `(ox, oy)`). Appends nothing if unchanged.
    /// Colours are left set at the end; callers should reset attributes afterwards.
    pub fn encode(&mut self, cells: &[Cell], w: usize, h: usize, ox: usize, oy: usize, out: &mut Vec<u8>) {
        assert_eq!(cells.len(), w * h);
        let full = self.prev.len() != cells.len() || self.dims != (w, h) || (self.ox, self.oy) != (ox, oy);
        let mut cursor: Option<(usize, usize)> = None;
        let (mut fg, mut bg) = (None::<u32>, None::<u32>);
        for y in 0..h {
            for x in 0..w {
                let c = cells[y * w + x];
                if !full && self.prev[y * w + x] == c {
                    continue;
                }
                if cursor != Some((x, y)) {
                    out.extend_from_slice(format!("\x1b[{};{}H", oy + y + 1, ox + x + 1).as_bytes());
                }
                if fg != Some(c.top) {
                    push_color(out, 38, c.top);
                    fg = Some(c.top);
                }
                if bg != Some(c.bot) {
                    push_color(out, 48, c.bot);
                    bg = Some(c.bot);
                }
                out.extend_from_slice(HALF_BLOCK.as_bytes());
                // after the last column the cursor is in "pending wrap" state: force an explicit move
                cursor = if x + 1 < w { Some((x + 1, y)) } else { None };
            }
        }
        self.prev.clear();
        self.prev.extend_from_slice(cells);
        self.dims = (w, h);
        self.ox = ox;
        self.oy = oy;
    }
}

/// Whole frame as plain ANSI text (one line per cell row, no cursor addressing): suitable for `cat`.
pub fn render_plain(cells: &[Cell], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..h {
        let (mut fg, mut bg) = (None::<u32>, None::<u32>);
        for c in &cells[y * w..(y + 1) * w] {
            if fg != Some(c.top) {
                push_color(&mut out, 38, c.top);
                fg = Some(c.top);
            }
            if bg != Some(c.bot) {
                push_color(&mut out, 48, c.bot);
                bg = Some(c.bot);
            }
            out.extend_from_slice(HALF_BLOCK.as_bytes());
        }
        out.extend_from_slice(b"\x1b[0m\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 0xFFFFFF;
    const L: u32 = 0xAAAAAA;
    const D: u32 = 0x555555;
    const B: u32 = 0x000000;

    #[test]
    fn two_by_two_frame_becomes_two_half_block_cells() {
        // pixels: row0 = W L, row1 = D B  ->  cell0 (top W, bottom D), cell1 (top L, bottom B)
        let fb = [W, L, D, B];
        let cells = to_cells_sized(&fb, 2, 2, 2, 1, Palette::Gray);
        assert_eq!(cells, [Cell { top: W, bot: D }, Cell { top: L, bot: B }]);
        let mut enc = Encoder::default();
        let mut out = Vec::new();
        enc.encode(&cells, 2, 1, 0, 0, &mut out);
        let expect =
            "\x1b[1;1H\x1b[38;2;255;255;255m\x1b[48;2;85;85;85m\u{2580}\x1b[38;2;170;170;170m\x1b[48;2;0;0;0m\u{2580}";
        assert_eq!(String::from_utf8(out).unwrap(), expect);
    }

    #[test]
    fn unchanged_frame_emits_nothing_and_change_emits_only_that_cell() {
        let mut cells = vec![Cell { top: W, bot: D }; 6]; // 3x2
        let mut enc = Encoder::default();
        let mut out = Vec::new();
        enc.encode(&cells, 3, 2, 4, 2, &mut out);
        assert!(!out.is_empty());
        out.clear();
        enc.encode(&cells, 3, 2, 4, 2, &mut out);
        assert!(out.is_empty());
        cells[4] = Cell { top: B, bot: W }; // x=1, y=1 -> screen col 4+1, row 2+1 (1-based: 6;4)
        enc.encode(&cells, 3, 2, 4, 2, &mut out);
        assert_eq!(
            String::from_utf8(out.clone()).unwrap(),
            "\x1b[4;6H\x1b[38;2;0;0;0m\x1b[48;2;255;255;255m\u{2580}"
        );
        // invalidate forces a full redraw
        out.clear();
        enc.invalidate();
        enc.encode(&cells, 3, 2, 4, 2, &mut out);
        assert_eq!(String::from_utf8(out).unwrap().matches('\u{2580}').count(), 6);
    }

    #[test]
    fn adjacent_changes_share_cursor_and_colours() {
        let mut cells = vec![Cell { top: W, bot: W }; 4];
        let mut enc = Encoder::default();
        let mut out = Vec::new();
        enc.encode(&cells, 4, 1, 0, 0, &mut out);
        out.clear();
        cells[1] = Cell { top: B, bot: B };
        cells[2] = Cell { top: B, bot: B };
        enc.encode(&cells, 4, 1, 0, 0, &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b[1;2H\x1b[38;2;0;0;0m\x1b[48;2;0;0;0m\u{2580}\u{2580}"
        );
    }

    #[test]
    fn fit_preserves_aspect_and_never_upscales() {
        assert_eq!(fit(300, 100), (160, 72));
        assert_eq!(fit(80, 100), (80, 36));
        assert_eq!(fit(160, 36), (80, 36));
        let (w, h) = fit(40, 10);
        assert!(w <= 40 && h <= 10 && w >= 1);
        assert_eq!(fit(0, 0), (1, 1));
    }

    #[test]
    fn palette_maps_exact_shades_only() {
        assert_eq!(Palette::Gray.map(0xAAAAAA), 0xAAAAAA);
        assert_eq!(Palette::DmgGreen.map(0xFFFFFF), 0x9BBC0F);
        assert_eq!(Palette::DmgGreen.map(0x000000), 0x0F380F);
        assert_eq!(Palette::Pocket.map(0x123456), 0x123456);
    }

    #[test]
    fn downscale_samples_nearest() {
        // 4x4 source into 2x1 cells (2 pixel rows): picks columns 0,2 and rows 0,2
        let fb: Vec<u32> = (0..16).collect();
        let cells = to_cells_sized(&fb, 4, 4, 2, 1, Palette::Gray);
        assert_eq!(cells, [Cell { top: 0, bot: 8 }, Cell { top: 2, bot: 10 }]);
    }
}
