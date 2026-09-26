//! RITE 0400 binary reader (mruby 4.1.0 `src/load.c`).
//!
//! Layout (all integers big-endian):
//! - binary header: `"RITE" "0400" size:u32 compiler_name[4] compiler_version[4]`
//! - sections: `ident[4] size:u32 ...` repeated until `"END\0"`
//!   - `"IREP"`: `rite_version[4]` then one irep record tree
//!   - `"DBG\0"`: line numbers (`Irep::lines`), `"LVAR"`: local variable names
//! - irep record: `size:u32 nlocals:u16 nregs:u16 rlen:u16 clen:u16 ilen:u32`
//!   `iseq[ilen] catch[clen*13] plen:u16 pool... slen:u16 syms...` then `rlen` child records

use alloc::{format, string::String, vec::Vec};

use crate::error::{VmError, VmResult};

#[derive(Clone, Debug, PartialEq)]
pub enum Pool {
    /// `IREP_TT_STR` / `IREP_TT_SSTR`: bytes without the trailing NUL.
    Str(Vec<u8>),
    /// `IREP_TT_INT32` / `IREP_TT_INT64`
    Int(i64),
    /// `IREP_TT_BIGINT`: the digits and the base they are written in. The base is
    /// negative for a negative number (`mrb_bint_new_str`).
    BigInt { base: i8, digits: Vec<u8> },
    /// `IREP_TT_FLOAT`
    Float(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchType {
    Rescue = 0,
    Ensure = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatchHandler {
    pub kind: CatchType,
    /// The handler covers instructions with `begin < pc <= end` where `pc`
    /// already points at the next instruction (mruby `catch_handler_find`).
    pub begin: u32,
    pub end: u32,
    pub target: u32,
}

/// One irep (instruction sequence + its constants), flattened: children are
/// indices into [`Rite::ireps`].
#[derive(Clone, Debug, Default)]
pub struct Irep {
    pub nlocals: u16,
    pub nregs: u16,
    pub iseq: Vec<u8>,
    pub catch: Vec<CatchHandler>,
    pub pool: Vec<Pool>,
    /// Symbol names (raw bytes). `None` is mruby's NULL symbol.
    pub syms: Vec<Option<Vec<u8>>>,
    pub reps: Vec<usize>,
    /// Local variable names for R1.. (from the LVAR section), if present.
    pub lv: Vec<Option<Vec<u8>>>,
    /// `(start_pc, line)` in ascending `start_pc`, from the DBG section (`mrbc -g`).
    /// A line covers the instructions from its `start_pc` to the next entry.
    pub lines: Vec<(u32, u32)>,
    /// File name of the first debug entry of this irep, if the DBG section has one.
    pub filename: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct Rite {
    pub compiler_name: [u8; 4],
    pub compiler_version: [u8; 4],
    pub ireps: Vec<Irep>,
    /// Index of the top-level irep.
    pub root: usize,
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn need(&self, n: usize) -> VmResult<()> {
        if self.p + n > self.b.len() {
            Err(VmError::Rite(format!("unexpected end at {} (need {} bytes)", self.p, n)))
        } else {
            Ok(())
        }
    }
    fn u8(&mut self) -> VmResult<u8> {
        self.need(1)?;
        let v = self.b[self.p];
        self.p += 1;
        Ok(v)
    }
    fn u16(&mut self) -> VmResult<u16> {
        self.need(2)?;
        let v = u16::from_be_bytes([self.b[self.p], self.b[self.p + 1]]);
        self.p += 2;
        Ok(v)
    }
    fn u32(&mut self) -> VmResult<u32> {
        self.need(4)?;
        let v = u32::from_be_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        Ok(v)
    }
    fn bytes(&mut self, n: usize) -> VmResult<&'a [u8]> {
        self.need(n)?;
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
}

pub fn parse(bin: &[u8]) -> VmResult<Rite> {
    let mut c = Cur { b: bin, p: 0 };
    if c.bytes(4)? != b"RITE" {
        return Err(VmError::Rite("bad identifier (expected RITE)".into()));
    }
    let ver = c.bytes(4)?;
    if ver != b"0400" {
        return Err(VmError::Rite(format!(
            "unsupported binary version {} (expected 0400)",
            String::from_utf8_lossy(ver)
        )));
    }
    let total = c.u32()? as usize;
    if total > bin.len() {
        return Err(VmError::Rite(format!("size {} exceeds buffer {}", total, bin.len())));
    }
    let mut compiler_name = [0u8; 4];
    compiler_name.copy_from_slice(c.bytes(4)?);
    let mut compiler_version = [0u8; 4];
    compiler_version.copy_from_slice(c.bytes(4)?);

    let mut ireps: Vec<Irep> = Vec::new();
    let mut root: Option<usize> = None;
    while c.p + 8 <= total {
        let ident = c.bytes(4)?;
        let size = c.u32()? as usize;
        if size < 8 {
            return Err(VmError::Rite("section size too small".into()));
        }
        let start = c.p - 8;
        let end = start + size;
        if end > bin.len() {
            return Err(VmError::Rite("section exceeds buffer".into()));
        }
        match ident {
            b"IREP" => {
                let _rite_version = c.bytes(4)?;
                let mut sub = Cur { b: &bin[..end], p: c.p };
                let r = read_record(&mut sub, &mut ireps)?;
                root = Some(r);
            }
            b"DBG\0" => {
                if let Some(r) = root {
                    let mut sub = Cur { b: &bin[..end], p: c.p };
                    read_debug(&mut sub, &mut ireps, r).ok(); // best effort, as LVAR
                }
            }
            b"LVAR" => {
                if let Some(r) = root {
                    let mut sub = Cur { b: &bin[..end], p: c.p };
                    read_lvar(&mut sub, &mut ireps, r).ok(); // best effort
                }
            }
            b"END\0" => break,
            other => {
                return Err(VmError::Rite(format!(
                    "unknown section {:?}",
                    String::from_utf8_lossy(other)
                )));
            }
        }
        c.p = end;
    }
    let root = root.ok_or_else(|| VmError::Rite("no IREP section".into()))?;
    Ok(Rite { compiler_name, compiler_version, ireps, root })
}

/// Reads one record and (recursively) its children; returns its index.
fn read_record(c: &mut Cur, out: &mut Vec<Irep>) -> VmResult<usize> {
    let _record_size = c.u32()?;
    let nlocals = c.u16()?;
    let nregs = c.u16()?;
    // A frame has `nregs` registers and its locals are the first of them, so a record with
    // more locals than registers describes a frame that cannot exist (`OP_ENTER` clears up to
    // `nlocals`). The compiler never writes one; mruby 4.1.0-rc2 refuses it here, where the
    // numbers arrive (`load.c`, 7ed51715e, GHSA-pmm3-g676-wxm7).
    if nlocals > nregs {
        return Err(VmError::Rite(format!("irep record with {nlocals} locals but {nregs} registers")));
    }
    let rlen = c.u16()?;
    let clen = c.u16()?;
    let ilen = c.u32()? as usize;
    let iseq = c.bytes(ilen)?.to_vec();
    let mut catch = Vec::with_capacity(clen as usize);
    for _ in 0..clen {
        let t = c.u8()?;
        let kind = match t {
            0 => CatchType::Rescue,
            1 => CatchType::Ensure,
            _ => return Err(VmError::Rite(format!("bad catch type {t}"))),
        };
        let begin = c.u32()?;
        let end = c.u32()?;
        let target = c.u32()?;
        catch.push(CatchHandler { kind, begin, end, target });
    }
    let plen = c.u16()?;
    let mut pool = Vec::with_capacity(plen as usize);
    for _ in 0..plen {
        let tt = c.u8()?;
        match tt {
            1 => pool.push(Pool::Int(c.u32()? as i32 as i64)),
            3 => {
                let hi = c.u32()? as u64;
                let lo = c.u32()? as u64;
                pool.push(Pool::Int(((hi << 32) | lo) as i64));
            }
            7 => {
                // `load.c`: `pool_data_len = len + 2` counts the length byte and the base
                // byte, so `len + 1` bytes follow the length byte.
                let len = c.u8()? as usize;
                let base = c.u8()? as i8;
                pool.push(Pool::BigInt { base, digits: c.bytes(len)?.to_vec() });
            }
            5 => {
                let raw: [u8; 8] = c.bytes(8)?.try_into().unwrap();
                pool.push(Pool::Float(f64::from_le_bytes(raw)));
            }
            0 => {
                let len = c.u16()? as usize;
                let s = c.bytes(len)?.to_vec();
                let _nul = c.u8()?;
                pool.push(Pool::Str(s));
            }
            _ => return Err(VmError::Rite(format!("bad pool type {tt}"))),
        }
    }
    let slen = c.u16()?;
    let mut syms = Vec::with_capacity(slen as usize);
    for _ in 0..slen {
        let n = c.u16()?;
        if n == 0xFFFF {
            syms.push(None);
            continue;
        }
        let s = c.bytes(n as usize)?.to_vec();
        let _nul = c.u8()?;
        syms.push(Some(s));
    }
    let idx = out.len();
    out.push(Irep { nlocals, nregs, iseq, catch, pool, syms, reps: Vec::new(), lv: Vec::new(), lines: Vec::new(), filename: None });
    let mut reps = Vec::with_capacity(rlen as usize);
    for _ in 0..rlen {
        reps.push(read_record(c, out)?);
    }
    out[idx].reps = reps;
    Ok(idx)
}

/// `"DBG\0"` (mruby `read_section_debug` / `read_debug_record`): the file name table, then one
/// record per irep in pre-order. Only the first file of a record is kept: `mrbc` writes one
/// file per irep unless a program is built from several sources.
fn read_debug(c: &mut Cur, ireps: &mut [Irep], root: usize) -> VmResult<()> {
    let flen = c.u16()? as usize;
    let mut filenames = Vec::with_capacity(flen);
    for _ in 0..flen {
        let n = c.u16()? as usize;
        filenames.push(c.bytes(n)?.to_vec());
    }
    fn rec(c: &mut Cur, ireps: &mut [Irep], i: usize, filenames: &[Vec<u8>]) -> VmResult<()> {
        let _record_size = c.u32()?;
        let files = c.u16()?;
        for f in 0..files {
            let _start_pos = c.u32()?;
            let filename_idx = c.u16()? as usize;
            let entries = c.u32()? as usize;
            let line_type = c.u8()?;
            let mut lines: Vec<(u32, u32)> = Vec::new();
            match line_type {
                // mrb_debug_line_ary: one line per instruction byte
                0 => for pc in 0..entries {
                    let line = c.u16()? as u32;
                    if lines.last().map(|e| e.1) != Some(line) { lines.push((pc as u32, line)); }
                }
                // mrb_debug_line_flat_map: (start_pos, line) pairs
                1 => for _ in 0..entries {
                    let start = c.u32()?;
                    let line = c.u16()? as u32;
                    lines.push((start, line));
                }
                // mrb_debug_line_packed_map: variable-length (pos_diff, line_diff) pairs
                2 => {
                    let bytes = c.bytes(entries)?.to_vec();
                    // the writer encodes the differences as wrapping u32 (mruby
                    // `mrc_debug_info_append_file`), so a line that goes backwards comes
                    // back as a large value: add the same way
                    let (mut at, mut pos, mut line) = (0usize, 0u32, 0u32);
                    while at < bytes.len() {
                        pos = pos.wrapping_add(packed_int(&bytes, &mut at));
                        line = line.wrapping_add(packed_int(&bytes, &mut at));
                        lines.push((pos, line));
                    }
                }
                other => return Err(VmError::Rite(format!("unknown debug line type {other}"))),
            }
            if f == 0 {
                ireps[i].lines = lines;
                ireps[i].filename = filenames.get(filename_idx).cloned();
            }
        }
        let reps = ireps[i].reps.clone();
        for r in reps {
            rec(c, ireps, r, filenames)?;
        }
        Ok(())
    }
    rec(c, ireps, root, &filenames)
}

/// `mrb_packed_int_decode`: 7 bits per byte, the top bit says "more follow".
fn packed_int(b: &[u8], at: &mut usize) -> u32 {
    let (mut n, mut shift) = (0u32, 0u32);
    while *at < b.len() {
        let byte = b[*at];
        *at += 1;
        n |= ((byte & 0x7f) as u32) << shift;
        shift += 7;
        if byte & 0x80 == 0 || shift >= 32 { break; }
    }
    n
}

fn read_lvar(c: &mut Cur, ireps: &mut [Irep], root: usize) -> VmResult<()> {
    // syms table: slen:u32 then (len:u16 name) ... ; then per-irep records (pre-order)
    // (`write_lv_sym_table`: the count is 32-bit, each name length 16-bit)
    let slen = c.u32()? as usize;
    let mut names = Vec::with_capacity(slen);
    for _ in 0..slen {
        let n = c.u16()? as usize;
        names.push(c.bytes(n)?.to_vec());
    }
    fn rec(c: &mut Cur, ireps: &mut [Irep], i: usize, names: &[Vec<u8>]) -> VmResult<()> {
        let nlocals = ireps[i].nlocals as usize;
        let mut lv = Vec::new();
        for _ in 1..nlocals {
            let idx = c.u16()?;
            if idx == 0xFFFF {
                lv.push(None);
            } else {
                lv.push(names.get(idx as usize).cloned());
            }
        }
        ireps[i].lv = lv;
        let reps = ireps[i].reps.clone();
        for r in reps {
            rec(c, ireps, r, names)?;
        }
        Ok(())
    }
    rec(c, ireps, root, &names)
}

impl Irep {
    /// The source line of the instruction at `pc` (mruby `mrb_debug_get_line`): the line of the
    /// last entry whose `start_pc` is at or before `pc`. `None` without a DBG section.
    pub fn line_of(&self, pc: usize) -> Option<u32> {
        let pc = pc as u32;
        match self.lines.partition_point(|(start, _)| *start <= pc) {
            0 => None,
            i => Some(self.lines[i - 1].1),
        }
    }

    /// Decodes the instruction at `pc` for dumping/debugging: returns
    /// (opcode, a, b, c, next_pc). Handles EXT1..EXT3 like `mrbc --verbose`.
    pub fn decode(&self, pc: usize) -> Option<(crate::opcode::Op, u32, u32, u32, usize)> {
        use crate::opcode::{Op, Operands};
        let mut p = pc;
        let mut op = Op::from_u8(*self.iseq.get(p)?)?;
        p += 1;
        let mut ext = 0u8;
        while matches!(op, Op::Ext1 | Op::Ext2 | Op::Ext3) {
            ext = match op {
                Op::Ext1 => 1,
                Op::Ext2 => 2,
                _ => 3,
            };
            op = Op::from_u8(*self.iseq.get(p)?)?;
            p += 1;
        }
        let rd_b = |p: &mut usize| -> Option<u32> {
            let v = *self.iseq.get(*p)? as u32;
            *p += 1;
            Some(v)
        };
        let rd_s = |p: &mut usize| -> Option<u32> {
            let v = ((*self.iseq.get(*p)? as u32) << 8) | (*self.iseq.get(*p + 1)? as u32);
            *p += 2;
            Some(v)
        };
        let rd_w = |p: &mut usize| -> Option<u32> {
            let v = ((*self.iseq.get(*p)? as u32) << 16)
                | ((*self.iseq.get(*p + 1)? as u32) << 8)
                | (*self.iseq.get(*p + 2)? as u32);
            *p += 3;
            Some(v)
        };
        let (mut a, mut b, mut c) = (0, 0, 0);
        let a_wide = ext == 1 || ext == 3;
        let b_wide = ext == 2 || ext == 3;
        match op.operands() {
            Operands::Z => {}
            Operands::B => a = if a_wide { rd_s(&mut p)? } else { rd_b(&mut p)? },
            Operands::BB => {
                a = if a_wide { rd_s(&mut p)? } else { rd_b(&mut p)? };
                b = if b_wide { rd_s(&mut p)? } else { rd_b(&mut p)? };
            }
            Operands::BBB => {
                a = if a_wide { rd_s(&mut p)? } else { rd_b(&mut p)? };
                b = if b_wide { rd_s(&mut p)? } else { rd_b(&mut p)? };
                c = rd_b(&mut p)?;
            }
            Operands::BS => {
                a = if a_wide { rd_s(&mut p)? } else { rd_b(&mut p)? };
                b = rd_s(&mut p)?;
            }
            Operands::BSS => {
                a = if a_wide { rd_s(&mut p)? } else { rd_b(&mut p)? };
                b = rd_s(&mut p)?;
                c = rd_s(&mut p)?;
            }
            Operands::S => a = rd_s(&mut p)?,
            Operands::W => a = rd_w(&mut p)?,
        }
        Some((op, a, b, c, p))
    }
}
