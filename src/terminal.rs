//! A small 80x24 text terminal for the LCD.
//!
//! [`Terminal`] interprets a byte stream with the common VT100/ANSI subset
//! used by telnet servers (cursor positioning, erase, cursor moves, CR/LF,
//! backspace, tab) and keeps the result in a character grid. Colour codes and
//! unknown sequences are parsed and ignored.
//!
//! [`TerminalView`] draws the grid with the 10x20 font, which makes 80x24
//! characters exactly fill the 800x480 panel. It only redraws cells that
//! changed, and at most [`TerminalView::CELLS_PER_FRAME`] per call, to keep
//! the PSRAM traffic during scan-out low (see the `display` module docs).

use embedded_graphics::{
    mono_font::{MonoTextStyle, MonoTextStyleBuilder, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    text::{Baseline, Text},
};

pub const COLS: usize = 80;
pub const ROWS: usize = 24;
const CELL_W: i32 = 10;
const CELL_H: i32 = 20;
/// Most CSI parameters we keep (e.g. `ESC [ row ; col H` uses two).
const MAX_PARAMS: usize = 4;

pub type Grid = [[u8; COLS]; ROWS];

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// After `ESC`.
    Escape,
    /// After `ESC [`, collecting parameters.
    Csi,
}

pub struct Terminal {
    grid: Grid,
    row: usize,
    col: usize,
    state: State,
    params: [u16; MAX_PARAMS],
    param_count: usize,
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminal {
    pub const fn new() -> Self {
        Self {
            grid: [[b' '; COLS]; ROWS],
            row: 0,
            col: 0,
            state: State::Ground,
            params: [0; MAX_PARAMS],
            param_count: 0,
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Clears the screen and homes the cursor.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Writes plain text, e.g. status messages (`\n` starts a new line).
    pub fn write_str(&mut self, text: &str) {
        for b in text.bytes() {
            if b == b'\n' {
                self.feed_byte(b'\r');
            }
            self.feed_byte(b);
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        for &b in data {
            self.feed_byte(b);
        }
    }

    fn feed_byte(&mut self, b: u8) {
        match self.state {
            State::Ground => self.ground(b),
            State::Escape => self.escape(b),
            State::Csi => self.csi(b),
        }
    }

    fn ground(&mut self, b: u8) {
        match b {
            0x1b => self.state = State::Escape,
            b'\r' => self.col = 0,
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            0x08 => self.col = self.col.saturating_sub(1),
            b'\t' => self.col = ((self.col / 8 + 1) * 8).min(COLS - 1),
            0x20..=0x7e => self.put(b),
            // Other control characters and non-ASCII bytes are ignored.
            _ => {}
        }
    }

    fn escape(&mut self, b: u8) {
        self.state = State::Ground;
        match b {
            b'[' => {
                self.state = State::Csi;
                self.params = [0; MAX_PARAMS];
                self.param_count = 0;
            }
            // RIS: full reset.
            b'c' => self.reset(),
            // Anything else (charset selection, keypad modes, ...) is ignored.
            _ => {}
        }
    }

    fn csi(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                if self.param_count == 0 {
                    self.param_count = 1;
                }
                if let Some(p) = self.params.get_mut(self.param_count - 1) {
                    *p = p.saturating_mul(10).saturating_add(u16::from(b - b'0'));
                }
            }
            b';' => self.param_count = (self.param_count.max(1) + 1).min(MAX_PARAMS + 1),
            // Private-mode markers and intermediates: keep collecting.
            b'?' | b'>' | b'=' | b' '..=b'/' => {}
            0x40..=0x7e => {
                self.state = State::Ground;
                self.dispatch(b);
            }
            // A control character inside a sequence aborts it.
            _ => self.state = State::Ground,
        }
    }

    /// Parameter `i`, with `default` for missing or zero values.
    fn param(&self, i: usize, default: u16) -> usize {
        match self.params.get(i) {
            Some(&p) if i < self.param_count && p != 0 => usize::from(p),
            _ => usize::from(default),
        }
    }

    fn dispatch(&mut self, command: u8) {
        match command {
            b'H' | b'f' => {
                self.row = (self.param(0, 1) - 1).min(ROWS - 1);
                self.col = (self.param(1, 1) - 1).min(COLS - 1);
            }
            b'A' => self.row = self.row.saturating_sub(self.param(0, 1)),
            b'B' => self.row = (self.row + self.param(0, 1)).min(ROWS - 1),
            b'C' => self.col = (self.col + self.param(0, 1)).min(COLS - 1),
            b'D' => self.col = self.col.saturating_sub(self.param(0, 1)),
            b'G' => self.col = (self.param(0, 1) - 1).min(COLS - 1),
            b'd' => self.row = (self.param(0, 1) - 1).min(ROWS - 1),
            b'J' => match self.param(0, 0) {
                0 => {
                    self.clear_line_from(self.row, self.col);
                    for row in self.row + 1..ROWS {
                        self.grid[row] = [b' '; COLS];
                    }
                }
                1 => {
                    for row in 0..self.row {
                        self.grid[row] = [b' '; COLS];
                    }
                    self.grid[self.row][..=self.col].fill(b' ');
                }
                _ => self.grid = [[b' '; COLS]; ROWS],
            },
            b'K' => match self.param(0, 0) {
                0 => self.clear_line_from(self.row, self.col),
                1 => self.grid[self.row][..=self.col].fill(b' '),
                _ => self.grid[self.row] = [b' '; COLS],
            },
            // SGR (colours, bold, ...) and everything else: ignored.
            _ => {}
        }
    }

    fn clear_line_from(&mut self, row: usize, col: usize) {
        self.grid[row][col..].fill(b' ');
    }

    fn put(&mut self, b: u8) {
        if self.col >= COLS {
            self.col = 0;
            self.line_feed();
        }
        self.grid[self.row][self.col] = b;
        self.col += 1;
    }

    fn line_feed(&mut self) {
        if self.row + 1 < ROWS {
            self.row += 1;
        } else {
            self.grid.copy_within(1.., 0);
            self.grid[ROWS - 1] = [b' '; COLS];
        }
    }
}

/// Draws a [`Grid`] incrementally onto a [`DrawTarget`].
pub struct TerminalView {
    /// What is currently on the panel.
    shown: Grid,
    style: MonoTextStyle<'static, Rgb565>,
}

impl TerminalView {
    /// Upper bound of cells redrawn per [`Self::draw`] call (~96 KB of pixels).
    pub const CELLS_PER_FRAME: usize = 240;

    /// `shown` starts as all spaces, so the target must start out filled with
    /// `background`.
    pub fn new(foreground: Rgb565, background: Rgb565) -> Self {
        Self {
            shown: [[b' '; COLS]; ROWS],
            style: MonoTextStyleBuilder::new()
                .font(&FONT_10X20)
                .text_color(foreground)
                .background_color(background)
                .build(),
        }
    }

    /// Redraws up to [`Self::CELLS_PER_FRAME`] cells where `grid` differs from
    /// the panel and returns how many differing cells are left.
    pub fn draw<D>(&mut self, grid: &Grid, target: &mut D) -> usize
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let mut budget = Self::CELLS_PER_FRAME;
        let mut left = 0;
        for (row, (want, have)) in grid.iter().zip(self.shown.iter_mut()).enumerate() {
            for (col, (&w, h)) in want.iter().zip(have.iter_mut()).enumerate() {
                if w == *h {
                    continue;
                }
                if budget == 0 {
                    left += 1;
                    continue;
                }
                budget -= 1;
                let glyph = [w];
                let text = core::str::from_utf8(&glyph).unwrap_or(" ");
                let origin = Point::new(col as i32 * CELL_W, row as i32 * CELL_H);
                let _ = Text::with_baseline(text, origin, self.style, Baseline::Top).draw(target);
                *h = w;
            }
        }
        left
    }
}
