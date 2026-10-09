//! SH7305 timer units (TMU, ETMU) and real-time clock.

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

pub const CPU_HZ: u64 = 29_491_200;
pub const PERIPH_HZ: u64 = CPU_HZ / 2;
pub const RTC_HZ: u64 = 32_768;

// --- TMU: 3 channels at 0xA4490004, counting peripheral clock / prescaler ---

#[derive(Default, Clone, Copy)]
pub struct TmuChan {
    pub tcor: u32,
    pub tcnt: u32,
    pub tcr: u16,
    acc: u64,
}

#[derive(Default)]
pub struct Tmu {
    pub tstr: u8,
    pub ch: [TmuChan; 3],
}

impl Tmu {
    fn div(tcr: u16) -> u64 {
        match tcr & 7 {
            0 => 4,
            1 => 16,
            2 => 64,
            3 => 256,
            _ => 1024,
        }
    }

    /// Advance by `pclk` peripheral clock cycles. Returns true if a flag changed.
    pub fn advance(&mut self, pclk: u64) -> bool {
        let mut changed = false;
        for i in 0..3 {
            if self.tstr & (1 << i) == 0 {
                continue;
            }
            let c = &mut self.ch[i];
            c.acc += pclk;
            let d = Self::div(c.tcr);
            let mut ticks = c.acc / d;
            c.acc %= d;
            while ticks > 0 {
                if ticks <= c.tcnt as u64 {
                    c.tcnt -= ticks as u32;
                    break;
                }
                ticks -= c.tcnt as u64 + 1;
                c.tcnt = c.tcor;
                if c.tcr & 0x100 == 0 {
                    c.tcr |= 0x100;
                    changed = true;
                }
                if c.tcor == 0 {
                    break;
                }
                ticks %= c.tcor as u64 + 1;
            }
        }
        changed
    }

    pub fn read(&self, off: u32, size: u32) -> u32 {
        match off {
            0x04 => self.tstr as u32,
            o if (0x08..0x2C).contains(&o) => {
                let i = ((o - 0x08) / 12) as usize;
                let r = (o - 0x08) % 12;
                let c = &self.ch[i];
                match (r, size) {
                    (0, 4) => c.tcor,
                    (4, 4) => c.tcnt,
                    (8, 2) | (8, 4) => c.tcr as u32,
                    (9, 1) => c.tcr as u32 & 0xFF,
                    (8, 1) => (c.tcr >> 8) as u32,
                    _ => 0,
                }
            }
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, size: u32, v: u32) {
        match off {
            0x04 => self.tstr = v as u8,
            o if (0x08..0x2C).contains(&o) => {
                let i = ((o - 0x08) / 12) as usize;
                let r = (o - 0x08) % 12;
                let c = &mut self.ch[i];
                match (r, size) {
                    (0, 4) => c.tcor = v,
                    (4, 4) => c.tcnt = v,
                    (8, _) => {
                        let v = v as u16;
                        // UNF can only be cleared by writing 0.
                        let unf = c.tcr & v & 0x100;
                        c.tcr = (v & !0x100) | unf;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn pending(&self, i: usize) -> bool {
        let t = self.ch[i].tcr;
        t & 0x100 != 0 && t & 0x20 != 0
    }
}

// --- ETMU: 6 channels at 0xA44D0030 (stride 0x20), counting at 32768 Hz ---

#[derive(Default, Clone, Copy)]
pub struct Etmu {
    pub tstr: u8,
    pub tcor: u32,
    pub tcnt: u32,
    pub tcr: u8,
}

impl Etmu {
    pub fn advance(&mut self, ticks: u64) -> bool {
        if self.tstr & 1 == 0 {
            return false;
        }
        let mut changed = false;
        let mut ticks = ticks;
        while ticks > 0 {
            if ticks <= self.tcnt as u64 {
                self.tcnt -= ticks as u32;
                break;
            }
            ticks -= self.tcnt as u64 + 1;
            self.tcnt = self.tcor;
            if self.tcr & 2 == 0 {
                self.tcr |= 2;
                changed = true;
            }
            if self.tcor == 0 {
                break;
            }
            ticks %= self.tcor as u64 + 1;
        }
        changed
    }

    pub fn read(&self, r: u32) -> u32 {
        match r {
            0x0 => self.tstr as u32,
            0x4 => self.tcor,
            0x8 => self.tcnt,
            0xC => self.tcr as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, r: u32, v: u32) {
        match r {
            0x0 => self.tstr = v as u8,
            0x4 => self.tcor = v,
            0x8 => self.tcnt = v,
            0xC => {
                let unf = self.tcr & v as u8 & 2;
                self.tcr = (v as u8 & !2) | unf;
            }
            _ => {}
        }
    }

    pub fn pending(&self) -> bool {
        self.tcr & 3 == 3
    }
}

// --- RTC at 0xA413FEC0 ---

pub struct Rtc {
    pub r64: u8,
    pub sec: u8,
    pub min: u8,
    pub hour: u8,
    pub wday: u8,
    pub day: u8,
    pub mon: u8,
    pub year: u16,
    pub alarm: [u8; 12],
    pub rcr1: u8,
    pub rcr2: u8,
    acc: u64,
    pri_count: u32,
}

fn bcd(v: u32) -> u32 {
    let mut out = 0;
    let mut v = v;
    let mut shift = 0;
    while v > 0 || shift == 0 {
        out |= (v % 10) << shift;
        v /= 10;
        shift += 4;
    }
    out
}

fn unbcd(v: u32) -> u32 {
    let mut out = 0;
    let mut mul = 1;
    let mut v = v;
    while v > 0 {
        out += (v & 0xF) * mul;
        v >>= 4;
        mul *= 10;
    }
    out
}

fn days_in_month(m: u8, y: u16) -> u8 {
    match m {
        2 => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 },
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Rtc {
    pub fn new() -> Self {
        // There is no system clock in WebAssembly; the web app sets the time itself.
        #[cfg(target_arch = "wasm32")]
        let secs = 0;
        #[cfg(not(target_arch = "wasm32"))]
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Self::from_unix(secs)
    }

    /// Clock set to the given Unix time (UTC).
    pub fn from_unix(secs: u64) -> Self {
        let days = (secs / 86400) as i64;
        let rem = secs % 86400;
        // Civil-from-days (Howard Hinnant's algorithm), UTC.
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as u16;
        Rtc {
            r64: 0,
            sec: (rem % 60) as u8,
            min: ((rem / 60) % 60) as u8,
            hour: (rem / 3600) as u8,
            wday: ((days + 4).rem_euclid(7)) as u8,
            day: d,
            mon: m,
            year: y,
            alarm: [0; 12],
            rcr1: 0,
            rcr2: 0x01,
            acc: 0,
            pri_count: 0,
        }
    }

    fn tick_second(&mut self) {
        self.sec += 1;
        if self.sec < 60 {
            return;
        }
        self.sec = 0;
        self.min += 1;
        if self.min < 60 {
            return;
        }
        self.min = 0;
        self.hour += 1;
        if self.hour < 24 {
            return;
        }
        self.hour = 0;
        self.wday = (self.wday + 1) % 7;
        self.day += 1;
        if self.day <= days_in_month(self.mon, self.year) {
            return;
        }
        self.day = 1;
        self.mon += 1;
        if self.mon <= 12 {
            return;
        }
        self.mon = 1;
        self.year += 1;
    }

    /// Advance by CPU cycles. Returns true if an interrupt flag was raised.
    pub fn advance(&mut self, cpu_cycles: u64) -> bool {
        if self.rcr2 & 1 == 0 {
            return false;
        }
        let mut changed = false;
        self.acc += cpu_cycles * 64;
        while self.acc >= CPU_HZ {
            self.acc -= CPU_HZ;
            self.r64 = (self.r64 + 1) & 0x7F;
            if self.r64 & 0x3F == 0 {
                self.tick_second();
                if self.rcr1 & 0x80 == 0 {
                    self.rcr1 |= 0x80; // carry flag
                    changed |= self.rcr1 & 0x10 != 0;
                }
            }
            let period = match (self.rcr2 >> 4) & 7 {
                0 => 0,
                1 | 2 => 1,
                3 => 4,
                4 => 16,
                5 => 32,
                6 => 64,
                _ => 128,
            };
            if period > 0 {
                self.pri_count += 1;
                if self.pri_count >= period {
                    self.pri_count = 0;
                    if self.rcr2 & 0x80 == 0 {
                        self.rcr2 |= 0x80;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    pub fn read(&self, off: u32) -> u32 {
        match off {
            0x00 => self.r64 as u32,
            0x02 => bcd(self.sec as u32),
            0x04 => bcd(self.min as u32),
            0x06 => bcd(self.hour as u32),
            0x08 => self.wday as u32,
            0x0A => bcd(self.day as u32),
            0x0C => bcd(self.mon as u32),
            0x0E => bcd(self.year as u32),
            0x10..=0x1B => self.alarm[(off - 0x10) as usize] as u32,
            0x1C => self.rcr1 as u32,
            0x1E => self.rcr2 as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x02 => self.sec = unbcd(v & 0x7F) as u8,
            0x04 => self.min = unbcd(v & 0x7F) as u8,
            0x06 => self.hour = unbcd(v & 0x3F) as u8,
            0x08 => self.wday = (v & 7) as u8,
            0x0A => self.day = unbcd(v & 0x3F) as u8,
            0x0C => self.mon = unbcd(v & 0x1F) as u8,
            0x0E => self.year = unbcd(v & 0xFFFF) as u16,
            0x10..=0x1B => self.alarm[(off - 0x10) as usize] = v as u8,
            0x1C => {
                // CF and AF are cleared by writing 0.
                let keep = self.rcr1 & v as u8 & 0x81;
                self.rcr1 = (v as u8 & !0x81) | keep;
            }
            0x1E => {
                let v = v as u8;
                if v & 2 != 0 {
                    self.r64 = 0;
                    self.acc = 0;
                }
                let pef = self.rcr2 & v & 0x80;
                self.rcr2 = (v & !0x82) | pef;
            }
            _ => {}
        }
    }

    pub fn pri_pending(&self) -> bool {
        self.rcr2 & 0x80 != 0 && (self.rcr2 >> 4) & 7 != 0
    }
    pub fn cui_pending(&self) -> bool {
        self.rcr1 & 0x90 == 0x90
    }
    pub fn ati_pending(&self) -> bool {
        self.rcr1 & 0x09 == 0x09
    }
}

// --- CMT: compare match timer at 0xA44A0000 (CMSTR), CMCSR +0x60, CMCNT +0x64, CMCOR +0x68 ---

#[derive(Default)]
pub struct Cmt {
    pub cmstr: u16,
    pub cmcsr: u16,
    pub cmcnt: u32,
    pub cmcor: u32,
    acc: u64,
}

impl Cmt {
    /// Counter clock. The bootloader programs CMCOR = ms * 1475 / 32 with CKS=5,
    /// i.e. ~46 kHz; use that for every clock selection.
    const DIV: u64 = 320;

    pub fn advance(&mut self, pclk: u64) -> bool {
        if self.cmstr & 0x20 == 0 {
            return false;
        }
        self.acc += pclk;
        let ticks = self.acc / Self::DIV;
        self.acc %= Self::DIV;
        if ticks == 0 {
            return false;
        }
        let next = self.cmcnt as u64 + ticks;
        if next >= self.cmcor as u64 {
            self.cmcnt = if self.cmcor == 0 { 0 } else { ((next - self.cmcor as u64) % (self.cmcor as u64 + 1)) as u32 };
            let was = self.cmcsr & 0x8000 != 0;
            self.cmcsr |= 0x8000;
            return !was;
        }
        self.cmcnt = next as u32;
        false
    }

    pub fn read(&self, off: u32) -> u32 {
        match off {
            0x00 => self.cmstr as u32,
            0x60 => self.cmcsr as u32,
            0x64 => self.cmcnt,
            0x68 => self.cmcor,
            _ => 0,
        }
    }

    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x00 => self.cmstr = v as u16,
            0x60 => {
                // CMF/OVF are cleared by writing 0; WRFLG (bit 13) always reads 0.
                let flags = self.cmcsr & v as u16 & 0xC000;
                self.cmcsr = (v as u16 & !0xE000) | flags;
            }
            0x64 => self.cmcnt = v,
            0x68 => self.cmcor = v,
            _ => {}
        }
    }

    pub fn pending(&self) -> bool {
        // CMR bits (5..4) select the interrupt request; treat any as enabled.
        self.cmcsr & 0x8000 != 0 && self.cmcsr & 0x0030 != 0
    }
}

// --- snapshots ---

use crate::state::{Bad, Snap, R, W};

impl Snap for Tmu {
    fn save(&self, w: &mut W) {
        w.u8(self.tstr);
        for c in &self.ch {
            w.u32(c.tcor);
            w.u32(c.tcnt);
            w.u16(c.tcr);
            w.u64(c.acc);
        }
    }
    fn load(&mut self, r: &mut R) -> Result<(), Bad> {
        self.tstr = r.u8()?;
        for c in self.ch.iter_mut() {
            c.tcor = r.u32()?;
            c.tcnt = r.u32()?;
            c.tcr = r.u16()?;
            c.acc = r.u64()?;
        }
        Ok(())
    }
}

impl Snap for Etmu {
    fn save(&self, w: &mut W) {
        w.u8(self.tstr);
        w.u32(self.tcor);
        w.u32(self.tcnt);
        w.u8(self.tcr);
    }
    fn load(&mut self, r: &mut R) -> Result<(), Bad> {
        self.tstr = r.u8()?;
        self.tcor = r.u32()?;
        self.tcnt = r.u32()?;
        self.tcr = r.u8()?;
        Ok(())
    }
}

impl Snap for Cmt {
    fn save(&self, w: &mut W) {
        w.u16(self.cmstr);
        w.u16(self.cmcsr);
        w.u32(self.cmcnt);
        w.u32(self.cmcor);
        w.u64(self.acc);
    }
    fn load(&mut self, r: &mut R) -> Result<(), Bad> {
        self.cmstr = r.u16()?;
        self.cmcsr = r.u16()?;
        self.cmcnt = r.u32()?;
        self.cmcor = r.u32()?;
        self.acc = r.u64()?;
        Ok(())
    }
}

impl Snap for Rtc {
    fn save(&self, w: &mut W) {
        for v in [self.r64, self.sec, self.min, self.hour, self.wday, self.day, self.mon] {
            w.u8(v);
        }
        w.u16(self.year);
        w.0.extend_from_slice(&self.alarm);
        w.u8(self.rcr1);
        w.u8(self.rcr2);
        w.u64(self.acc);
        w.u32(self.pri_count);
    }
    fn load(&mut self, r: &mut R) -> Result<(), Bad> {
        self.r64 = r.u8()?;
        self.sec = r.u8()?;
        self.min = r.u8()?;
        self.hour = r.u8()?;
        self.wday = r.u8()?;
        self.day = r.u8()?;
        self.mon = r.u8()?;
        self.year = r.u16()?;
        for a in self.alarm.iter_mut() {
            *a = r.u8()?;
        }
        self.rcr1 = r.u8()?;
        self.rcr2 = r.u8()?;
        self.acc = r.u64()?;
        self.pri_count = r.u32()?;
        Ok(())
    }
}

impl Rtc {
    /// Let the clock catch up with time that passed while the emulator was
    /// closed, like the real RTC which keeps running while the calculator is off.
    pub fn add_seconds(&mut self, secs: u64) {
        for _ in 0..secs.min(40 * 366 * 86400) {
            self.tick_second();
        }
    }
}
