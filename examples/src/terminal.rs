//! A small 80x24 text terminal for the LCD.
//!
//! [`Terminal`] interprets a byte stream with the common VT100/ANSI subset
//! used by telnet servers (cursor positioning, erase, cursor moves, CR/LF,
//! backspace, tab) and keeps the result in a character grid. Colour codes and
//! unknown sequences are parsed and ignored.
//!
//! [`TerminalView`] draws the grid with the 10x20 font, which makes 80x24
//! characters exactly fill the 800x480 panel. The glyphs are rendered once
//! into internal RAM, so drawing a character is a plain memory copy with no
//! font data read from flash (flash shares the cache and memory path with the
//! PSRAM the LCD streams from). It only redraws cells that changed, and at
//! most [`TerminalView::CELLS_PER_FRAME`] per call, to keep the PSRAM traffic
//! low (see the `display` module docs).

//!
//! The parser has no hardware dependencies and is unit-tested on a PC; the
//! view is built only for the chip.

#[cfg(target_arch = "riscv32")]
pub use view::TerminalView;

/// Columns of the terminal.
pub const COLS: usize = 80;
/// Rows of the terminal.
pub const ROWS: usize = 24;
/// Most CSI parameters kept (e.g. `ESC [ row ; col H` uses two); further
/// parameters are parsed and dropped.
const MAX_PARAMS: usize = 4;

/// The character grid, `[row][col]`, printable ASCII.
pub type Grid = [[u8; COLS]; ROWS];

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    /// After `ESC`.
    Escape,
    /// After `ESC [`, collecting parameters.
    Csi,
}

/// The terminal state: grid, cursor and parser.
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
    /// An empty screen with the cursor at the top left.
    #[must_use]
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

    /// The current characters.
    #[must_use]
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

    /// Interprets bytes from the remote side.
    pub fn feed(&mut self, data: &[u8]) {
        for &b in data {
            self.feed_byte(b);
        }
    }

    fn feed_byte(&mut self, b: u8) {
        match b {
            // ESC starts a new sequence from any state, abandoning an
            // unfinished one (VT100 / ECMA-48).
            0x1b => self.state = State::Escape,
            // Other C0 controls are executed in any state, also in the middle
            // of a sequence, which then continues.
            0x00..=0x1f => self.control(b),
            _ => match self.state {
                State::Ground => self.ground(b),
                State::Escape => self.escape(b),
                State::Csi => self.csi(b),
            },
        }
    }

    fn control(&mut self, b: u8) {
        match b {
            b'\r' => self.col = 0,
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            0x08 => self.col = self.col.saturating_sub(1).min(COLS - 1),
            b'\t' => self.col = ((self.col / 8 + 1) * 8).min(COLS - 1),
            // CAN and SUB cancel a sequence.
            0x18 | 0x1a => self.state = State::Ground,
            // Other control characters are ignored.
            _ => {}
        }
    }

    fn ground(&mut self, b: u8) {
        // Printable ASCII; DEL and non-ASCII bytes are ignored.
        if (0x20..=0x7e).contains(&b) {
            self.put(b);
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
            // `:` separates sub-parameters (e.g. `38:2:r:g:b`); counted like
            // `;`, which is enough for the commands handled here. At most one
            // past MAX_PARAMS is counted: digits of later parameters land in
            // no slot (`get_mut` is None), and `param()` returns the default.
            b';' | b':' => self.param_count = (self.param_count.max(1) + 1).min(MAX_PARAMS + 1),
            // Private-mode markers and intermediates: keep collecting.
            b'<'..=b'?' | b' '..=b'/' => {}
            0x40..=0x7e => {
                self.state = State::Ground;
                self.dispatch(b);
            }
            // DEL and non-ASCII bytes are ignored inside a sequence.
            _ => {}
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
                    self.clear_line_to(self.row, self.col);
                }
                _ => self.grid = [[b' '; COLS]; ROWS],
            },
            b'K' => match self.param(0, 0) {
                0 => self.clear_line_from(self.row, self.col),
                1 => self.clear_line_to(self.row, self.col),
                _ => self.grid[self.row] = [b' '; COLS],
            },
            // SGR (colours, bold, ...) and everything else: ignored.
            _ => {}
        }
    }

    /// Clears from `col` to the end of the line.
    fn clear_line_from(&mut self, row: usize, col: usize) {
        self.grid[row][col.min(COLS)..].fill(b' ');
    }

    /// Clears from the start of the line to `col`, inclusive. After the last
    /// column has been written the cursor sits at `COLS` (the next character
    /// wraps), so clamp to the last column.
    fn clear_line_to(&mut self, row: usize, col: usize) {
        self.grid[row][..=col.min(COLS - 1)].fill(b' ');
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

/// Draws the grid on the LCD (chip only).
#[cfg(target_arch = "riscv32")]
mod view {
    use core::convert::Infallible;

    use embedded_graphics::{
        Pixel,
        mono_font::{MonoTextStyleBuilder, ascii::FONT_10X20},
        pixelcolor::{Rgb565, raw::RawU16},
        prelude::*,
        text::{Baseline, Text},
    };
    use esp32_s31_korvo::display::Canvas;
    use static_cell::ConstStaticCell;

    use super::{COLS, Grid, ROWS};

    const CELL_W: usize = 10;
    const CELL_H: usize = 20;
    const GLYPH_BYTES: usize = CELL_W * CELL_H * 2;
    /// Printable ASCII, `' '..='~'`.
    const GLYPHS: usize = 95;

    type GlyphSet = [[u8; GLYPH_BYTES]; GLYPHS];

    /// Pre-rendered glyphs, in internal RAM (`.bss`).
    static GLYPH_SET: ConstStaticCell<GlyphSet> = ConstStaticCell::new([[0; GLYPH_BYTES]; GLYPHS]);

    /// A one-character [`DrawTarget`] used to render a glyph into RAM.
    struct GlyphTarget<'a>(&'a mut [u8; GLYPH_BYTES]);

    impl OriginDimensions for GlyphTarget<'_> {
        fn size(&self) -> Size {
            Size::new(CELL_W as u32, CELL_H as u32)
        }
    }

    impl DrawTarget for GlyphTarget<'_> {
        type Color = Rgb565;
        type Error = Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if let (Ok(x), Ok(y)) = (usize::try_from(point.x), usize::try_from(point.y))
                    && x < CELL_W
                    && y < CELL_H
                {
                    let i = (y * CELL_W + x) * 2;
                    self.0[i..i + 2]
                        .copy_from_slice(&RawU16::from(color).into_inner().to_le_bytes());
                }
            }
            Ok(())
        }
    }

    /// Draws a [`Grid`] incrementally onto the display [`Canvas`].
    pub struct TerminalView {
        /// What is currently on the panel.
        shown: Grid,
        glyphs: &'static GlyphSet,
    }

    impl TerminalView {
        /// Upper bound of cells redrawn per [`Self::draw`] call (~32 KB of
        /// pixels). Each glyph row is 20 bytes, so the cache first fills the
        /// whole 64-byte line from PSRAM; 240 cells per frame (~300 KB of line
        /// fills) still made the LCD stream drift during busy scenes. A full
        /// screen change takes ~0.7 s at this rate.
        pub const CELLS_PER_FRAME: usize = 80;

        /// Renders the glyphs in `foreground` on `background`. `shown` starts as
        /// all spaces, so the panel must start out filled with `background`.
        ///
        /// Can only be called once: the glyphs are kept in a static buffer.
        pub fn new(foreground: Rgb565, background: Rgb565) -> Self {
            let glyphs = GLYPH_SET.take();
            let style = MonoTextStyleBuilder::new()
                .font(&FONT_10X20)
                .text_color(foreground)
                .background_color(background)
                .build();
            for (glyph, ch) in glyphs.iter_mut().zip(b' '..=b'~') {
                let text = [ch];
                let text = core::str::from_utf8(&text).unwrap_or(" ");
                let _ = Text::with_baseline(text, Point::zero(), style, Baseline::Top)
                    .draw(&mut GlyphTarget(glyph));
            }
            Self {
                shown: [[b' '; COLS]; ROWS],
                glyphs,
            }
        }

        /// Redraws up to [`Self::CELLS_PER_FRAME`] cells where `grid` differs from
        /// the panel and returns how many differing cells are left.
        pub fn draw(&mut self, grid: &Grid, canvas: &mut Canvas<'_>) -> usize {
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
                    let index = usize::from(w.clamp(b' ', b'~') - b' ');
                    canvas.blit(
                        col * CELL_W,
                        row * CELL_H,
                        CELL_W,
                        CELL_H,
                        &self.glyphs[index],
                    );
                    *h = w;
                }
            }
            left
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(t: &Terminal, row: usize) -> &[u8] {
        &t.grid()[row]
    }

    #[test]
    fn erase_to_start_after_a_full_line_does_not_panic() {
        // Regression: after 80 characters the cursor is at column 80, and the
        // inclusive "erase to start" range ran past the line.
        for seq in [&b"\x1b[1K"[..], b"\x1b[1J"] {
            let mut t = Terminal::new();
            t.feed(&[b'x'; COLS]);
            t.feed(seq);
            assert!(line(&t, 0).iter().all(|&c| c == b' '));
        }
        let mut t = Terminal::new();
        t.feed(&[b'x'; COLS]);
        t.feed(b"\x1b[K");
        assert!(line(&t, 0).iter().all(|&c| c == b'x'));
    }

    #[test]
    fn cursor_position_is_clamped() {
        let mut t = Terminal::new();
        t.feed(b"\x1b[99;200HZ");
        assert_eq!(t.grid()[ROWS - 1][COLS - 1], b'Z');
        t.feed(b"\x1b[2;3HA");
        assert_eq!(t.grid()[1][2], b'A');
    }

    #[test]
    fn esc_inside_csi_starts_a_new_sequence() {
        let mut t = Terminal::new();
        t.feed(b"\x1b[1;2\x1b[5;6HX");
        assert_eq!(t.grid()[4][5], b'X');
        assert!(!line(&t, 0).contains(&b'['));
    }

    #[test]
    fn controls_inside_csi_are_executed() {
        let mut t = Terminal::new();
        t.feed(b"abc\x1b[1;\r2HX");
        // CR ran (cursor to column 0) and the sequence went on to `2H`.
        assert_eq!(t.grid()[0][1], b'X');
        assert_eq!(&line(&t, 0)[..3], b"aXc");
    }

    #[test]
    fn colon_parameters_do_not_abort() {
        let mut t = Terminal::new();
        t.feed(b"\x1b[38:2:1:2:3mA");
        assert_eq!(t.grid()[0][0], b'A');
    }

    #[test]
    fn many_parameters_are_dropped_safely() {
        let mut t = Terminal::new();
        t.feed(b"\x1b[1;2;3;4;5;6;7HB");
        assert_eq!(t.grid()[0][1], b'B');
    }

    #[test]
    fn wrap_and_scroll() {
        let mut t = Terminal::new();
        t.feed(&[b'a'; COLS]);
        t.feed(b"b");
        assert_eq!(t.grid()[1][0], b'b');
        for _ in 0..ROWS {
            t.feed(b"\r\n");
        }
        t.feed(b"z");
        assert_eq!(t.grid()[ROWS - 1][0], b'z');
        assert!(t.grid().iter().take(ROWS - 1).flatten().all(|&c| c == b' '));
    }

    #[test]
    fn erase_display_variants() {
        let mut t = Terminal::new();
        t.write_str("one\ntwo\nthree");
        t.feed(b"\x1b[2;2H\x1b[0J");
        assert_eq!(&line(&t, 0)[..3], b"one");
        assert_eq!(&line(&t, 1)[..3], b"t  ");
        assert!(line(&t, 2).iter().all(|&c| c == b' '));
        t.feed(b"\x1b[2J");
        assert!(t.grid().iter().flatten().all(|&c| c == b' '));
    }

    #[test]
    fn backspace_after_full_line_stays_on_the_line() {
        let mut t = Terminal::new();
        t.feed(&[b'a'; COLS]);
        t.feed(b"\x08Q");
        assert_eq!(t.grid()[0][COLS - 1], b'Q');
    }
}
