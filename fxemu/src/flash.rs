//! 4 MiB NOR flash (AMD/Spansion command set, 16-bit bus), holding the
//! bootloader, the OS, the main-memory backup and the storage memory.
//!
//! Program and erase operations complete instantly, so status polling
//! (DQ7 data polling or DQ6 toggle) sees the final data immediately.

pub const FLASH_SIZE: usize = 4 << 20;

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Read,
    Unlock1,
    Unlock2,
    Program,
    EraseSetup,
    Erase1,
    Erase2,
    Autoselect,
    Cfi,
    Bypass,
    BypassProgram,
    BypassReset,
}

pub struct Flash {
    pub data: Vec<u8>,
    state: State,
    pub dirty: bool,
    pub log: bool,
    pub writes: u64,
}

impl Flash {
    pub fn new(mut data: Vec<u8>) -> Self {
        data.resize(FLASH_SIZE, 0xFF);
        Flash { data, state: State::Read, dirty: false, log: false, writes: 0 }
    }

    #[inline]
    fn off(a: u32) -> usize {
        (a as usize) & (FLASH_SIZE - 1)
    }

    fn special_read(&self, off: usize) -> Option<u16> {
        match self.state {
            State::Autoselect => {
                let w = (off >> 1) & 0xFF;
                Some(match w {
                    0x00 => 0x0001, // Spansion / AMD
                    0x01 => 0x227E, // S29JL032H-style device ID
                    0x02 => 0x0000, // sector protection: unprotected
                    0x0E => 0x220A,
                    0x0F => 0x2200,
                    _ => 0x0000,
                })
            }
            State::Cfi => {
                let w = (off >> 1) & 0xFF;
                let q = [b'Q', b'R', b'Y'];
                Some(match w {
                    0x10..=0x12 => q[w - 0x10] as u16,
                    0x13 => 0x0002,
                    0x27 => 0x0016, // 2^22 = 4 MiB
                    _ => 0x0000,
                })
            }
            _ => None,
        }
    }

    #[inline]
    pub fn read8(&self, a: u32) -> u8 {
        let off = Self::off(a);
        if self.state == State::Read {
            return self.data[off];
        }
        match self.special_read(off & !1) {
            Some(v) => if off & 1 == 0 { (v >> 8) as u8 } else { v as u8 },
            None => self.data[off],
        }
    }

    #[inline]
    pub fn read16(&self, a: u32) -> u16 {
        let off = Self::off(a) & !1;
        if self.state != State::Read {
            if let Some(v) = self.special_read(off) {
                return v;
            }
        }
        u16::from_be_bytes([self.data[off], self.data[off + 1]])
    }

    #[inline]
    pub fn read32(&self, a: u32) -> u32 {
        (self.read16(a) as u32) << 16 | self.read16(a.wrapping_add(2)) as u32
    }

    fn sector_range(off: usize) -> (usize, usize) {
        // Bottom-boot layout: 8 x 8 KiB sectors, then 64 KiB sectors.
        if off < 0x10000 {
            let s = off & !0x1FFF;
            (s, 0x2000)
        } else {
            (off & !0xFFFF, 0x10000)
        }
    }

    fn program(&mut self, off: usize, v: u16) {
        let off = off & !1;
        // Programming can only clear bits.
        self.data[off] &= (v >> 8) as u8;
        self.data[off + 1] &= v as u8;
        self.dirty = true;
        self.writes += 1;
    }

    pub fn write16(&mut self, a: u32, v: u16) {
        let off = Self::off(a);
        let cmd = (off >> 1) & 0x7FF;
        let b = (v & 0xFF) as u8;
        let old = self.state;
        self.state = match self.state {
            State::Read | State::Autoselect | State::Cfi => {
                if b == 0xAA && cmd == 0x555 {
                    State::Unlock1
                } else if b == 0x98 && cmd == 0x55 {
                    State::Cfi
                } else if b == 0xF0 || b == 0xFF {
                    State::Read
                } else {
                    if self.log {
                        eprintln!("[flash] stray write {:04x} @ {:06x} in {:?}", v, off, self.state);
                    }
                    self.state
                }
            }
            State::Unlock1 => {
                if b == 0x55 && cmd == 0x2AA { State::Unlock2 } else { State::Read }
            }
            State::Unlock2 => match (cmd, b) {
                (0x555, 0xA0) => State::Program,
                (0x555, 0x80) => State::EraseSetup,
                (0x555, 0x90) => State::Autoselect,
                (0x555, 0x20) => State::Bypass,
                _ => State::Read,
            },
            State::Program => {
                self.program(off, v);
                State::Read
            }
            State::EraseSetup => {
                if b == 0xAA && cmd == 0x555 { State::Erase1 } else { State::Read }
            }
            State::Erase1 => {
                if b == 0x55 && cmd == 0x2AA { State::Erase2 } else { State::Read }
            }
            State::Erase2 => {
                if b == 0x30 {
                    let (s, len) = Self::sector_range(off);
                    if self.log {
                        eprintln!("[flash] erase sector {:06x}+{:x}", s, len);
                    }
                    self.data[s..s + len].fill(0xFF);
                    self.dirty = true;
                } else if b == 0x10 && cmd == 0x555 {
                    eprintln!("[flash] chip erase requested -- ignored to protect the image");
                }
                State::Read
            }
            State::Bypass => match b {
                0xA0 => State::BypassProgram,
                0x90 => State::BypassReset,
                _ => State::Bypass,
            },
            State::BypassProgram => {
                self.program(off, v);
                State::Bypass
            }
            State::BypassReset => State::Read,
        };
        if self.log && old != self.state && matches!(self.state, State::Autoselect | State::Cfi) {
            eprintln!("[flash] entering {:?}", self.state);
        }
    }

    pub fn write8(&mut self, a: u32, v: u8) {
        self.write16(a, v as u16);
    }

    pub fn write32(&mut self, a: u32, v: u32) {
        self.write16(a, (v >> 16) as u16);
        self.write16(a.wrapping_add(2), v as u16);
    }
}
