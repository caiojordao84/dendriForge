//! Display framebuffers and authoritative HD44780 Character ROM tables (closes `DEF-007`).
//!
//! Reference: Hitachi HD44780U LCD-II Datasheet (ADE-207-272), Table 4 (ROM Code A00)
//! and Table 5 (ROM Code A02).
//!
//! Includes:
//! - [`DisplayCharsetSpec`]: Strict glyph-set encoder/decoder for `ascii_7bit`, `hd44780_a00`,
//!   and `hd44780_a02` (`ERR_STRING_CHARSET` ground truth).
//! - [`LcdCharFramebuffer`]: `cols x rows` character LCD framebuffer (`16x2` / `20x4`) with
//!   cursor addressing, backlight, and full PCF8574 4-bit I2C backpack [`I2cDevice`] protocol.
//! - [`OledPixelFramebuffer`]: `128x64` monochrome SSD1306 graphic pixel framebuffer with
//!   page-mode GDDRAM and native [`I2cDevice`] driver (default address `0x3C`).

use dendri_bus::{I2cDevice, I2cError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Normative display character-ROM specification (`DEF-007`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayCharsetSpec {
    /// Strict printable 7-bit ASCII (`0x20..=0x7E`).
    Ascii7Bit,
    /// Hitachi HD44780U ROM Code A00 (ASCII + `¥`, `→`, `←`, `°`, `α`, `β`, `ε`, `μ`, `Ω`, `Σ`, `π`, `÷`, `█`, half-width Katakana).
    Hd44780A00,
    /// Hitachi HD44780U ROM Code A02 (ASCII + European/Latin-1 accented glyphs `áéíóúãõçüöäñ°±µΩ`).
    Hd44780A02,
}

impl DisplayCharsetSpec {
    /// Parses a profile TOML `[display].charset` identifier.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "ascii_7bit" => Some(Self::Ascii7Bit),
            "hd44780_a00" => Some(Self::Hd44780A00),
            "hd44780_a02" => Some(Self::Hd44780A02),
            _ => None,
        }
    }

    /// Canonical TOML identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascii7Bit => "ascii_7bit",
            Self::Hd44780A00 => "hd44780_a00",
            Self::Hd44780A02 => "hd44780_a02",
        }
    }

    /// Encodes a Unicode `char` into its exact ROM byte code, or returns `None` if unsupported
    /// by this display ROM (`ERR_STRING_CHARSET`).
    #[must_use]
    pub const fn encode_char(self, ch: char) -> Option<u8> {
        match self {
            Self::Ascii7Bit => {
                if ch >= ' ' && ch <= '~' {
                    Some(ch as u8)
                } else {
                    None
                }
            }
            Self::Hd44780A00 => encode_hd44780_a00(ch),
            Self::Hd44780A02 => encode_hd44780_a02(ch),
        }
    }

    /// Returns `true` iff `ch` (including newline/CR control chars) is legal on this display ROM.
    #[must_use]
    pub const fn supports_char(self, ch: char) -> bool {
        matches!(ch, '\n' | '\r') || self.encode_char(ch).is_some()
    }

    /// Decodes a ROM byte code back to a representative Unicode glyph for inspection/rendering.
    #[must_use]
    pub const fn decode_byte(self, code: u8) -> char {
        match self {
            Self::Ascii7Bit => {
                if code >= 0x20 && code <= 0x7E {
                    code as char
                } else {
                    ' '
                }
            }
            Self::Hd44780A00 => decode_hd44780_a00(code),
            Self::Hd44780A02 => decode_hd44780_a02(code),
        }
    }
}

/// Hitachi HD44780U Datasheet Table 4 — ROM Code A00 exact encoder.
const fn encode_hd44780_a00(ch: char) -> Option<u8> {
    match ch {
        // Standard ASCII except 0x5C ('\' -> '¥') and 0x7E ('~' -> '→')
        ' '..='[' | ']'..='}' => Some(ch as u8),
        '¥' => Some(0x5C),
        '→' => Some(0x7E),
        '←' => Some(0x7F),
        '°' => Some(0xDF),
        'α' => Some(0xE0),
        'ä' => Some(0xE1),
        'β' => Some(0xE2),
        'ε' => Some(0xE3),
        'μ' | 'µ' => Some(0xE4),
        'σ' => Some(0xE5),
        'ρ' => Some(0xE6),
        '√' => Some(0xE8),
        '¢' => Some(0xEC),
        '£' => Some(0xED),
        'ñ' => Some(0xEE),
        'ö' => Some(0xEF),
        'θ' => Some(0xF2),
        '∞' => Some(0xF3),
        'Ω' => Some(0xF4),
        'ü' => Some(0xF5),
        'Σ' => Some(0xF6),
        'π' => Some(0xF7),
        '÷' => Some(0xFD),
        '█' => Some(0xFF),
        // Half-width Katakana U+FF61..=U+FF9F -> 0xA1..=0xDF
        '\u{FF61}'..='\u{FF9E}' => Some((ch as u32 - 0xFF61 + 0xA1) as u8),
        _ => None,
    }
}

const fn decode_hd44780_a00(code: u8) -> char {
    match code {
        0x20..=0x5B | 0x5D..=0x7D => code as char,
        0x5C => '¥',
        0x7E => '→',
        0x7F => '←',
        0xDF => '°',
        0xE0 => 'α',
        0xE1 => 'ä',
        0xE2 => 'β',
        0xE3 => 'ε',
        0xE4 => 'μ',
        0xE5 => 'σ',
        0xE6 => 'ρ',
        0xE8 => '√',
        0xEC => '¢',
        0xED => '£',
        0xEE => 'ñ',
        0xEF => 'ö',
        0xF2 => 'θ',
        0xF3 => '∞',
        0xF4 => 'Ω',
        0xF5 => 'ü',
        0xF6 => 'Σ',
        0xF7 => 'π',
        0xFD => '÷',
        0xFF => '█',
        _ => '?',
    }
}

/// Hitachi HD44780U Datasheet Table 5 — ROM Code A02 (European / Latin-1) exact encoder.
const fn encode_hd44780_a02(ch: char) -> Option<u8> {
    match ch {
        ' '..='~' => Some(ch as u8),
        '£' => Some(0xA3),
        '¥' => Some(0xA5),
        '°' => Some(0xB0),
        '±' => Some(0xB1),
        'µ' | 'μ' => Some(0xB5),
        'À' => Some(0xC0),
        'Á' => Some(0xC1),
        'Â' => Some(0xC2),
        'Ã' => Some(0xC3),
        'Ä' => Some(0xC4),
        'Ç' => Some(0xC7),
        'É' => Some(0xC9),
        'Ê' => Some(0xCA),
        'Í' => Some(0xCD),
        'Ñ' => Some(0xD1),
        'Ó' => Some(0xD3),
        'Ô' => Some(0xD4),
        'Õ' => Some(0xD5),
        'Ö' => Some(0xD6),
        'Ú' => Some(0xDA),
        'Ü' => Some(0xDC),
        'ß' => Some(0xDF),
        'à' => Some(0xE0),
        'á' => Some(0xE1),
        'â' => Some(0xE2),
        'ã' => Some(0xE3),
        'ä' => Some(0xE4),
        'ç' => Some(0xE7),
        'é' => Some(0xE9),
        'ê' => Some(0xEA),
        'í' => Some(0xED),
        'ñ' => Some(0xF1),
        'ó' => Some(0xF3),
        'ô' => Some(0xF4),
        'õ' => Some(0xF5),
        'ö' => Some(0xF6),
        '÷' => Some(0xF7),
        'ú' => Some(0xFA),
        'ü' => Some(0xFC),
        'Ω' => Some(0x9A),
        '█' => Some(0xFF),
        _ => None,
    }
}

const fn decode_hd44780_a02(code: u8) -> char {
    match code {
        0x20..=0x7E => code as char,
        0x9A => 'Ω',
        0xA3 => '£',
        0xA5 => '¥',
        0xB0 => '°',
        0xB1 => '±',
        0xB5 => 'µ',
        0xC0 => 'À',
        0xC1 => 'Á',
        0xC2 => 'Â',
        0xC3 => 'Ã',
        0xC4 => 'Ä',
        0xC7 => 'Ç',
        0xC9 => 'É',
        0xCA => 'Ê',
        0xCD => 'Í',
        0xD1 => 'Ñ',
        0xD3 => 'Ó',
        0xD4 => 'Ô',
        0xD5 => 'Õ',
        0xD6 => 'Ö',
        0xDA => 'Ú',
        0xDC => 'Ü',
        0xDF => 'ß',
        0xE0 => 'à',
        0xE1 => 'á',
        0xE2 => 'â',
        0xE3 => 'ã',
        0xE4 => 'ä',
        0xE7 => 'ç',
        0xE9 => 'é',
        0xEA => 'ê',
        0xED => 'í',
        0xF1 => 'ñ',
        0xF3 => 'ó',
        0xF4 => 'ô',
        0xF5 => 'õ',
        0xF6 => 'ö',
        0xF7 => '÷',
        0xFA => 'ú',
        0xFC => 'ü',
        0xFF => '█',
        _ => '?',
    }
}

/// Returned when printing a character unsupported by the target display's ROM charset (`DEF-007`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("character `{ch}` (U+{code:04X}) is unsupported by display charset `{charset}`")]
pub struct CharsetViolation {
    pub ch: char,
    pub code: u32,
    pub charset: String,
}

/// Encodes one HD44780 8-bit command (`rs = false`) or data (`rs = true`) byte into the standard
/// 4-byte PCF8574 I2C backpack nibble strobe sequence (`D7..D4 | BL | EN | RW=0 | RS`).
#[must_use]
pub fn encode_hd44780_i2c_byte(byte: u8, rs: bool, backlight: bool) -> [u8; 4] {
    let flags = u8::from(rs) | if backlight { 0x08 } else { 0x00 };
    let hi = (byte & 0xF0) | flags;
    let lo = ((byte << 4) & 0xF0) | flags;
    [hi | 0x04, hi, lo | 0x04, lo]
}

/// HD44780 character LCD framebuffer (`16x2` or `20x4`) supporting both direct prints and
/// PCF8574 4-bit I2C backpack transactions (`I2cDevice`, default address `0x27`).
#[derive(Debug, Clone)]
pub struct LcdCharFramebuffer {
    address: u8,
    cols: usize,
    rows: usize,
    charset: DisplayCharsetSpec,
    cells: Vec<u8>,
    cursor_col: usize,
    cursor_row: usize,
    backlight: bool,
    // PCF8574 4-bit nibble assembly state
    prev_en: bool,
    pending_hi_nibble: Option<(u8, bool)>,
}

impl LcdCharFramebuffer {
    /// Creates a blank `cols x rows` character LCD framebuffer.
    #[must_use]
    pub fn new(address: u8, cols: usize, rows: usize, charset: DisplayCharsetSpec) -> Self {
        let c = cols.max(1);
        let r = rows.max(1);
        Self {
            address,
            cols: c,
            rows: r,
            charset,
            cells: vec![b' '; c.saturating_mul(r)],
            cursor_col: 0,
            cursor_row: 0,
            backlight: true,
            prev_en: false,
            pending_hi_nibble: None,
        }
    }

    /// Standard 16×2 HD44780 LCD on I2C address `0x27` with ROM A00.
    #[must_use]
    pub fn hd44780_16x2(address: u8) -> Self {
        Self::new(address, 16, 2, DisplayCharsetSpec::Hd44780A00)
    }

    #[must_use]
    pub const fn cols(&self) -> usize {
        self.cols
    }

    #[must_use]
    pub const fn rows(&self) -> usize {
        self.rows
    }

    #[must_use]
    pub const fn charset(&self) -> DisplayCharsetSpec {
        self.charset
    }

    #[must_use]
    pub const fn backlight(&self) -> bool {
        self.backlight
    }

    /// Clears display RAM to spaces and resets cursor to `(0, 0)`.
    pub fn clear(&mut self) {
        self.cells.fill(b' ');
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    /// Moves cursor to `(0, 0)` without clearing display RAM.
    pub fn home(&mut self) {
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    /// Positions cursor at `(col, row)`, clamped to framebuffer bounds.
    pub fn set_cursor(&mut self, col: usize, row: usize) {
        self.cursor_col = col.min(self.cols.saturating_sub(1));
        self.cursor_row = row.min(self.rows.saturating_sub(1));
    }

    /// Prints a string using this display's authoritative ROM charset; fails cleanly on invalid glyphs.
    pub fn print_str(&mut self, text: &str) -> Result<(), CharsetViolation> {
        for ch in text.chars() {
            match ch {
                '\n' => {
                    self.cursor_row = (self.cursor_row + 1) % self.rows;
                    self.cursor_col = 0;
                }
                '\r' => {
                    self.cursor_col = 0;
                }
                other => {
                    let Some(code) = self.charset.encode_char(other) else {
                        return Err(CharsetViolation {
                            ch: other,
                            code: other as u32,
                            charset: self.charset.as_str().to_owned(),
                        });
                    };
                    self.write_raw_data_byte(code);
                }
            }
        }
        Ok(())
    }

    /// Decodes row `row` into a trimmed-free fixed-width UTF-8 string.
    #[must_use]
    pub fn line_text(&self, row: usize) -> String {
        let start = row.saturating_mul(self.cols);
        let end = start.saturating_add(self.cols);
        let Some(slice) = self.cells.get(start..end) else {
            return String::new();
        };
        slice.iter().map(|&b| self.charset.decode_byte(b)).collect()
    }

    fn write_raw_data_byte(&mut self, byte: u8) {
        let idx = self
            .cursor_row
            .saturating_mul(self.cols)
            .saturating_add(self.cursor_col);
        if let Some(slot) = self.cells.get_mut(idx) {
            *slot = byte;
        }
        self.cursor_col += 1;
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.cursor_row = (self.cursor_row + 1) % self.rows;
        }
    }

    fn execute_hd44780_command(&mut self, cmd: u8) {
        if cmd == 0x01 {
            self.clear();
        } else if (cmd & 0xFE) == 0x02 {
            self.home();
        } else if (cmd & 0x80) != 0 {
            // Set DDRAM address: row 0 = 0x00..0x27, row 1 = 0x40..0x67
            let addr = cmd & 0x7F;
            let (row, col) = if addr >= 0x40 {
                (1 % self.rows, usize::from(addr - 0x40))
            } else {
                (0, usize::from(addr))
            };
            self.set_cursor(col, row);
        }
    }
}

impl I2cDevice for LcdCharFramebuffer {
    fn address_7bit(&self) -> u8 {
        self.address
    }

    fn write(&mut self, data: &[u8]) -> Result<(), I2cError> {
        for &raw in data {
            let rs = (raw & 0x01) != 0;
            let en = (raw & 0x04) != 0;
            self.backlight = (raw & 0x08) != 0;
            let nibble = raw & 0xF0;

            // HD44780 latches each 4-bit nibble on the falling edge of EN.
            if self.prev_en && !en {
                if let Some((hi, prev_rs)) = self.pending_hi_nibble.take() {
                    let full = hi | (nibble >> 4);
                    if prev_rs {
                        self.write_raw_data_byte(full);
                    } else {
                        self.execute_hd44780_command(full);
                    }
                } else {
                    self.pending_hi_nibble = Some((nibble, rs));
                }
            }
            self.prev_en = en;
        }
        Ok(())
    }

    fn read(&mut self, len: usize) -> Result<Vec<u8>, I2cError> {
        Ok(vec![if self.backlight { 0x08 } else { 0x00 }; len])
    }
}

/// 128×64 monochrome graphic OLED framebuffer (SSD1306-compatible I2C device, default `0x3C`).
#[derive(Debug, Clone)]
pub struct OledPixelFramebuffer {
    address: u8,
    width: usize,
    height: usize,
    /// Page-organized 1-bit GDDRAM (`(height / 8) * width` bytes).
    pages: Vec<u8>,
    display_on: bool,
    col_cursor: usize,
    page_cursor: usize,
}

impl OledPixelFramebuffer {
    /// Creates a cleared `128x64` SSD1306 OLED framebuffer on `address` (typically `0x3C`).
    #[must_use]
    pub fn ssd1306_128x64(address: u8) -> Self {
        Self {
            address,
            width: 128,
            height: 64,
            pages: vec![0u8; 128 * 8],
            display_on: true,
            col_cursor: 0,
            page_cursor: 0,
        }
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    #[must_use]
    pub const fn is_display_on(&self) -> bool {
        self.display_on
    }

    pub fn clear(&mut self) {
        self.pages.fill(0);
    }

    /// Sets or clears pixel `(x, y)` (out-of-bounds writes safely ignored).
    pub fn set_pixel(&mut self, x: usize, y: usize, on: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let page = y / 8;
        let bit = y % 8;
        let idx = page.saturating_mul(self.width).saturating_add(x);
        if let Some(slot) = self.pages.get_mut(idx) {
            if on {
                *slot |= 1u8 << bit;
            } else {
                *slot &= !(1u8 << bit);
            }
        }
    }

    /// Reads pixel `(x, y)`.
    #[must_use]
    pub fn get_pixel(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let page = y / 8;
        let bit = y % 8;
        let idx = page.saturating_mul(self.width).saturating_add(x);
        self.pages
            .get(idx)
            .is_some_and(|&byte| ((byte >> bit) & 1) != 0)
    }

    /// Draws an axis-aligned rectangle outline or filled box.
    pub fn draw_rect(&mut self, x: usize, y: usize, w: usize, h: usize, filled: bool, on: bool) {
        for dy in 0..h {
            for dx in 0..w {
                if filled || dx == 0 || dy == 0 || dx + 1 == w || dy + 1 == h {
                    self.set_pixel(x.saturating_add(dx), y.saturating_add(dy), on);
                }
            }
        }
    }

    /// Counts total lit pixels (convenient for rendering/framebuffer tests).
    #[must_use]
    pub fn lit_pixel_count(&self) -> usize {
        self.pages.iter().map(|b| b.count_ones() as usize).sum()
    }
}

impl I2cDevice for OledPixelFramebuffer {
    fn address_7bit(&self) -> u8 {
        self.address
    }

    fn write(&mut self, data: &[u8]) -> Result<(), I2cError> {
        let Some(&control) = data.first() else {
            return Ok(());
        };
        let payload = data.get(1..).unwrap_or(&[]);
        match control {
            // Command stream
            0x00 | 0x80 => {
                for &cmd in payload {
                    match cmd {
                        0xAE => self.display_on = false,
                        0xAF => self.display_on = true,
                        0xB0..=0xB7 => {
                            self.page_cursor = usize::from(cmd - 0xB0);
                        }
                        0x00..=0x0F => {
                            self.col_cursor = (self.col_cursor & 0xF0) | usize::from(cmd & 0x0F);
                        }
                        0x10..=0x1F => {
                            self.col_cursor =
                                (self.col_cursor & 0x0F) | (usize::from(cmd & 0x0F) << 4);
                        }
                        _ => {}
                    }
                }
            }
            // GDDRAM pixel data stream
            0x40 => {
                for &byte in payload {
                    let idx = self
                        .page_cursor
                        .saturating_mul(self.width)
                        .saturating_add(self.col_cursor % self.width);
                    if let Some(slot) = self.pages.get_mut(idx) {
                        *slot = byte;
                    }
                    self.col_cursor = (self.col_cursor + 1) % self.width;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn read(&mut self, len: usize) -> Result<Vec<u8>, I2cError> {
        let status = if self.display_on { 0x00 } else { 0x40 };
        Ok(vec![status; len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dendri_bus::{BusTopology, I2cBus, I2cEndpoint};

    #[test]
    fn def007_hd44780_rom_tables_validate_and_reject_unsupported_glyphs() {
        // ROM A00: degree symbol is 0xDF, Ohm is 0xF4, Portuguese 'ç' & emoji rejected
        assert_eq!(DisplayCharsetSpec::Hd44780A00.encode_char('°'), Some(0xDF));
        assert_eq!(DisplayCharsetSpec::Hd44780A00.encode_char('Ω'), Some(0xF4));
        assert_eq!(DisplayCharsetSpec::Hd44780A00.encode_char('ç'), None);
        assert_eq!(DisplayCharsetSpec::Hd44780A00.encode_char('🔥'), None);

        // ROM A02: European Latin-1 accents supported ('ç', 'ã', 'é', '°')
        assert_eq!(DisplayCharsetSpec::Hd44780A02.encode_char('ç'), Some(0xE7));
        assert_eq!(DisplayCharsetSpec::Hd44780A02.encode_char('ã'), Some(0xE3));
        assert_eq!(DisplayCharsetSpec::Hd44780A02.encode_char('°'), Some(0xB0));
        assert_eq!(DisplayCharsetSpec::Hd44780A02.encode_char('🔥'), None);

        // Strict ASCII rejects degree symbol
        assert_eq!(DisplayCharsetSpec::Ascii7Bit.encode_char('A'), Some(0x41));
        assert_eq!(DisplayCharsetSpec::Ascii7Bit.encode_char('°'), None);
    }

    #[test]
    fn lcd_and_oled_coexist_on_physical_i2c_bus_and_render_frames() {
        let mut topo = BusTopology::new();
        for dev in ["lcd1", "oled1"] {
            topo.connect("uno", "A4", dev, "SDA");
            topo.connect("uno", "A5", dev, "SCL");
        }
        topo.rebuild();

        let ctrl = I2cEndpoint::new("uno", "A4", "A5");
        let mut bus = I2cBus::with_pullups();
        bus.attach_device(
            I2cEndpoint::new("lcd1", "SDA", "SCL"),
            Box::new(LcdCharFramebuffer::hd44780_16x2(0x27)),
        )
        .unwrap();
        bus.attach_device(
            I2cEndpoint::new("oled1", "SDA", "SCL"),
            Box::new(OledPixelFramebuffer::ssd1306_128x64(0x3C)),
        )
        .unwrap();

        assert_eq!(bus.scan(&ctrl, &topo).unwrap(), vec![0x27, 0x3C]);

        // Send "25°C" over real PCF8574 4-bit I2C protocol to LCD 0x27
        let mut seq = Vec::new();
        seq.extend_from_slice(&encode_hd44780_i2c_byte(0x01, false, true)); // Clear
        for byte in [b'2', b'5', 0xDF, b'C'] {
            seq.extend_from_slice(&encode_hd44780_i2c_byte(byte, true, true));
        }
        bus.write(&ctrl, 0x27, &seq, &topo).unwrap();

        // Send 8-pixel solid vertical bar to OLED 0x3C at page 0, col 0
        bus.write(&ctrl, 0x3C, &[0x00, 0xAF, 0xB0, 0x00, 0x10], &topo)
            .unwrap();
        bus.write(&ctrl, 0x3C, &[0x40, 0xFF, 0xFF], &topo).unwrap();

        // Verify direct framebuffer print + CharsetViolation
        let mut lcd = LcdCharFramebuffer::hd44780_16x2(0x27);
        lcd.print_str("TEMP: 25°C\nOK").unwrap();
        assert_eq!(lcd.line_text(0).trim_end(), "TEMP: 25°C");
        assert_eq!(lcd.line_text(1).trim_end(), "OK");
        assert!(lcd.print_str("BAD🔥").is_err());

        let mut oled = OledPixelFramebuffer::ssd1306_128x64(0x3C);
        oled.draw_rect(0, 0, 4, 4, true, true);
        assert_eq!(oled.lit_pixel_count(), 16);
        assert!(oled.get_pixel(3, 3));
        assert!(!oled.get_pixel(4, 4));
    }
}
