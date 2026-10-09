//! SH-4A (SH4AL-DSP) integer CPU core, as found in the SH7305.
//!
//! Implements the SH-4 integer instruction set plus the SH-4A additions the
//! CASIO OS uses, banked registers, exceptions, interrupts and the UTLB-based
//! MMU. There is no FPU on this chip; DSP instructions are not implemented.

use crate::bus::Bus;

pub const SR_T: u32 = 1 << 0;
pub const SR_S: u32 = 1 << 1;
pub const SR_IMASK: u32 = 0xF << 4;
pub const SR_Q: u32 = 1 << 8;
pub const SR_M: u32 = 1 << 9;
pub const SR_BL: u32 = 1 << 28;
pub const SR_RB: u32 = 1 << 29;
pub const SR_MD: u32 = 1 << 30;
const SR_MASK: u32 = 0x7000_83F3;

/// Exception raised while executing an instruction (payloads are shown in logs).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum Exc {
    TlbMissRead(u32),
    TlbMissWrite(u32),
    InitialPageWrite(u32),
    AddrErrRead(u32),
    AddrErrWrite(u32),
    Trap(u32),
    Illegal(u16),
    SlotIllegal(u16),
}

enum Flow {
    Next,
    Jump(u32),
    Delayed(u32),
}

#[derive(Clone, Copy, Default)]
pub struct TlbEntry {
    pub vpn: u32,  // virtual address bits 31..10
    pub asid: u8,
    pub ppn: u32,  // physical address bits 28..10
    pub sz: u8,    // 0:1K 1:4K 2:64K 3:1M
    pub v: bool,
    pub d: bool,
    pub sh: bool,
    pub pr: u8,
    pub c: bool,
    pub wt: bool,
}

impl TlbEntry {
    fn mask(&self) -> u32 {
        match self.sz {
            0 => 0xFFFF_FC00,
            1 => 0xFFFF_F000,
            2 => 0xFFFF_0000,
            _ => 0xFFF0_0000,
        }
    }
}

pub struct Cpu {
    pub r: [u32; 16],
    pub bank: [u32; 8],
    pub sr: u32,
    pub gbr: u32,
    pub vbr: u32,
    pub ssr: u32,
    pub spc: u32,
    pub sgr: u32,
    pub dbr: u32,
    pub mach: u32,
    pub macl: u32,
    pub pr: u32,
    pub pc: u32,
    pub ldst: bool,
    pub sleeping: bool,
    pub cycles: u64,
    pub instructions: u64,
    pub utlb: [TlbEntry; 64],
    tlb_hint: usize,
    pub bus: Bus,
    /// Ring buffer of recent PCs, for post-mortem debugging.
    pub pc_hist: [u32; 64],
    pub pc_hist_pos: usize,
    pub unknown_ops: u32,
}

impl Cpu {
    pub fn new(bus: Bus) -> Self {
        let mut c = Cpu {
            r: [0; 16],
            bank: [0; 8],
            sr: 0,
            gbr: 0,
            vbr: 0,
            ssr: 0,
            spc: 0,
            sgr: 0,
            dbr: 0,
            mach: 0,
            macl: 0,
            pr: 0,
            pc: 0,
            ldst: false,
            sleeping: false,
            cycles: 0,
            instructions: 0,
            utlb: [TlbEntry::default(); 64],
            tlb_hint: 0,
            bus,
            pc_hist: [0; 64],
            pc_hist_pos: 0,
            unknown_ops: 0,
        };
        c.reset();
        c
    }

    pub fn reset(&mut self) {
        self.sr = SR_MD | SR_RB | SR_BL | SR_IMASK;
        self.vbr = 0;
        self.pc = 0xA000_0000;
        self.bus.mmu.mmucr = 0;
        for e in self.utlb.iter_mut() {
            e.v = false;
        }
        self.sleeping = false;
    }

    // --- status register and banks ---

    #[inline]
    fn t(&self) -> bool {
        self.sr & SR_T != 0
    }
    #[inline]
    fn set_t(&mut self, v: bool) {
        if v {
            self.sr |= SR_T
        } else {
            self.sr &= !SR_T
        }
    }
    #[inline]
    fn banked(sr: u32) -> bool {
        sr & SR_MD != 0 && sr & SR_RB != 0
    }

    pub fn set_sr(&mut self, new: u32) {
        let new = new & SR_MASK;
        if Self::banked(self.sr) != Self::banked(new) {
            for i in 0..8 {
                std::mem::swap(&mut self.r[i], &mut self.bank[i]);
            }
        }
        self.sr = new;
        self.bus.irq_dirty = true;
    }

    // --- MMU ---

    fn tlb_lookup(&mut self, va: u32, write: bool) -> Result<u32, Exc> {
        let asid = (self.bus.mmu.pteh & 0xFF) as u8;
        let sv = self.bus.mmu.mmucr & (1 << 8) != 0 && self.sr & SR_MD != 0;
        let n = self.utlb.len();
        for k in 0..n {
            let i = (self.tlb_hint + k) % n;
            let e = &self.utlb[i];
            if !e.v {
                continue;
            }
            let m = e.mask();
            if (va & m) != (e.vpn & m) {
                continue;
            }
            if !(e.sh || sv || e.asid == asid) {
                continue;
            }
            if write && !e.d {
                return Err(Exc::InitialPageWrite(va));
            }
            self.tlb_hint = i;
            return Ok((e.ppn & m & 0x1FFF_FFFF) | (va & !m));
        }
        Err(if write { Exc::TlbMissWrite(va) } else { Exc::TlbMissRead(va) })
    }

    #[inline]
    fn translate(&mut self, va: u32, write: bool) -> Result<u32, Exc> {
        match va >> 29 {
            4 | 5 => Ok(va & 0x1FFF_FFFF),
            7 => Ok(va),
            _ => {
                if self.bus.mmu.mmucr & 1 != 0 {
                    self.tlb_lookup(va, write)
                } else {
                    Ok(va & 0x1FFF_FFFF)
                }
            }
        }
    }

    pub fn ldtlb(&mut self) {
        let m = &self.bus.mmu;
        let urc = ((m.mmucr >> 10) & 63) as usize;
        let ptel = m.ptel;
        let pteh = m.pteh;
        self.utlb[urc] = TlbEntry {
            vpn: pteh & 0xFFFF_FC00,
            asid: (pteh & 0xFF) as u8,
            ppn: ptel & 0x1FFF_FC00,
            sz: (((ptel >> 7) & 1) << 1 | ((ptel >> 4) & 1)) as u8,
            v: ptel & 0x100 != 0,
            d: ptel & 4 != 0,
            sh: ptel & 2 != 0,
            pr: ((ptel >> 5) & 3) as u8,
            c: ptel & 8 != 0,
            wt: ptel & 1 != 0,
        };
        // Advance the replacement counter (wrapping at URB when set).
        let urb = (m.mmucr >> 18) & 63;
        let mut next = (urc as u32 + 1) & 63;
        if urb != 0 && next >= urb {
            next = 0;
        }
        self.bus.mmu.mmucr = (self.bus.mmu.mmucr & !(63 << 10)) | (next << 10);
    }

    /// Memory-mapped UTLB address/data arrays (0xF6xxxxxx / 0xF7xxxxxx).
    fn utlb_array_write(&mut self, addr: u32, val: u32) {
        let i = ((addr >> 8) & 63) as usize;
        if addr >> 24 == 0xF6 {
            if addr & 0x80 != 0 {
                // Associative write: update V/D of a matching entry.
                let vpn = val & 0xFFFF_FC00;
                let asid = (val & 0xFF) as u8;
                for e in self.utlb.iter_mut() {
                    let m = e.mask();
                    if e.v && (e.vpn & m) == (vpn & m) && (e.sh || e.asid == asid) {
                        e.v = val & 0x100 != 0;
                        e.d = val & 0x200 != 0;
                    }
                }
            } else {
                let e = &mut self.utlb[i];
                e.vpn = val & 0xFFFF_FC00;
                e.asid = (val & 0xFF) as u8;
                e.v = val & 0x100 != 0;
                e.d = val & 0x200 != 0;
            }
        } else {
            let e = &mut self.utlb[i];
            e.ppn = val & 0x1FFF_FC00;
            e.v = val & 0x100 != 0;
            e.sz = (((val >> 7) & 1) << 1 | ((val >> 4) & 1)) as u8;
            e.pr = ((val >> 5) & 3) as u8;
            e.c = val & 8 != 0;
            e.d = val & 4 != 0;
            e.sh = val & 2 != 0;
            e.wt = val & 1 != 0;
        }
    }

    fn utlb_array_read(&self, addr: u32) -> u32 {
        let e = &self.utlb[((addr >> 8) & 63) as usize];
        if addr >> 24 == 0xF6 {
            e.vpn | (e.d as u32) << 9 | (e.v as u32) << 8 | e.asid as u32
        } else {
            e.ppn
                | (e.v as u32) << 8
                | ((e.sz as u32 >> 1) & 1) << 7
                | (e.pr as u32) << 5
                | (e.sz as u32 & 1) << 4
                | (e.c as u32) << 3
                | (e.d as u32) << 2
                | (e.sh as u32) << 1
                | e.wt as u32
        }
    }

    // --- memory access (virtual) ---

    #[inline]
    pub fn read8(&mut self, va: u32) -> Result<u8, Exc> {
        let pa = self.translate(va, false)?;
        Ok(self.bus.read8(pa, self.pc))
    }
    #[inline]
    pub fn read16(&mut self, va: u32) -> Result<u16, Exc> {
        if va & 1 != 0 {
            return Err(Exc::AddrErrRead(va));
        }
        let pa = self.translate(va, false)?;
        Ok(self.bus.read16(pa, self.pc))
    }
    #[inline]
    pub fn read32(&mut self, va: u32) -> Result<u32, Exc> {
        if va & 3 != 0 {
            return Err(Exc::AddrErrRead(va));
        }
        if va >> 24 == 0xF6 || va >> 24 == 0xF7 {
            return Ok(self.utlb_array_read(va));
        }
        let pa = self.translate(va, false)?;
        Ok(self.bus.read32(pa, self.pc))
    }
    #[inline]
    pub fn write8(&mut self, va: u32, v: u8) -> Result<(), Exc> {
        let pa = self.translate(va, true)?;
        self.bus.write8(pa, v, self.pc);
        Ok(())
    }
    #[inline]
    pub fn write16(&mut self, va: u32, v: u16) -> Result<(), Exc> {
        if va & 1 != 0 {
            return Err(Exc::AddrErrWrite(va));
        }
        let pa = self.translate(va, true)?;
        self.bus.write16(pa, v, self.pc);
        Ok(())
    }
    #[inline]
    pub fn write32(&mut self, va: u32, v: u32) -> Result<(), Exc> {
        if va & 3 != 0 {
            return Err(Exc::AddrErrWrite(va));
        }
        if va >> 24 == 0xF6 || va >> 24 == 0xF7 {
            self.utlb_array_write(va, v);
            return Ok(());
        }
        let pa = self.translate(va, true)?;
        self.bus.write32(pa, v, self.pc);
        if self.bus.tlb_flush_request {
            self.bus.tlb_flush_request = false;
            for e in self.utlb.iter_mut() {
                e.v = false;
            }
        }
        Ok(())
    }
    #[inline]
    fn fetch(&mut self, va: u32) -> Result<u16, Exc> {
        if va & 1 != 0 {
            return Err(Exc::AddrErrRead(va));
        }
        let pa = self.translate(va, false)?;
        Ok(self.bus.fetch16(pa))
    }

    // --- exceptions and interrupts ---

    fn enter_exception(&mut self, offset: u32, spc: u32) {
        self.ssr = self.sr;
        self.spc = spc;
        self.sgr = self.r[15];
        self.ldst = false;
        let sr = self.sr | SR_MD | SR_RB | SR_BL;
        self.set_sr(sr);
        self.pc = self.vbr.wrapping_add(offset);
    }

    fn raise(&mut self, e: Exc, pc: u32) {
        if self.sr & SR_BL != 0 {
            eprintln!(
                "[cpu] exception {:?} at pc={:08x} with SR.BL=1 -> reset",
                e, pc
            );
            self.dump_history();
            self.reset();
            return;
        }
        let mmu = &mut self.bus.mmu;
        let (code, offset, spc) = match e {
            Exc::TlbMissRead(a) => {
                mmu.tea = a;
                mmu.pteh = (mmu.pteh & 0xFF) | (a & 0xFFFF_FC00);
                (0x040, 0x400, pc)
            }
            Exc::TlbMissWrite(a) => {
                mmu.tea = a;
                mmu.pteh = (mmu.pteh & 0xFF) | (a & 0xFFFF_FC00);
                (0x060, 0x400, pc)
            }
            Exc::InitialPageWrite(a) => {
                mmu.tea = a;
                mmu.pteh = (mmu.pteh & 0xFF) | (a & 0xFFFF_FC00);
                (0x080, 0x100, pc)
            }
            Exc::AddrErrRead(a) => {
                mmu.tea = a;
                (0x0E0, 0x100, pc)
            }
            Exc::AddrErrWrite(a) => {
                mmu.tea = a;
                (0x100, 0x100, pc)
            }
            Exc::Trap(imm) => {
                mmu.tra = imm << 2;
                (0x160, 0x100, pc.wrapping_add(2))
            }
            Exc::Illegal(_) => (0x180, 0x100, pc),
            Exc::SlotIllegal(_) => (0x1A0, 0x100, pc),
        };
        if self.bus.trace_exc {
            eprintln!(
                "[cpu] exception {:?} code {:03x} at pc={:08x} vbr={:08x}",
                e, code, pc, self.vbr
            );
        }
        if matches!(e, Exc::Illegal(_) | Exc::SlotIllegal(_) | Exc::AddrErrRead(_) | Exc::AddrErrWrite(_)) {
            eprintln!("[cpu] {:?} at pc={:08x}", e, pc);
            self.dump_history();
        }
        self.bus.mmu.expevt = code;
        self.enter_exception(offset, spc);
    }

    fn check_interrupt(&mut self) -> bool {
        if self.bus.irq_dirty {
            self.bus.update_irq();
        }
        if let Some((level, code)) = self.bus.irq_line {
            let imask = (self.sr >> 4) & 0xF;
            if level > imask && self.sr & SR_BL == 0 {
                self.bus.mmu.intevt = code;
                self.sleeping = false;
                let pc = self.pc;
                self.enter_exception(0x600, pc);
                // SR.IMASK is not changed on SH-4 interrupt acceptance.
                return true;
            }
            if self.sleeping && level > 0 && level > imask {
                // BL=1 while sleeping: wake up without taking the interrupt.
                self.sleeping = false;
            }
        }
        false
    }

    pub fn dump_history(&self) {
        let mut s = String::new();
        for k in 0..self.pc_hist.len() {
            let i = (self.pc_hist_pos + k) % self.pc_hist.len();
            s += &format!("{:08x} ", self.pc_hist[i]);
        }
        eprintln!("[cpu] recent PCs: {}", s);
        eprintln!("[cpu] {}", self.regs_string());
    }

    pub fn regs_string(&self) -> String {
        let mut s = format!("pc={:08x} sr={:08x} pr={:08x} ", self.pc, self.sr, self.pr);
        for i in 0..16 {
            s += &format!("r{}={:08x} ", i, self.r[i]);
        }
        s += &format!("gbr={:08x} vbr={:08x} spc={:08x} ssr={:08x}", self.gbr, self.vbr, self.spc, self.ssr);
        s
    }

    // --- main step ---

    /// Run one instruction (or idle while sleeping). Returns cycles consumed.
    #[inline]
    pub fn step(&mut self) -> u32 {
        if self.bus.irq_line.is_some() || self.bus.irq_dirty {
            if self.check_interrupt() {
                return 4;
            }
        }
        if self.sleeping {
            return 64;
        }
        let pc = self.pc;
        self.pc_hist[self.pc_hist_pos] = pc;
        self.pc_hist_pos = (self.pc_hist_pos + 1) & 63;
        self.instructions += 1;
        let op = match self.fetch(pc) {
            Ok(op) => op,
            Err(e) => {
                self.raise(e, pc);
                return 1;
            }
        };
        match self.exec(op, pc) {
            Ok(Flow::Next) => self.pc = pc.wrapping_add(2),
            Ok(Flow::Jump(t)) => self.pc = t,
            Ok(Flow::Delayed(t)) => {
                let spc = pc.wrapping_add(2);
                let slot = match self.fetch(spc) {
                    Ok(op) => op,
                    Err(e) => {
                        self.raise(e, pc);
                        return 2;
                    }
                };
                match self.exec(slot, spc) {
                    Ok(Flow::Next) => self.pc = t,
                    Ok(_) => {
                        self.raise(Exc::SlotIllegal(slot), pc);
                    }
                    Err(e) => self.raise(e, pc),
                }
                return 2;
            }
            Err(e) => self.raise(e, pc),
        }
        1
    }

    #[inline]
    fn disp_pc_l(pc: u32, d: u32) -> u32 {
        (pc & !3).wrapping_add(4).wrapping_add(d << 2)
    }

    fn exec(&mut self, op: u16, pc: u32) -> Result<Flow, Exc> {
        let n = ((op >> 8) & 0xF) as usize;
        let m = ((op >> 4) & 0xF) as usize;
        let imm8 = (op & 0xFF) as u32;
        let simm8 = (op & 0xFF) as i8 as i32 as u32;
        match op >> 12 {
            0x0 => match op & 0xF {
                0x2 => {
                    // STC xx,Rn
                    let v = match (op >> 4) & 0xF {
                        0x0 => self.sr,
                        0x1 => self.gbr,
                        0x2 => self.vbr,
                        0x3 => self.ssr,
                        0x4 => self.spc,
                        0x5 | 0x6 | 0x7 => 0, // DSP MOD/RS/RE
                        x if x >= 8 => self.bank[(x & 7) as usize],
                        _ => return self.unknown(op, pc),
                    };
                    self.r[n] = v;
                }
                0x3 => match (op >> 4) & 0xF {
                    0x0 => {
                        // BSRF Rn
                        self.pr = pc.wrapping_add(4);
                        return Ok(Flow::Delayed(pc.wrapping_add(4).wrapping_add(self.r[n])));
                    }
                    0x2 => return Ok(Flow::Delayed(pc.wrapping_add(4).wrapping_add(self.r[n]))), // BRAF
                    0x6 => {
                        // MOVLI.L @Rm,R0
                        self.r[0] = self.read32(self.r[n])?;
                        self.ldst = true;
                    }
                    0x7 => {
                        // MOVCO.L R0,@Rn
                        if self.ldst {
                            self.write32(self.r[n], self.r[0])?;
                        }
                        let t = self.ldst;
                        self.set_t(t);
                        self.ldst = false;
                    }
                    0x8 | 0x9 | 0xA | 0xB | 0xD | 0xE => {} // PREF, OCBI, OCBP, OCBWB, PREFI, ICBI
                    0xC => self.write32(self.r[n], self.r[0])?, // MOVCA.L R0,@Rn
                    _ => return self.unknown(op, pc),
                },
                0x4 => self.write8(self.r[n].wrapping_add(self.r[0]), self.r[m] as u8)?,
                0x5 => self.write16(self.r[n].wrapping_add(self.r[0]), self.r[m] as u16)?,
                0x6 => self.write32(self.r[n].wrapping_add(self.r[0]), self.r[m])?,
                0x7 => self.macl = self.r[n].wrapping_mul(self.r[m]),
                0x8 => match op {
                    0x0008 => self.set_t(false),
                    0x0018 => self.set_t(true),
                    0x0028 => {
                        self.mach = 0;
                        self.macl = 0;
                    }
                    0x0038 => self.ldtlb(),
                    0x0048 => self.sr &= !SR_S,
                    0x0058 => self.sr |= SR_S,
                    _ => return self.unknown(op, pc),
                },
                0x9 => match (op >> 4) & 0xF {
                    0x0 => {} // NOP
                    0x1 => {
                        // DIV0U
                        self.sr &= !(SR_M | SR_Q | SR_T);
                    }
                    0x2 => self.r[n] = self.t() as u32, // MOVT
                    _ => return self.unknown(op, pc),
                },
                0xA => {
                    let v = match (op >> 4) & 0xF {
                        0x0 => self.mach,
                        0x1 => self.macl,
                        0x2 => self.pr,
                        0x3 => self.sgr,
                        0x5 | 0x6 | 0x7 => 0, // FPUL / FPSCR / DSR: no FPU
                        0xF => self.dbr,
                        _ => return self.unknown(op, pc),
                    };
                    self.r[n] = v;
                }
                0xB => match op {
                    0x000B => return Ok(Flow::Delayed(self.pr)), // RTS
                    0x001B => {
                        // SLEEP
                        self.sleeping = true;
                        return Ok(Flow::Next);
                    }
                    0x002B => {
                        // RTE
                        let target = self.spc;
                        let ssr = self.ssr;
                        self.set_sr(ssr);
                        return Ok(Flow::Delayed(target));
                    }
                    0x00AB => {} // SYNCO
                    _ => return self.unknown(op, pc),
                },
                0xC => self.r[n] = self.read8(self.r[m].wrapping_add(self.r[0]))? as i8 as i32 as u32,
                0xD => self.r[n] = self.read16(self.r[m].wrapping_add(self.r[0]))? as i16 as i32 as u32,
                0xE => self.r[n] = self.read32(self.r[m].wrapping_add(self.r[0]))?,
                0xF => {
                    // MAC.L @Rm+,@Rn+
                    let a = self.read32(self.r[n])? as i32 as i64;
                    self.r[n] = self.r[n].wrapping_add(4);
                    let b = self.read32(self.r[m])? as i32 as i64;
                    self.r[m] = self.r[m].wrapping_add(4);
                    let mac = ((self.mach as u64) << 32 | self.macl as u64) as i64;
                    let mut res = mac.wrapping_add(a.wrapping_mul(b));
                    if self.sr & SR_S != 0 {
                        res = res.clamp(-(1i64 << 47), (1i64 << 47) - 1);
                    }
                    self.mach = (res >> 32) as u32;
                    self.macl = res as u32;
                }
                _ => return self.unknown(op, pc),
            },
            0x1 => {
                let d = (op & 0xF) as u32;
                self.write32(self.r[n].wrapping_add(d << 2), self.r[m])?;
            }
            0x2 => match op & 0xF {
                0x0 => self.write8(self.r[n], self.r[m] as u8)?,
                0x1 => self.write16(self.r[n], self.r[m] as u16)?,
                0x2 => self.write32(self.r[n], self.r[m])?,
                0x4 => {
                    let a = self.r[n].wrapping_sub(1);
                    self.write8(a, self.r[m] as u8)?;
                    self.r[n] = a;
                }
                0x5 => {
                    let a = self.r[n].wrapping_sub(2);
                    self.write16(a, self.r[m] as u16)?;
                    self.r[n] = a;
                }
                0x6 => {
                    let a = self.r[n].wrapping_sub(4);
                    self.write32(a, self.r[m])?;
                    self.r[n] = a;
                }
                0x7 => {
                    // DIV0S
                    let q = self.r[n] >> 31;
                    let mm = self.r[m] >> 31;
                    self.sr = (self.sr & !(SR_Q | SR_M | SR_T)) | q << 8 | mm << 9 | (q ^ mm);
                }
                0x8 => {
                    let t = self.r[n] & self.r[m] == 0;
                    self.set_t(t);
                }
                0x9 => self.r[n] &= self.r[m],
                0xA => self.r[n] ^= self.r[m],
                0xB => self.r[n] |= self.r[m],
                0xC => {
                    let x = self.r[n] ^ self.r[m];
                    let t = x & 0xFF00_0000 == 0 || x & 0x00FF_0000 == 0 || x & 0xFF00 == 0 || x & 0xFF == 0;
                    self.set_t(t);
                }
                0xD => self.r[n] = (self.r[n] >> 16) | (self.r[m] << 16),
                0xE => self.macl = (self.r[n] & 0xFFFF).wrapping_mul(self.r[m] & 0xFFFF),
                0xF => {
                    self.macl = ((self.r[n] as i16 as i32).wrapping_mul(self.r[m] as i16 as i32)) as u32
                }
                _ => return self.unknown(op, pc),
            },
            0x3 => {
                let a = self.r[n];
                let b = self.r[m];
                match op & 0xF {
                    0x0 => self.set_t(a == b),
                    0x2 => self.set_t(a >= b),
                    0x3 => self.set_t(a as i32 >= b as i32),
                    0x4 => self.div1(n, m),
                    0x5 => {
                        let r = a as u64 * b as u64;
                        self.mach = (r >> 32) as u32;
                        self.macl = r as u32;
                    }
                    0x6 => self.set_t(a > b),
                    0x7 => self.set_t(a as i32 > b as i32),
                    0x8 => self.r[n] = a.wrapping_sub(b),
                    0xA => {
                        // SUBC
                        let t = self.t() as u32;
                        let tmp1 = a.wrapping_sub(b);
                        let res = tmp1.wrapping_sub(t);
                        self.r[n] = res;
                        self.set_t(a < tmp1 || tmp1 < res);
                    }
                    0xB => {
                        let (res, ov) = (a as i32).overflowing_sub(b as i32);
                        self.r[n] = res as u32;
                        self.set_t(ov);
                    }
                    0xC => self.r[n] = a.wrapping_add(b),
                    0xD => {
                        let r = (a as i32 as i64).wrapping_mul(b as i32 as i64);
                        self.mach = (r >> 32) as u32;
                        self.macl = r as u32;
                    }
                    0xE => {
                        // ADDC
                        let t = self.t() as u32;
                        let tmp1 = a.wrapping_add(b);
                        let res = tmp1.wrapping_add(t);
                        self.r[n] = res;
                        self.set_t(tmp1 < a || res < tmp1);
                    }
                    0xF => {
                        let (res, ov) = (a as i32).overflowing_add(b as i32);
                        self.r[n] = res as u32;
                        self.set_t(ov);
                    }
                    _ => return self.unknown(op, pc),
                }
            }
            0x4 => return self.exec4(op, pc, n, m),
            0x5 => {
                let d = (op & 0xF) as u32;
                self.r[n] = self.read32(self.r[m].wrapping_add(d << 2))?;
            }
            0x6 => {
                let b = self.r[m];
                match op & 0xF {
                    0x0 => self.r[n] = self.read8(b)? as i8 as i32 as u32,
                    0x1 => self.r[n] = self.read16(b)? as i16 as i32 as u32,
                    0x2 => self.r[n] = self.read32(b)?,
                    0x3 => self.r[n] = b,
                    0x4 => {
                        let v = self.read8(b)? as i8 as i32 as u32;
                        if n != m {
                            self.r[m] = b.wrapping_add(1);
                        }
                        self.r[n] = v;
                    }
                    0x5 => {
                        let v = self.read16(b)? as i16 as i32 as u32;
                        if n != m {
                            self.r[m] = b.wrapping_add(2);
                        }
                        self.r[n] = v;
                    }
                    0x6 => {
                        let v = self.read32(b)?;
                        if n != m {
                            self.r[m] = b.wrapping_add(4);
                        }
                        self.r[n] = v;
                    }
                    0x7 => self.r[n] = !b,
                    0x8 => self.r[n] = (b & 0xFFFF_0000) | (b & 0xFF) << 8 | (b >> 8) & 0xFF,
                    0x9 => self.r[n] = b.rotate_left(16),
                    0xA => {
                        // NEGC
                        let t = self.t() as u32;
                        let tmp = 0u32.wrapping_sub(b);
                        let res = tmp.wrapping_sub(t);
                        self.r[n] = res;
                        self.set_t(0 < tmp || tmp < res);
                    }
                    0xB => self.r[n] = 0u32.wrapping_sub(b),
                    0xC => self.r[n] = b & 0xFF,
                    0xD => self.r[n] = b & 0xFFFF,
                    0xE => self.r[n] = b as i8 as i32 as u32,
                    0xF => self.r[n] = b as i16 as i32 as u32,
                    _ => unreachable!(),
                }
            }
            0x7 => self.r[n] = self.r[n].wrapping_add(simm8),
            0x8 => {
                let d4 = (op & 0xF) as u32;
                match n {
                    0x0 => self.write8(self.r[m].wrapping_add(d4), self.r[0] as u8)?,
                    0x1 => self.write16(self.r[m].wrapping_add(d4 << 1), self.r[0] as u16)?,
                    0x4 => self.r[0] = self.read8(self.r[m].wrapping_add(d4))? as i8 as i32 as u32,
                    0x5 => self.r[0] = self.read16(self.r[m].wrapping_add(d4 << 1))? as i16 as i32 as u32,
                    0x8 => {
                        let t = self.r[0] == simm8;
                        self.set_t(t);
                    }
                    0x9 => {
                        if self.t() {
                            return Ok(Flow::Jump(pc.wrapping_add(4).wrapping_add(simm8 << 1)));
                        }
                    }
                    0xB => {
                        if !self.t() {
                            return Ok(Flow::Jump(pc.wrapping_add(4).wrapping_add(simm8 << 1)));
                        }
                    }
                    0xD => {
                        if self.t() {
                            return Ok(Flow::Delayed(pc.wrapping_add(4).wrapping_add(simm8 << 1)));
                        }
                    }
                    0xF => {
                        if !self.t() {
                            return Ok(Flow::Delayed(pc.wrapping_add(4).wrapping_add(simm8 << 1)));
                        }
                    }
                    0x2 | 0xC | 0xE => {} // DSP SETRC #imm / LDRS / LDRE
                    _ => return self.unknown(op, pc),
                }
            }
            0x9 => {
                self.r[n] = self.read16(pc.wrapping_add(4).wrapping_add(imm8 << 1))? as i16 as i32 as u32;
            }
            0xA | 0xB => {
                let d = ((op & 0xFFF) as u32) << 20;
                let d = ((d as i32) >> 19) as u32; // sign-extend 12 bits, times 2
                if op >> 12 == 0xB {
                    self.pr = pc.wrapping_add(4);
                }
                return Ok(Flow::Delayed(pc.wrapping_add(4).wrapping_add(d)));
            }
            0xC => {
                let g = self.gbr;
                match n {
                    0x0 => self.write8(g.wrapping_add(imm8), self.r[0] as u8)?,
                    0x1 => self.write16(g.wrapping_add(imm8 << 1), self.r[0] as u16)?,
                    0x2 => self.write32(g.wrapping_add(imm8 << 2), self.r[0])?,
                    0x3 => return Err(Exc::Trap(imm8)),
                    0x4 => self.r[0] = self.read8(g.wrapping_add(imm8))? as i8 as i32 as u32,
                    0x5 => self.r[0] = self.read16(g.wrapping_add(imm8 << 1))? as i16 as i32 as u32,
                    0x6 => self.r[0] = self.read32(g.wrapping_add(imm8 << 2))?,
                    0x7 => self.r[0] = Self::disp_pc_l(pc, imm8),
                    0x8 => {
                        let t = self.r[0] & imm8 == 0;
                        self.set_t(t);
                    }
                    0x9 => self.r[0] &= imm8,
                    0xA => self.r[0] ^= imm8,
                    0xB => self.r[0] |= imm8,
                    0xC..=0xF => {
                        let a = g.wrapping_add(self.r[0]);
                        let v = self.read8(a)? as u32;
                        match n {
                            0xC => self.set_t(v & imm8 == 0),
                            0xD => self.write8(a, (v & imm8) as u8)?,
                            0xE => self.write8(a, (v ^ imm8) as u8)?,
                            _ => self.write8(a, (v | imm8) as u8)?,
                        }
                    }
                    _ => unreachable!(),
                }
            }
            0xD => self.r[n] = self.read32(Self::disp_pc_l(pc, imm8))?,
            0xE => self.r[n] = simm8,
            _ => return self.unknown(op, pc),
        }
        Ok(Flow::Next)
    }

    fn exec4(&mut self, op: u16, pc: u32, n: usize, m: usize) -> Result<Flow, Exc> {
        match op & 0xF {
            0xC => {
                // SHAD
                let s = self.r[m] as i32;
                let v = self.r[n];
                self.r[n] = if s >= 0 {
                    v << (s & 31)
                } else if s & 31 == 0 {
                    if (v as i32) < 0 { 0xFFFF_FFFF } else { 0 }
                } else {
                    ((v as i32) >> ((!s & 31) + 1)) as u32
                };
                return Ok(Flow::Next);
            }
            0xD => {
                // SHLD
                let s = self.r[m] as i32;
                let v = self.r[n];
                self.r[n] = if s >= 0 {
                    v << (s & 31)
                } else if s & 31 == 0 {
                    0
                } else {
                    v >> ((!s & 31) + 1)
                };
                return Ok(Flow::Next);
            }
            0xF => {
                // MAC.W @Rm+,@Rn+
                let a = self.read16(self.r[n])? as i16 as i64;
                self.r[n] = self.r[n].wrapping_add(2);
                let b = self.read16(self.r[m])? as i16 as i64;
                self.r[m] = self.r[m].wrapping_add(2);
                if self.sr & SR_S != 0 {
                    let r = (self.macl as i32 as i64) + a * b;
                    if r > i32::MAX as i64 {
                        self.macl = 0x7FFF_FFFF;
                        self.mach |= 1;
                    } else if r < i32::MIN as i64 {
                        self.macl = 0x8000_0000;
                        self.mach |= 1;
                    } else {
                        self.macl = r as u32;
                    }
                } else {
                    let mac = ((self.mach as u64) << 32 | self.macl as u64) as i64;
                    let r = mac.wrapping_add(a * b);
                    self.mach = (r >> 32) as u32;
                    self.macl = r as u32;
                }
                return Ok(Flow::Next);
            }
            _ => {}
        }
        let low = op & 0xFF;
        // Banked register transfers: STC.L Rm_BANK,@-Rn / LDC.L @Rm+,Rn_BANK / LDC Rm,Rn_BANK
        if low & 0x8F == 0x83 {
            let a = self.r[n].wrapping_sub(4);
            self.write32(a, self.bank[((op >> 4) & 7) as usize])?;
            self.r[n] = a;
            return Ok(Flow::Next);
        }
        if low & 0x8F == 0x87 {
            let v = self.read32(self.r[n])?;
            self.r[n] = self.r[n].wrapping_add(4);
            self.bank[((op >> 4) & 7) as usize] = v;
            return Ok(Flow::Next);
        }
        if low & 0x8F == 0x8E {
            self.bank[((op >> 4) & 7) as usize] = self.r[n];
            return Ok(Flow::Next);
        }
        let v = self.r[n];
        match low {
            0x00 | 0x20 => {
                self.set_t(v >> 31 != 0);
                self.r[n] = v << 1;
            }
            0x01 => {
                self.set_t(v & 1 != 0);
                self.r[n] = v >> 1;
            }
            0x21 => {
                self.set_t(v & 1 != 0);
                self.r[n] = ((v as i32) >> 1) as u32;
            }
            0x04 => {
                self.set_t(v >> 31 != 0);
                self.r[n] = v.rotate_left(1);
            }
            0x05 => {
                self.set_t(v & 1 != 0);
                self.r[n] = v.rotate_right(1);
            }
            0x24 => {
                let t = self.t() as u32;
                self.set_t(v >> 31 != 0);
                self.r[n] = v << 1 | t;
            }
            0x25 => {
                let t = self.t() as u32;
                self.set_t(v & 1 != 0);
                self.r[n] = v >> 1 | t << 31;
            }
            0x08 => self.r[n] = v << 2,
            0x09 => self.r[n] = v >> 2,
            0x18 => self.r[n] = v << 8,
            0x19 => self.r[n] = v >> 8,
            0x28 => self.r[n] = v << 16,
            0x29 => self.r[n] = v >> 16,
            0x10 => {
                let r = v.wrapping_sub(1);
                self.r[n] = r;
                self.set_t(r == 0);
            }
            0x11 => self.set_t(v as i32 >= 0),
            0x15 => self.set_t(v as i32 > 0),
            0x1B => {
                // TAS.B @Rn
                let b = self.read8(v)?;
                self.set_t(b == 0);
                self.write8(v, b | 0x80)?;
            }
            0x0B => {
                // JSR @Rn
                self.pr = pc.wrapping_add(4);
                return Ok(Flow::Delayed(v));
            }
            0x2B => return Ok(Flow::Delayed(v)), // JMP @Rn
            // STS.L / STC.L xx,@-Rn
            0x02 | 0x12 | 0x22 | 0x03 | 0x13 | 0x23 | 0x33 | 0x43 | 0x32 | 0xF2 | 0x52 | 0x62 => {
                let val = match low {
                    0x02 => self.mach,
                    0x12 => self.macl,
                    0x22 => self.pr,
                    0x03 => self.sr,
                    0x13 => self.gbr,
                    0x23 => self.vbr,
                    0x33 => self.ssr,
                    0x43 => self.spc,
                    0x32 => self.sgr,
                    0xF2 => self.dbr,
                    _ => 0, // FPUL/FPSCR (no FPU)
                };
                let a = v.wrapping_sub(4);
                self.write32(a, val)?;
                self.r[n] = a;
            }
            // LDS.L / LDC.L @Rm+,xx  (register number is in the n field)
            0x06 | 0x16 | 0x26 | 0x07 | 0x17 | 0x27 | 0x37 | 0x47 | 0x36 | 0xF6 | 0x56 | 0x66 => {
                let val = self.read32(v)?;
                self.r[n] = v.wrapping_add(4);
                match low {
                    0x06 => self.mach = val,
                    0x16 => self.macl = val,
                    0x26 => self.pr = val,
                    0x07 => self.set_sr(val),
                    0x17 => self.gbr = val,
                    0x27 => self.vbr = val,
                    0x37 => self.ssr = val,
                    0x47 => self.spc = val,
                    0x36 => self.sgr = val,
                    0xF6 => self.dbr = val,
                    _ => {}
                }
            }
            // LDS / LDC Rm,xx
            0x0A => self.mach = v,
            0x1A => self.macl = v,
            0x2A => self.pr = v,
            0x0E => self.set_sr(v),
            0x1E => self.gbr = v,
            0x2E => self.vbr = v,
            0x3E => self.ssr = v,
            0x4E => self.spc = v,
            0x3A => self.sgr = v,
            0xFA => self.dbr = v,
            0x5A | 0x6A | 0x5E | 0x6E | 0x7E | 0x14 => {} // FPUL/FPSCR, DSP MOD/RS/RE, SETRC
            0xA9 => self.r[0] = self.read_unaligned32(v)?, // MOVUA.L @Rm,R0
            0xE9 => {
                self.r[0] = self.read_unaligned32(v)?;
                if n != 0 {
                    self.r[n] = v.wrapping_add(4);
                }
            }
            _ => return self.unknown(op, pc),
        }
        Ok(Flow::Next)
    }

    fn read_unaligned32(&mut self, a: u32) -> Result<u32, Exc> {
        let mut v = 0u32;
        for i in 0..4 {
            v = v << 8 | self.read8(a.wrapping_add(i))? as u32;
        }
        Ok(v)
    }

    fn div1(&mut self, n: usize, m: usize) {
        let old_q = self.sr & SR_Q != 0;
        let mbit = self.sr & SR_M != 0;
        let mut q = self.r[n] >> 31 != 0;
        let rm = self.r[m];
        let t = self.t() as u32;
        let mut rn = self.r[n] << 1 | t;
        let tmp0 = rn;
        let tmp1;
        if old_q == mbit {
            rn = rn.wrapping_sub(rm);
            tmp1 = rn > tmp0;
        } else {
            rn = rn.wrapping_add(rm);
            tmp1 = rn < tmp0;
        }
        q = match (old_q, mbit) {
            (false, false) | (true, false) => {
                if !q { tmp1 } else { !tmp1 }
            }
            _ => {
                if !q { !tmp1 } else { tmp1 }
            }
        };
        self.r[n] = rn;
        self.sr = (self.sr & !(SR_Q | SR_T)) | (q as u32) << 8 | ((q == mbit) as u32);
    }

    fn unknown(&mut self, op: u16, pc: u32) -> Result<Flow, Exc> {
        self.unknown_ops += 1;
        if self.unknown_ops < 20 {
            eprintln!("[cpu] unimplemented opcode {:04x} at {:08x}", op, pc);
        }
        Err(Exc::Illegal(op))
    }
}

// --- snapshots ---

use crate::state::{Bad, Snap, R, W};

impl Snap for Cpu {
    fn save(&self, w: &mut W) {
        for v in self.r.iter().chain(self.bank.iter()) {
            w.u32(*v);
        }
        for v in [self.sr, self.gbr, self.vbr, self.ssr, self.spc, self.sgr, self.dbr, self.mach, self.macl, self.pr, self.pc] {
            w.u32(v);
        }
        w.bool(self.ldst);
        w.bool(self.sleeping);
        w.u64(self.cycles);
        w.u64(self.instructions);
        for e in &self.utlb {
            w.u32(e.vpn);
            w.u8(e.asid);
            w.u32(e.ppn);
            w.u8(e.sz);
            w.u8(e.pr);
            for b in [e.v, e.d, e.sh, e.c, e.wt] {
                w.bool(b);
            }
        }
        self.bus.save(w);
    }

    fn load(&mut self, r: &mut R) -> Result<(), Bad> {
        for i in 0..16 {
            self.r[i] = r.u32()?;
        }
        for i in 0..8 {
            self.bank[i] = r.u32()?;
        }
        for f in [
            &mut self.sr, &mut self.gbr, &mut self.vbr, &mut self.ssr, &mut self.spc, &mut self.sgr,
            &mut self.dbr, &mut self.mach, &mut self.macl, &mut self.pr, &mut self.pc,
        ] {
            *f = r.u32()?;
        }
        self.ldst = r.bool()?;
        self.sleeping = r.bool()?;
        self.cycles = r.u64()?;
        self.instructions = r.u64()?;
        for e in self.utlb.iter_mut() {
            e.vpn = r.u32()?;
            e.asid = r.u8()?;
            e.ppn = r.u32()?;
            e.sz = r.u8()?;
            e.pr = r.u8()?;
            e.v = r.bool()?;
            e.d = r.bool()?;
            e.sh = r.bool()?;
            e.c = r.bool()?;
            e.wt = r.bool()?;
        }
        self.bus.load(r)
    }
}
