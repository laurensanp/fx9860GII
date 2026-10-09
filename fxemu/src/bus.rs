//! Physical memory map and on-chip peripherals of the SH7305 as used in the
//! fx-9860GII-2.
//!
//! Addresses handed to the bus are either 29-bit physical addresses
//! (< 0x2000_0000) or P4 addresses (>= 0xE000_0000).

use std::collections::{HashMap, HashSet};

use crate::flash::Flash;
use crate::lcd::Lcd;
use crate::timers::{Cmt, Etmu, Rtc, Tmu, CPU_HZ, PERIPH_HZ, RTC_HZ};

pub const RAM_SIZE: usize = 512 << 10;

#[derive(Default)]
pub struct Mmu {
    pub pteh: u32,
    pub ptel: u32,
    pub ttb: u32,
    pub tea: u32,
    pub mmucr: u32,
    pub ccr: u32,
    pub tra: u32,
    pub expevt: u32,
    pub intevt: u32,
    pub ptea: u32,
}

/// One interrupt source: event code, IPR register/nibble, IMR register/bit.
struct Source {
    name: &'static str,
    code: u32,
    ipr: usize,
    shift: u32,
    imr: usize,
    bit: u8,
}

const SOURCES: &[Source] = &[
    Source { name: "TMU0", code: 0x400, ipr: 0, shift: 12, imr: 4, bit: 0x10 },
    Source { name: "TMU1", code: 0x420, ipr: 0, shift: 8, imr: 4, bit: 0x20 },
    Source { name: "TMU2", code: 0x440, ipr: 0, shift: 4, imr: 4, bit: 0x40 },
    Source { name: "ETMU0", code: 0x9E0, ipr: 9, shift: 12, imr: 6, bit: 0x08 },
    Source { name: "ETMU1", code: 0xC20, ipr: 6, shift: 8, imr: 5, bit: 0x02 },
    Source { name: "ETMU2", code: 0xC40, ipr: 6, shift: 4, imr: 5, bit: 0x04 },
    Source { name: "ETMU3", code: 0x900, ipr: 4, shift: 4, imr: 2, bit: 0x01 },
    Source { name: "ETMU4", code: 0xD00, ipr: 8, shift: 12, imr: 6, bit: 0x10 },
    Source { name: "ETMU5", code: 0xFA0, ipr: 11, shift: 12, imr: 8, bit: 0x02 },
    Source { name: "RTC_ATI", code: 0xA80, ipr: 10, shift: 12, imr: 10, bit: 0x04 },
    Source { name: "RTC_PRI", code: 0xAA0, ipr: 10, shift: 12, imr: 10, bit: 0x02 },
    Source { name: "RTC_CUI", code: 0xAC0, ipr: 10, shift: 12, imr: 10, bit: 0x01 },
    Source { name: "KEYSC", code: 0xBE0, ipr: 5, shift: 12, imr: 5, bit: 0x80 },
    Source { name: "CMT", code: 0xF00, ipr: 9, shift: 4, imr: 6, bit: 0x20 },
    Source { name: "DEI0", code: 0x800, ipr: 4, shift: 12, imr: 1, bit: 0x01 },
    Source { name: "DEI1", code: 0x820, ipr: 4, shift: 12, imr: 1, bit: 0x02 },
    Source { name: "DEI2", code: 0x840, ipr: 4, shift: 12, imr: 1, bit: 0x04 },
    Source { name: "DEI3", code: 0x860, ipr: 4, shift: 12, imr: 1, bit: 0x08 },
    Source { name: "DEI4", code: 0xB80, ipr: 5, shift: 8, imr: 5, bit: 0x10 },
    Source { name: "DEI5", code: 0xBA0, ipr: 5, shift: 8, imr: 5, bit: 0x20 },
];

#[derive(Default, Clone, Copy)]
pub struct DmaChan {
    pub sar: u32,
    pub dar: u32,
    pub tcr: u32,
    pub chcr: u32,
}

#[derive(Default)]
pub struct Dma {
    pub ch: [DmaChan; 6],
    pub dmaor: u16,
}

impl Dma {
    /// Channel index and register offset for an address in 0xFE008020..0xFE00808F.
    fn decode(a: u32) -> Option<(usize, u32)> {
        let o = a - 0xFE00_8020;
        match o {
            0x00..=0x3F => Some(((o / 0x10) as usize, o % 0x10)),
            0x50..=0x6F => Some((4 + ((o - 0x50) / 0x10) as usize, (o - 0x50) % 0x10)),
            _ => None,
        }
    }
}

/// BCD arithmetic unit at 0xA4CB0010 (CASIO-specific): 8-digit packed BCD
/// add/subtract with an internal carry, chained by the OS over 24-digit
/// mantissas. Command (0x10): bit 0 = add (clear = subtract a - b), bit 1 =
/// use the previous carry/borrow, bit 2 = carry/borrow-in of 1.
#[derive(Default)]
pub struct BcdUnit {
    pub a: u32,
    pub b: u32,
    pub result: u32,
    pub carry: bool,
}

impl BcdUnit {
    fn run(&mut self, cmd: u32) {
        let sub = cmd & 1 == 0;
        // Carry-in: bit 1 = previous carry/borrow, bit 2 = forced 1.
        let mut c = if cmd & 2 != 0 { self.carry as i32 } else { (cmd & 4 != 0) as i32 };
        let mut out = 0u32;
        for i in 0..8 {
            let x = ((self.a >> (4 * i)) & 0xF) as i32;
            let y = ((self.b >> (4 * i)) & 0xF) as i32;
            let mut d = if sub { x - y - c } else { x + y + c };
            if sub {
                c = (d < 0) as i32;
                if d < 0 {
                    d += 10;
                }
            } else {
                c = (d >= 10) as i32;
                if d >= 10 {
                    d -= 10;
                }
            }
            out |= (d as u32 & 0xF) << (4 * i);
        }
        self.result = out;
        self.carry = c != 0;
    }
}

pub struct Intc {
    pub ipr: [u16; 12],
    pub imr: [u8; 13],
}

pub struct Bus {
    pub flash: Flash,
    pub ram: Vec<u8>,
    pub ilram: Vec<u8>,
    pub xram: Vec<u8>,
    pub yram: Vec<u8>,
    pub rsram: Vec<u8>,
    pub mmu: Mmu,
    pub intc: Intc,
    pub tmu: Tmu,
    pub etmu: [Etmu; 6],
    pub rtc: Rtc,
    pub cmt: Cmt,
    pub dma: Dma,
    pub bcd: BcdUnit,
    /// A/D converter at 0xA4610080 (battery voltage). Status register at +0x08.
    pub adcsr: u16,
    pub adc_value: u16,
    pub keys: [u8; 12],
    pub keysc_irq: bool,
    /// KEYSC status register 0xA44B0014: high byte = mode, low byte = event
    /// flags (bit 3 key data changed, bits 5/6 release events), write-1-to-clear.
    pub kysr_hi: u8,
    pub kysr_flags: u8,
    /// Set when the OS reads the key matrix; cleared when a new key goes down.
    pub keys_seen: bool,
    pub tlb_flush_request: bool,
    pub lcd: Lcd,
    regs: HashMap<u32, u8>,
    pub irq_line: Option<(u32, u32)>,
    pub irq_dirty: bool,
    pub next_tick: u64,
    last_tick: u64,
    rtc_acc: u64,
    pub trace_io: bool,
    pub trace_exc: bool,
    pub trace_irq: bool,
    /// Log every peripheral access (set from main after --trace-all-after).
    pub trace_all: u32,
    seen: HashSet<u32>,
    pub unknown_log: Vec<String>,
}

impl Bus {
    pub fn new(flash: Flash) -> Self {
        // Power-on values of input pins that are not otherwise modelled.
        let mut regs = HashMap::new();
        // Port data register: bit 0 high = normal boot (low requests OS update mode).
        regs.insert(0xA405_013A, 0x01);
        Bus {
            flash,
            ram: vec![0; RAM_SIZE],
            ilram: vec![0; 4 << 10],
            xram: vec![0; 8 << 10],
            yram: vec![0; 8 << 10],
            rsram: vec![0; 16 << 10],
            mmu: Mmu::default(),
            intc: Intc { ipr: [0; 12], imr: [0xFF; 13] },
            tmu: Tmu::default(),
            etmu: [Etmu::default(); 6],
            rtc: Rtc::new(),
            cmt: Cmt::default(),
            dma: Dma::default(),
            bcd: BcdUnit::default(),
            adcsr: 0,
            adc_value: 0x300,
            keys: [0; 12],
            keysc_irq: false,
            kysr_hi: 0,
            kysr_flags: 0,
            keys_seen: true,
            tlb_flush_request: false,
            lcd: Lcd::new(),
            regs,
            irq_line: None,
            irq_dirty: true,
            next_tick: 0,
            last_tick: 0,
            rtc_acc: 0,
            trace_io: false,
            trace_exc: false,
            trace_irq: false,
            trace_all: 0,
            seen: HashSet::new(),
            unknown_log: Vec::new(),
        }
    }

    // --- timing ---

    pub const TICK: u64 = 512;

    pub fn tick(&mut self, now: u64) {
        let elapsed = now - self.last_tick;
        self.last_tick = now;
        self.next_tick = now + Self::TICK;
        let pclk = elapsed * PERIPH_HZ / CPU_HZ;
        let mut changed = self.tmu.advance(pclk);
        changed |= self.cmt.advance(pclk);
        self.rtc_acc += elapsed * RTC_HZ;
        let rtc_ticks = self.rtc_acc / CPU_HZ;
        self.rtc_acc %= CPU_HZ;
        if rtc_ticks > 0 {
            for e in self.etmu.iter_mut() {
                changed |= e.advance(rtc_ticks);
            }
        }
        changed |= self.rtc.advance(elapsed);
        if changed {
            self.irq_dirty = true;
        }
    }

    // --- interrupts ---

    fn source_pending(&self, i: usize) -> bool {
        match i {
            0..=2 => self.tmu.pending(i),
            3..=8 => self.etmu[i - 3].pending(),
            9 => self.rtc.ati_pending(),
            10 => self.rtc.pri_pending(),
            11 => self.rtc.cui_pending(),
            12 => self.keysc_irq,
            13 => self.cmt.pending(),
            14..=19 => {
                let c = self.dma.ch[i - 14].chcr;
                c & 6 == 6
            }
            _ => false,
        }
    }

    pub fn update_irq(&mut self) {
        self.irq_dirty = false;
        let mut best: Option<(u32, u32)> = None;
        for (i, s) in SOURCES.iter().enumerate() {
            if !self.source_pending(i) {
                continue;
            }
            let level = ((self.intc.ipr[s.ipr] >> s.shift) & 0xF) as u32;
            let masked = self.intc.imr[s.imr] & s.bit != 0;
            if level == 0 || masked {
                continue;
            }
            if best.map_or(true, |(l, _)| level > l) {
                best = Some((level, s.code));
            }
        }
        if self.trace_irq && best != self.irq_line {
            if let Some((l, c)) = best {
                let name = SOURCES.iter().find(|s| s.code == c).map(|s| s.name).unwrap_or("?");
                eprintln!("[intc] line -> {} (code {:03x}, level {})", name, c, l);
            }
        }
        self.irq_line = best;
    }

    fn intc_read(&self, off: u32) -> u32 {
        match off {
            0x00..=0x2F if off % 4 == 0 => self.intc.ipr[(off / 4) as usize] as u32,
            0x80..=0xB3 if off % 4 == 0 => self.intc.imr[((off - 0x80) / 4) as usize] as u32,
            _ => 0,
        }
    }

    fn intc_write(&mut self, off: u32, v: u32) {
        match off {
            0x00..=0x2F if off % 4 == 0 => self.intc.ipr[(off / 4) as usize] = v as u16,
            0x80..=0xB3 if off % 4 == 0 => self.intc.imr[((off - 0x80) / 4) as usize] |= v as u8,
            0xC0..=0xF3 if off % 4 == 0 => self.intc.imr[((off - 0xC0) / 4) as usize] &= !(v as u8),
            _ => {}
        }
        self.irq_dirty = true;
    }

    // --- logging of accesses to unmodelled registers ---

    fn note(&mut self, kind: &str, a: u32, size: u32, v: u32, pc: u32) {
        if self.seen.insert(a) {
            let s = format!("{} {:08x} size{} val={:08x} pc={:08x}", kind, a, size, v, pc);
            if self.trace_io {
                eprintln!("[io] {}", s);
            }
            if self.unknown_log.len() < 4000 {
                self.unknown_log.push(s);
            }
        }
    }

    fn generic_read(&mut self, a: u32, size: u32, pc: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..size {
            v = v << 8 | *self.regs.get(&(a + i)).unwrap_or(&0) as u32;
        }
        self.note("R", a, size, v, pc);
        v
    }

    fn generic_write(&mut self, a: u32, size: u32, v: u32, pc: u32) {
        for i in 0..size {
            let shift = 8 * (size - 1 - i);
            self.regs.insert(a + i, (v >> shift) as u8);
        }
        self.note("W", a, size, v, pc);
    }

    // --- peripheral area 0xA4000000-0xA7FFFFFF (physical 0x04xxxxxx) ---

    fn periph_read(&mut self, a: u32, size: u32, pc: u32) -> u32 {
        let v = self.periph_read_inner(a, size, pc);
        if self.trace_all > 0 {
            self.trace_all -= 1;
            eprintln!("[all] R {:08x} size{} -> {:08x} pc={:08x}", a | 0xA000_0000, size, v, pc);
        }
        v
    }

    fn periph_read_inner(&mut self, a: u32, size: u32, pc: u32) -> u32 {
        let a = a | 0xA000_0000;
        match a {
            0xA408_0000..=0xA408_00FF => self.intc_read(a - 0xA408_0000),
            0xA449_0000..=0xA449_002F => self.tmu.read(a - 0xA449_0000, size),
            0xA44A_0000..=0xA44A_006F => self.cmt.read(a - 0xA44A_0000),
            0xA44D_0030..=0xA44D_00EF => {
                let o = a - 0xA44D_0030;
                self.etmu[(o / 0x20) as usize].read(o % 0x20)
            }
            0xA413_FEC0..=0xA413_FEDF => self.rtc.read(a - 0xA413_FEC0),
            0xA44B_0000..=0xA44B_000B => {
                let i = ((a - 0xA44B_0000) / 2) as usize;
                self.keys_seen = true;
                let w = self.keys[2 * i] as u32 | (self.keys[2 * i + 1] as u32) << 8;
                if size == 1 {
                    if a & 1 == 0 { w >> 8 } else { w & 0xFF }
                } else {
                    w
                }
            }
            0xA44B_0014 => (self.kysr_hi as u32) << 8 | self.kysr_flags as u32,
            0xA4CB_0010 => self.bcd.carry as u32,
            0xA4CB_0014 => self.bcd.a,
            0xA4CB_0018 => self.bcd.b,
            0xA4CB_001C => self.bcd.result,
            0xA415_0000 => self.generic_read(a, size, pc) & !0x8000_0000, // FRQCR (KICK reads 0)
            0xA415_0060 => 0, // LSTATS: frequency change complete
            0xA461_0080..=0xA461_0087 => (self.adc_value as u32) << 6, // left-aligned 10-bit result
            0xA461_0088 => self.adcsr as u32,
            _ => self.generic_read(a, size, pc),
        }
    }

    fn periph_write(&mut self, a: u32, size: u32, v: u32, pc: u32) {
        if self.trace_all > 0 {
            self.trace_all -= 1;
            eprintln!("[all] W {:08x} size{} <- {:08x} pc={:08x}", a | 0xA000_0000, size, v, pc);
        }
        let a = a | 0xA000_0000;
        match a {
            0xA408_0000..=0xA408_00FF => self.intc_write(a - 0xA408_0000, v),
            0xA449_0000..=0xA449_002F => {
                self.tmu.write(a - 0xA449_0000, size, v);
                self.irq_dirty = true;
            }
            0xA44A_0000..=0xA44A_006F => {
                self.cmt.write(a - 0xA44A_0000, v);
                self.irq_dirty = true;
            }
            0xA44D_0030..=0xA44D_00EF => {
                let o = a - 0xA44D_0030;
                self.etmu[(o / 0x20) as usize].write(o % 0x20, v);
                self.irq_dirty = true;
            }
            0xA413_FEC0..=0xA413_FEDF => {
                self.rtc.write(a - 0xA413_FEC0, v);
                self.irq_dirty = true;
            }
            0xA4CB_0010 => self.bcd.run(v),
            0xA4CB_0014 => self.bcd.a = v,
            0xA4CB_0018 => self.bcd.b = v,
            0xA44B_0014 => {
                self.kysr_hi = (v >> 8) as u8;
                self.kysr_flags &= !(v as u8);
                self.keysc_irq = self.kysr_flags != 0;
                self.irq_dirty = true;
            }
            0xA461_0088 => {
                let mut v = v as u16;
                // ADF (bit 15) is cleared by writing 0; ADST (bit 13) starts a
                // conversion, which completes immediately.
                let adf = self.adcsr & v & 0x8000;
                v = (v & !0x8000) | adf;
                if v & 0x2000 != 0 {
                    v = (v & !0x2000) | 0x8000;
                }
                self.adcsr = v;
            }
            _ => self.generic_write(a, size, v, pc),
        }
    }

    // --- P4 area ---

    fn p4_read(&mut self, a: u32, size: u32, pc: u32) -> u32 {
        match a {
            0xFF00_0000 => self.mmu.pteh,
            0xFF00_0004 => self.mmu.ptel,
            0xFF00_0008 => self.mmu.ttb,
            0xFF00_000C => self.mmu.tea,
            0xFF00_0010 => self.mmu.mmucr,
            0xFF00_001C => self.mmu.ccr,
            0xFF00_0020 => self.mmu.tra,
            0xFF00_0024 => self.mmu.expevt,
            0xFF00_0028 => self.mmu.intevt,
            0xFF00_0030 => 0x1030_0B00, // PVR: SH-4A
            0xFF00_0034 => self.mmu.ptea,
            0xFF00_0044 => 0x0000_2C00, // PRR: SH7305
            0xE520_0000..=0xE520_0FFF => mem_read(&self.ilram, (a & 0xFFF) as usize, size),
            0xE500_7000..=0xE500_8FFF => mem_read(&self.xram, (a - 0xE500_7000) as usize, size),
            0xE501_7000..=0xE501_8FFF => mem_read(&self.yram, (a - 0xE501_7000) as usize, size),
            0xFD80_0000..=0xFD80_3FFF => mem_read(&self.rsram, (a & 0x3FFF) as usize, size),
            0xF000_0000..=0xF5FF_FFFF => 0, // cache / ITLB arrays
            0xFE00_8060 => self.dma.dmaor as u32,
            0xFE00_8020..=0xFE00_808F => match Dma::decode(a) {
                Some((c, r)) => {
                    let ch = &self.dma.ch[c];
                    match r {
                        0x0 => ch.sar,
                        0x4 => ch.dar,
                        0x8 => ch.tcr,
                        _ => ch.chcr,
                    }
                }
                None => self.generic_read(a, size, pc),
            },
            _ => self.generic_read(a, size, pc),
        }
    }

    fn p4_write(&mut self, a: u32, size: u32, v: u32, pc: u32) {
        match a {
            0xFF00_0000 => self.mmu.pteh = v,
            0xFF00_0004 => self.mmu.ptel = v,
            0xFF00_0008 => self.mmu.ttb = v,
            0xFF00_000C => self.mmu.tea = v,
            0xFF00_0010 => {
                if v & 4 != 0 {
                    self.mmu.mmucr = v & !4;
                    self.tlb_flush_request = true;
                } else {
                    self.mmu.mmucr = v;
                }
            }
            0xFF00_001C => self.mmu.ccr = v,
            0xFF00_0020 => self.mmu.tra = v,
            0xFF00_0024 => self.mmu.expevt = v,
            0xFF00_0028 => self.mmu.intevt = v,
            0xFF00_0034 => self.mmu.ptea = v,
            0xE520_0000..=0xE520_0FFF => mem_write(&mut self.ilram, (a & 0xFFF) as usize, size, v),
            0xE500_7000..=0xE500_8FFF => mem_write(&mut self.xram, (a - 0xE500_7000) as usize, size, v),
            0xE501_7000..=0xE501_8FFF => mem_write(&mut self.yram, (a - 0xE501_7000) as usize, size, v),
            0xFD80_0000..=0xFD80_3FFF => mem_write(&mut self.rsram, (a & 0x3FFF) as usize, size, v),
            0xE000_0000..=0xE3FF_FFFF => {} // store queues
            0xF000_0000..=0xF5FF_FFFF => {} // cache / ITLB arrays
            0xFE00_8060 => {
                // AE and NMIF are cleared by writing 0.
                let keep = self.dma.dmaor & v as u16 & 0x6;
                self.dma.dmaor = (v as u16 & !0x6) | keep;
                for c in 0..6 {
                    self.dma_run(c);
                }
            }
            0xFE00_8020..=0xFE00_808F => match Dma::decode(a) {
                Some((c, r)) => {
                    let ch = &mut self.dma.ch[c];
                    match r {
                        0x0 => ch.sar = v,
                        0x4 => ch.dar = v,
                        0x8 => ch.tcr = v & 0x00FF_FFFF,
                        _ => {
                            // TE is cleared by writing 0.
                            let te = ch.chcr & v & 2;
                            ch.chcr = (v & !2) | te;
                        }
                    }
                    self.dma_run(c);
                    self.irq_dirty = true;
                }
                None => self.generic_write(a, size, v, pc),
            },
            _ => self.generic_write(a, size, v, pc),
        }
    }

    // --- dispatch ---

    fn read(&mut self, a: u32, size: u32, pc: u32) -> u32 {
        if a >= 0xE000_0000 {
            return self.p4_read(a, size, pc);
        }
        match a >> 24 {
            0x00..=0x03 => match size {
                1 => self.flash.read8(a) as u32,
                2 => self.flash.read16(a) as u32,
                _ => self.flash.read32(a),
            },
            0x04..=0x07 => self.periph_read(a, size, pc),
            0x08..=0x0B => mem_read(&self.ram, (a as usize) & (RAM_SIZE - 1), size),
            0x14..=0x17 => {
                if a & 0x10000 != 0 {
                    self.lcd.read_data() as u32
                } else {
                    self.lcd.read_status() as u32
                }
            }
            _ => self.generic_read(a, size, pc),
        }
    }

    fn write(&mut self, a: u32, size: u32, v: u32, pc: u32) {
        if a >= 0xE000_0000 {
            return self.p4_write(a, size, v, pc);
        }
        match a >> 24 {
            0x00..=0x03 => match size {
                1 => self.flash.write8(a, v as u8),
                2 => self.flash.write16(a, v as u16),
                _ => self.flash.write32(a, v),
            },
            0x04..=0x07 => self.periph_write(a, size, v, pc),
            0x08..=0x0B => mem_write(&mut self.ram, (a as usize) & (RAM_SIZE - 1), size, v),
            0x14..=0x17 => {
                if a & 0x10000 != 0 {
                    self.lcd.write_data(v as u8)
                } else {
                    self.lcd.write_sel(v as u8)
                }
            }
            _ => self.generic_write(a, size, v, pc),
        }
    }

    #[inline]
    pub fn read8(&mut self, a: u32, pc: u32) -> u8 {
        if a >> 24 == 0x08 {
            return self.ram[(a as usize) & (RAM_SIZE - 1)];
        }
        self.read(a, 1, pc) as u8
    }
    #[inline]
    pub fn read16(&mut self, a: u32, pc: u32) -> u16 {
        if a >> 24 == 0x08 {
            let o = (a as usize) & (RAM_SIZE - 1);
            return u16::from_be_bytes([self.ram[o], self.ram[o + 1]]);
        }
        self.read(a, 2, pc) as u16
    }
    #[inline]
    pub fn read32(&mut self, a: u32, pc: u32) -> u32 {
        if a >> 24 == 0x08 {
            let o = (a as usize) & (RAM_SIZE - 1);
            return u32::from_be_bytes([self.ram[o], self.ram[o + 1], self.ram[o + 2], self.ram[o + 3]]);
        }
        self.read(a, 4, pc)
    }
    #[inline]
    pub fn write8(&mut self, a: u32, v: u8, pc: u32) {
        if a >> 24 == 0x08 {
            self.ram[(a as usize) & (RAM_SIZE - 1)] = v;
            return;
        }
        self.write(a, 1, v as u32, pc)
    }
    #[inline]
    pub fn write16(&mut self, a: u32, v: u16, pc: u32) {
        if a >> 24 == 0x08 {
            let o = (a as usize) & (RAM_SIZE - 1);
            self.ram[o..o + 2].copy_from_slice(&v.to_be_bytes());
            return;
        }
        self.write(a, 2, v as u32, pc)
    }
    #[inline]
    pub fn write32(&mut self, a: u32, v: u32, pc: u32) {
        if a >> 24 == 0x08 {
            let o = (a as usize) & (RAM_SIZE - 1);
            self.ram[o..o + 4].copy_from_slice(&v.to_be_bytes());
            return;
        }
        self.write(a, 4, v, pc)
    }
    #[inline]
    pub fn fetch16(&mut self, a: u32) -> u16 {
        match a >> 24 {
            0x00..=0x03 => self.flash.read16(a),
            0x08 => {
                let o = (a as usize) & (RAM_SIZE - 1);
                u16::from_be_bytes([self.ram[o], self.ram[o + 1]])
            }
            _ => self.read(a, 2, a) as u16,
        }
    }

    /// Run a DMA transfer to completion if the channel is enabled.
    fn dma_run(&mut self, c: usize) {
        let ch = self.dma.ch[c];
        if ch.chcr & 1 == 0 || ch.chcr & 2 != 0 || self.dma.dmaor & 1 == 0 {
            return;
        }
        let ts = ((ch.chcr >> 20) & 3) << 2 | (ch.chcr >> 3) & 3;
        let unit: u32 = match ts {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 16,
            4 => 32,
            _ => 4,
        };
        let count = if ch.tcr == 0 { 0x0100_0000 } else { ch.tcr };
        let phys = |a: u32| if a >= 0xE000_0000 { a } else { a & 0x1FFF_FFFF };
        let step = |mode: u32, a: u32| match mode {
            1 => a.wrapping_add(unit),
            2 => a.wrapping_sub(unit),
            _ => a,
        };
        let sm = (ch.chcr >> 12) & 3;
        let dm = (ch.chcr >> 14) & 3;
        let (mut sar, mut dar) = (phys(ch.sar), phys(ch.dar));
        if self.trace_io {
            eprintln!("[dma] ch{} {:08x} -> {:08x}, {} x {} bytes, sm={} dm={}", c, sar, dar, count, unit, sm, dm);
        }
        for _ in 0..count {
            if unit >= 4 {
                for k in (0..unit).step_by(4) {
                    let v = self.read(sar + k, 4, 0);
                    self.write(dar + k, 4, v, 0);
                }
            } else {
                let v = self.read(sar, unit, 0);
                self.write(dar, unit, v, 0);
            }
            sar = step(sm, sar);
            dar = step(dm, dar);
        }
        let ch = &mut self.dma.ch[c];
        ch.sar = sar;
        ch.dar = dar;
        ch.tcr = 0;
        ch.chcr |= 2;
        self.irq_dirty = true;
    }

    /// Update the keyboard matrix. Raises the KEYSC interrupt on change.
    pub fn set_keys(&mut self, keys: [u8; 12]) {
        if keys != self.keys {
            self.keys_seen = false;
            self.keys = keys;
            self.kysr_flags |= 0x08;
            if keys.iter().all(|&b| b == 0) {
                self.kysr_flags |= 0x40;
            }
            self.keysc_irq = true;
            self.irq_dirty = true;
        }
    }
}

fn mem_read(m: &[u8], o: usize, size: u32) -> u32 {
    let o = o % m.len();
    match size {
        1 => m[o] as u32,
        2 => u16::from_be_bytes([m[o], m[o + 1]]) as u32,
        _ => u32::from_be_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]),
    }
}

fn mem_write(m: &mut [u8], o: usize, size: u32, v: u32) {
    let o = o % m.len();
    match size {
        1 => m[o] = v as u8,
        2 => m[o..o + 2].copy_from_slice(&(v as u16).to_be_bytes()),
        _ => m[o..o + 4].copy_from_slice(&v.to_be_bytes()),
    }
}

// --- snapshots ---

use crate::state::{Bad, Snap, R, W};

impl Snap for Bus {
    fn save(&self, w: &mut W) {
        w.bytes(&self.ram);
        w.bytes(&self.ilram);
        w.bytes(&self.xram);
        w.bytes(&self.yram);
        w.bytes(&self.rsram);
        let m = &self.mmu;
        for v in [m.pteh, m.ptel, m.ttb, m.tea, m.mmucr, m.ccr, m.tra, m.expevt, m.intevt, m.ptea] {
            w.u32(v);
        }
        for v in self.intc.ipr {
            w.u16(v);
        }
        w.0.extend_from_slice(&self.intc.imr);
        self.tmu.save(w);
        for e in &self.etmu {
            e.save(w);
        }
        self.rtc.save(w);
        self.cmt.save(w);
        for c in &self.dma.ch {
            for v in [c.sar, c.dar, c.tcr, c.chcr] {
                w.u32(v);
            }
        }
        w.u16(self.dma.dmaor);
        w.u32(self.bcd.a);
        w.u32(self.bcd.b);
        w.u32(self.bcd.result);
        w.bool(self.bcd.carry);
        w.u16(self.adcsr);
        w.u16(self.adc_value);
        w.0.extend_from_slice(&self.keys);
        w.bool(self.keysc_irq);
        w.u8(self.kysr_hi);
        w.u8(self.kysr_flags);
        self.lcd.save(w);
        let mut regs: Vec<(u32, u8)> = self.regs.iter().map(|(a, v)| (*a, *v)).collect();
        regs.sort();
        w.u32(regs.len() as u32);
        for (a, v) in regs {
            w.u32(a);
            w.u8(v);
        }
        w.u64(self.next_tick);
        w.u64(self.last_tick);
        w.u64(self.rtc_acc);
    }

    fn load(&mut self, r: &mut R) -> Result<(), Bad> {
        r.bytes_into(&mut self.ram)?;
        r.bytes_into(&mut self.ilram)?;
        r.bytes_into(&mut self.xram)?;
        r.bytes_into(&mut self.yram)?;
        r.bytes_into(&mut self.rsram)?;
        let m = &mut self.mmu;
        for f in [
            &mut m.pteh, &mut m.ptel, &mut m.ttb, &mut m.tea, &mut m.mmucr,
            &mut m.ccr, &mut m.tra, &mut m.expevt, &mut m.intevt, &mut m.ptea,
        ] {
            *f = r.u32()?;
        }
        for v in self.intc.ipr.iter_mut() {
            *v = r.u16()?;
        }
        for v in self.intc.imr.iter_mut() {
            *v = r.u8()?;
        }
        self.tmu.load(r)?;
        for e in self.etmu.iter_mut() {
            e.load(r)?;
        }
        self.rtc.load(r)?;
        self.cmt.load(r)?;
        for c in self.dma.ch.iter_mut() {
            c.sar = r.u32()?;
            c.dar = r.u32()?;
            c.tcr = r.u32()?;
            c.chcr = r.u32()?;
        }
        self.dma.dmaor = r.u16()?;
        self.bcd.a = r.u32()?;
        self.bcd.b = r.u32()?;
        self.bcd.result = r.u32()?;
        self.bcd.carry = r.bool()?;
        self.adcsr = r.u16()?;
        self.adc_value = r.u16()?;
        for k in self.keys.iter_mut() {
            *k = r.u8()?;
        }
        self.keysc_irq = r.bool()?;
        self.kysr_hi = r.u8()?;
        self.kysr_flags = r.u8()?;
        self.lcd.load(r)?;
        self.regs.clear();
        for _ in 0..r.u32()? {
            let a = r.u32()?;
            let v = r.u8()?;
            self.regs.insert(a, v);
        }
        self.next_tick = r.u64()?;
        self.last_tick = r.u64()?;
        self.rtc_acc = r.u64()?;
        self.irq_dirty = true;
        Ok(())
    }
}
