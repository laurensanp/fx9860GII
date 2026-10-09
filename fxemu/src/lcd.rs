//! Toshiba T6K11-compatible 128x64 monochrome LCD controller.
//!
//! Mapped at 0xB4000000 (RS=0: register select / status) and 0xB4010000
//! (RS=1: register data). Protocol per gint's t6k11 driver: register 4 sets
//! the X address (row, when bit 7 is set) or the Y address (column byte),
//! register 1 the auto-increment mode, register 7 transfers VRAM data.
//! The "version 2" protocol (registers 8 and 10) is also accepted.

pub const ROWS: usize = 64;
pub const COLS: usize = 16; // bytes per row (128 pixels)

pub struct Lcd {
    pub vram: [u8; ROWS * 32],
    reg: u8,
    x: usize,
    y: usize,
    counter: u8,
    pub zaddr: u8,
    pub contrast: u8,
    pub display_ctrl: u8,
    pub writes: u64,
    pub log: bool,
}

impl Lcd {
    pub fn new() -> Self {
        Lcd {
            vram: [0; ROWS * 32],
            reg: 0,
            x: 0,
            y: 0,
            counter: 1,
            zaddr: 0,
            contrast: 0xA0,
            display_ctrl: 0,
            writes: 0,
            log: false,
        }
    }

    fn advance(&mut self) {
        match self.counter & 3 {
            0 => self.y = (self.y + 31) % 32,
            1 => self.y = (self.y + 1) % 32,
            2 => self.x = (self.x + ROWS - 1) % ROWS,
            _ => self.x = (self.x + 1) % ROWS,
        }
    }

    pub fn write_sel(&mut self, v: u8) {
        self.reg = v;
    }

    pub fn read_status(&self) -> u8 {
        self.counter & 3 | 0x04
    }

    pub fn write_data(&mut self, v: u8) {
        match self.reg {
            0 => self.display_ctrl = v,
            1 => self.counter = v & 3,
            4 => {
                if v & 0x80 != 0 {
                    self.x = (v & 0x3F) as usize;
                } else {
                    self.y = (v & 0x1F) as usize;
                }
            }
            5 => self.zaddr = v & 0x3F,
            6 => self.contrast = v,
            7 | 10 => {
                self.vram[self.x * 32 + self.y] = v;
                self.writes += 1;
                if self.reg == 10 {
                    self.y = (self.y + 1) % 32;
                } else {
                    self.advance();
                }
            }
            8 => {
                if v & 0x80 != 0 {
                    self.x = (v & 0x3F) as usize;
                    self.y = 0;
                }
            }
            _ => {
                if self.log {
                    eprintln!("[lcd] reg {} <- {:02x}", self.reg, v);
                }
            }
        }
    }

    pub fn read_data(&mut self) -> u8 {
        if self.reg == 7 {
            let v = self.vram[self.x * 32 + self.y];
            self.advance();
            v
        } else {
            0
        }
    }

    /// The visible 128x64 frame, 16 bytes per row, MSB = leftmost pixel.
    pub fn frame(&self) -> Vec<u8> {
        let mut out = vec![0u8; ROWS * COLS];
        for r in 0..ROWS {
            let src = (r + self.zaddr as usize) % ROWS;
            out[r * COLS..(r + 1) * COLS].copy_from_slice(&self.vram[src * 32..src * 32 + COLS]);
        }
        out
    }
}

// --- snapshots ---

use crate::state::{Bad, Snap, R, W};

impl Snap for Lcd {
    fn save(&self, w: &mut W) {
        w.bytes(&self.vram);
        for v in [self.reg, self.x as u8, self.y as u8, self.counter, self.zaddr, self.contrast, self.display_ctrl] {
            w.u8(v);
        }
    }
    fn load(&mut self, r: &mut R) -> Result<(), Bad> {
        r.bytes_into(&mut self.vram)?;
        self.reg = r.u8()?;
        self.x = r.u8()? as usize % ROWS;
        self.y = r.u8()? as usize % 32;
        self.counter = r.u8()?;
        self.zaddr = r.u8()?;
        self.contrast = r.u8()?;
        self.display_ctrl = r.u8()?;
        Ok(())
    }
}
