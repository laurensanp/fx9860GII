//! Machine snapshots: the complete emulator state (CPU, RAM, peripherals),
//! so closing the emulator works like switching the calculator off and the
//! next start resumes exactly where it was. Flash is stored separately.

pub const MAGIC: &[u8; 8] = b"FXEMUSN1";

#[derive(Default)]
pub struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn bool(&mut self, v: bool) {
        self.0.push(v as u8);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
}

pub struct R<'a> {
    d: &'a [u8],
    p: usize,
}

#[derive(Debug)]
pub struct Bad;

impl<'a> R<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        R { d, p: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Bad> {
        let s = self.d.get(self.p..self.p + n).ok_or(Bad)?;
        self.p += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, Bad> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool, Bad> {
        Ok(self.u8()? != 0)
    }
    pub fn u16(&mut self) -> Result<u16, Bad> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, Bad> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, Bad> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// Read a length-prefixed byte block into `out`, which must have that exact length.
    pub fn bytes_into(&mut self, out: &mut [u8]) -> Result<(), Bad> {
        let n = self.u32()? as usize;
        if n != out.len() {
            return Err(Bad);
        }
        out.copy_from_slice(self.take(n)?);
        Ok(())
    }
}

/// Things that can be saved into / restored from a snapshot.
pub trait Snap {
    fn save(&self, w: &mut W);
    fn load(&mut self, r: &mut R) -> Result<(), Bad>;
}
