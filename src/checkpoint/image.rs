//! Reading a checkpoint image into plain records, before anything is built in the VM.
//!
//! The layout is the format the image's writer documents (format v0 with the additions made
//! for this reader: a permanent Proc whose irep is carried, record 15, and a permanent irep
//! with its content, irep record 2). Every number is big-endian; a reference to a symbol, an
//! irep or an object is a `u32` index from 1 into its own table, 0 meaning "none".

use alloc::{format, string::String, vec::Vec};

/// A value as the image writes it. References stay indices until the objects exist.
#[derive(Clone, Copy, Debug)]
pub enum Val {
    Nil,
    False,
    True,
    Int(i64),
    Float(f64),
    Sym(u32),
    Obj(u32),
    /// The writer's "no value" (`MRB_TT_UNDEF`), seen only in registers.
    Undef,
}

/// One entry of an irep's constant pool.
pub enum PoolRec {
    Str(Vec<u8>),
    Int(i64),
    Float(f64),
    /// The writer's packing of a big integer: length, base, digits.
    BigInt(Vec<u8>),
}

/// An irep record: kind 0 (carried), 1 (permanent, by hash only) or 2 (permanent, carried).
pub struct IrepRec {
    pub kind: u8,
    pub hash: u64,
    pub nlocals: u16,
    pub nregs: u16,
    pub clen: u16,
    pub iseq: Vec<u8>,
    /// The `13 * clen` bytes of catch handlers that follow the instructions.
    pub catch: Vec<u8>,
    pub pool: Vec<PoolRec>,
    pub syms: Vec<u32>,
    pub reps: Vec<u32>,
    pub lv: Vec<u32>,
}

/// The common head of a non-permanent object: its class and whether it is frozen.
#[derive(Clone, Copy)]
pub struct Head {
    pub class: u32,
    pub frozen: bool,
}

pub enum MethBody {
    Proc(u32),
    /// The C function's name and its argument spec. The spec is not used: the method here is
    /// this VM's own of that name, with its own spec.
    Func(String, #[allow(dead_code)] u32),
    Undef,
}

pub struct MethRec {
    pub name: u32,
    pub vis: u8,
    pub body: MethBody,
}

pub enum ProcBody {
    Irep(u32),
    Func(String),
    /// alias_method of a Ruby method: the original's name (the Proc's upper is the original)
    #[allow(dead_code)] // the name is not needed: the upper Proc is the method
    Alias(u32),
}

pub enum EnvPlace {
    /// Attached: the window starts at this index of the stack.
    Stack(u32),
    /// Detached: the values themselves.
    Values(Vec<Val>),
}

pub type Ivars = Vec<(u32, Val)>;

pub enum ObjRec {
    Perm(String),
    /// A permanent Proc a frame runs, with the irep the frame runs (carried).
    PermBody(String, u32),
    Object(Head, Ivars),
    Str(Head, Vec<u8>),
    BinStr(Head, Vec<u8>),
    Array(Head, Vec<Val>),
    Hash { head: Head, flags: u8, pairs: Vec<(Val, Val)>, ivars: Ivars },
    Range { head: Head, begin: Val, end: Val, excl: bool },
    Exception { head: Head, mesg: Val, bt: Val, ivars: Ivars },
    /// Class (8), module (9) or singleton class (10).
    /// `inst` (what the class allocates) is not used: a class here inherits that from its
    /// superclass, which gives the same for every kind the writer carries.
    Class { head: Head, kind: u8, sup: u32, #[allow(dead_code)] inst: u8, cflags: u8, ivars: Ivars, methods: Vec<MethRec> },
    /// An include class: `head.class` is the module it stands for.
    IClass { head: Head, sup: u32 },
    Proc { head: Head, flags: u8, body: ProcBody, upper: u32, env_kind: u8, env: u32 },
    Env { head: Head, mid: u32, len: u8, bidx: u8, vis: u8, place: EnvPlace },
    Data { head: Head, type_name: String, blob: Vec<u8>, ivars: Ivars },
}

pub enum PatchRec {
    MethodSet(u32, MethRec),
    MethodRemove(u32, u32),
    IvSet(u32, u32, Val),
    IvRemove(u32, u32),
    SuperSet(u32, u32),
    GvSet(u32, Val),
    GvRemove(u32),
    /// change 10: the new contents of a permanent String (`Str`) or Array (`Ary`), e.g. `ARGV << x`
    StrSet(u32, bool, Vec<u8>),
    AryContents(u32, bool, Vec<Val>),
}

pub struct FrameRec {
    pub mid: u32,
    pub proc_: u32,
    pub base: u32,
    pub pc: u32,
    pub n: u8,
    pub nk: u8,
    pub cci: u8,
    pub vis: u8,
    /// The frame's block. Not used: the block is also in the frame's block register, which
    /// is where this VM reads it from.
    #[allow(dead_code)]
    pub blk: u32,
    pub env_kind: u8,
    pub env: u32,
}

pub struct Image {
    pub version: u16,
    pub writer: String,
    pub syms: Vec<Vec<u8>>,
    pub ireps: Vec<IrepRec>,
    pub objs: Vec<ObjRec>,
    pub patches: Vec<PatchRec>,
    pub stack: Vec<Val>,
    pub frames: Vec<FrameRec>,
    /// line numbers of carried ireps (section DBG0): irep number -> (file of its first part,
    /// (pc, line) where the line changes)
    pub lines: Vec<LinesRec>,
}

/// One irep's line numbers: (irep number, file name, (pc, line) where the line changes).
pub type LinesRec = (u32, String, Vec<(u32, u32)>);

/// The format versions this reader takes.
pub const FORMAT_VERSION: u16 = 0;

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

type R<T> = Result<T, String>;

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if self.b.len() - self.p < n { return Err(format!("truncated image at byte {}", self.p)); }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> R<u16> { let s = self.take(2)?; Ok(u16::from_be_bytes([s[0], s[1]])) }
    fn u32(&mut self) -> R<u32> { let s = self.take(4)?; Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]])) }
    fn u64(&mut self) -> R<u64> { let hi = self.u32()? as u64; Ok((hi << 32) | self.u32()? as u64) }
    fn bytes(&mut self) -> R<Vec<u8>> { let n = self.u32()? as usize; Ok(self.take(n)?.to_vec()) }
    fn string(&mut self) -> R<String> {
        let b = self.bytes()?;
        String::from_utf8(b).map_err(|_| String::from("a name in the image is not UTF-8"))
    }
    fn val(&mut self) -> R<Val> {
        Ok(match self.u8()? {
            0 => Val::Nil,
            1 => Val::False,
            2 => Val::True,
            3 => Val::Int(self.u64()? as i64),
            4 => Val::Float(f64::from_bits(self.u64()?)),
            5 => Val::Sym(self.u32()?),
            6 => Val::Obj(self.u32()?),
            7 => Val::Undef,
            t => return Err(format!("bad value tag {t}")),
        })
    }
    fn ivars(&mut self) -> R<Ivars> {
        let n = self.u32()?;
        (0..n).map(|_| Ok((self.u32()?, self.val()?))).collect()
    }
    fn head(&mut self) -> R<Head> {
        Ok(Head { class: self.u32()?, frozen: self.u8()? != 0 })
    }
    fn method(&mut self) -> R<MethRec> {
        let name = self.u32()?;
        let vis = self.u8()?;
        let body = match self.u8()? {
            0 => MethBody::Proc(self.u32()?),
            1 => { let f = self.string()?; MethBody::Func(f, self.u32()?) }
            2 => MethBody::Undef,
            k => return Err(format!("bad method kind {k}")),
        };
        Ok(MethRec { name, vis, body })
    }
    fn irep(&mut self) -> R<IrepRec> {
        let kind = self.u8()?;
        let hash = self.u64()?;
        let mut r = IrepRec { kind, hash, nlocals: 0, nregs: 0, clen: 0, iseq: Vec::new(), catch: Vec::new(), pool: Vec::new(), syms: Vec::new(), reps: Vec::new(), lv: Vec::new() };
        match kind {
            1 => return Ok(r),
            0 | 2 => {}
            k => return Err(format!("bad irep kind {k}")),
        }
        r.nlocals = self.u16()?;
        r.nregs = self.u16()?;
        r.clen = self.u16()?;
        let ilen = self.u32()? as usize;
        r.iseq = self.take(ilen)?.to_vec();
        r.catch = self.take(13 * r.clen as usize)?.to_vec();
        let plen = self.u16()?;
        for _ in 0..plen {
            r.pool.push(match self.u8()? {
                0 => PoolRec::Str(self.bytes()?),
                1 => PoolRec::Int(self.u64()? as i64),
                2 => PoolRec::Float(f64::from_bits(self.u64()?)),
                3 => PoolRec::BigInt(self.bytes()?),
                t => return Err(format!("bad pool entry type {t}")),
            });
        }
        let slen = self.u16()?;
        r.syms = (0..slen).map(|_| self.u32()).collect::<R<_>>()?;
        let rlen = self.u16()?;
        r.reps = (0..rlen).map(|_| self.u32()).collect::<R<_>>()?;
        let nlv = self.u16()?;
        r.lv = (0..nlv).map(|_| self.u32()).collect::<R<_>>()?;
        Ok(r)
    }
    fn object(&mut self) -> R<ObjRec> {
        let kind = self.u8()?;
        if kind == 1 { return Ok(ObjRec::Perm(self.string()?)); }
        if kind == 15 { let n = self.string()?; return Ok(ObjRec::PermBody(n, self.u32()?)); }
        let head = self.head()?;
        Ok(match kind {
            2 => ObjRec::Object(head, self.ivars()?),
            3 => ObjRec::Str(head, self.bytes()?),
            25 => ObjRec::BinStr(head, self.bytes()?),   // a String marked binary (String#b)
            4 => { let n = self.u32()?; ObjRec::Array(head, (0..n).map(|_| self.val()).collect::<R<_>>()?) }
            5 => {
                let flags = self.u8()?;
                let n = self.u32()?;
                let pairs = (0..n).map(|_| Ok((self.val()?, self.val()?))).collect::<R<_>>()?;
                ObjRec::Hash { head, flags, pairs, ivars: self.ivars()? }
            }
            6 => { let begin = self.val()?; let end = self.val()?; ObjRec::Range { head, begin, end, excl: self.u8()? != 0 } }
            7 => { let mesg = self.val()?; let bt = self.val()?; ObjRec::Exception { head, mesg, bt, ivars: self.ivars()? } }
            8..=10 => {
                let sup = self.u32()?;
                let inst = self.u8()?;
                let cflags = self.u8()?;
                let ivars = self.ivars()?;
                let n = self.u32()?;
                let methods = (0..n).map(|_| self.method()).collect::<R<_>>()?;
                ObjRec::Class { head, kind, sup, inst, cflags, ivars, methods }
            }
            11 => ObjRec::IClass { head, sup: self.u32()? },
            12 => {
                let flags = self.u8()?;
                let body = match self.u8()? {
                    0 => ProcBody::Irep(self.u32()?),
                    1 => { let f = self.string()?; self.u32()?; ProcBody::Func(f) }
                    2 => ProcBody::Alias(self.u32()?),
                    b => return Err(format!("bad proc body kind {b}")),
                };
                let upper = self.u32()?;
                let env_kind = self.u8()?;
                let env = self.u32()?;
                ObjRec::Proc { head, flags, body, upper, env_kind, env }
            }
            13 => {
                let mid = self.u32()?;
                let len = self.u8()?;
                let bidx = self.u8()?;
                let vis = self.u8()?;
                let place = if self.u8()? == 1 { EnvPlace::Stack(self.u32()?) } else {
                    EnvPlace::Values((0..len).map(|_| self.val()).collect::<R<_>>()?)
                };
                ObjRec::Env { head, mid, len, bidx, vis, place }
            }
            14 => { let type_name = self.string()?; let blob = self.bytes()?; ObjRec::Data { head, type_name, blob, ivars: self.ivars()? } }
            k @ 16..=24 => return Err(format!("object kind {k} ({}) is not carried to this VM yet", match k {
                16 => "Integer beyond 64 bits", 17 => "Struct", 18 => "Set", 19 => "Rational", 20 => "Complex",
                21 => "break through ensure", 23 => "Random", 24 => "origin of a prepended class", _ => "?",
            })),
            k => return Err(format!("bad object kind {k}")),
        })
    }
    fn patch(&mut self) -> R<PatchRec> {
        Ok(match self.u8()? {
            1 => { let c = self.u32()?; PatchRec::MethodSet(c, self.method()?) }
            2 => PatchRec::MethodRemove(self.u32()?, self.u32()?),
            3 => { let o = self.u32()?; let s = self.u32()?; PatchRec::IvSet(o, s, self.val()?) }
            4 => PatchRec::IvRemove(self.u32()?, self.u32()?),
            5 => PatchRec::SuperSet(self.u32()?, self.u32()?),
            6 => { let s = self.u32()?; PatchRec::GvSet(s, self.val()?) }
            7 => PatchRec::GvRemove(self.u32()?),
            8 => return Err(String::from("the state of the default Random (change kind 8) is not carried to this VM yet")),
            9 => return Err(String::from("prepended modules on a built-in class (change kind 9) are not carried to this VM yet")),
            10 => {
                let o = self.u32()?;
                let frozen = self.u8()? != 0;
                match self.u8()? {
                    3 => PatchRec::StrSet(o, frozen, self.bytes()?),
                    4 => { let n = self.u32()?; PatchRec::AryContents(o, frozen, (0..n).map(|_| self.val()).collect::<R<_>>()?) }
                    5 => return Err(String::from("new contents of a built-in Hash (change kind 10) are not carried to this VM yet")),
                    t => return Err(format!("bad contents type {t}")),
                }
            }
            k => return Err(format!("bad change kind {k}")),
        })
    }
}

/// Reads the whole image; checks the header and that nothing is left over.
pub fn parse(bytes: &[u8]) -> R<Image> {
    let mut c = Cur { b: bytes, p: 0 };
    if c.take(4).ok() != Some(b"MRCK".as_slice()) { return Err(String::from("not a checkpoint image")); }
    let version = c.u16()?;
    if version != FORMAT_VERSION { return Err(format!("checkpoint format {version} is not supported")); }
    let writer = c.string()?;
    let rite = c.bytes()?;
    if rite != b"RITE0400" { return Err(format!("bytecode {} is not RITE0400", String::from_utf8_lossy(&rite))); }
    let (ibits, fbits, utf8) = (c.u8()?, c.u8()?, c.u8()?);
    c.u8()?; // the writer's pointer width, for information only
    if ibits != 64 || fbits != 64 || utf8 != 1 {
        return Err(String::from("value representation differs (needs 64-bit Integer, double Float, UTF-8)"));
    }
    let n = c.u32()?;
    let syms = (0..n).map(|_| c.bytes()).collect::<R<_>>()?;
    let n = c.u32()?;
    let ireps = (0..n).map(|_| c.irep()).collect::<R<_>>()?;
    let n = c.u32()?;
    let objs = (0..n).map(|_| c.object()).collect::<R<_>>()?;
    let n = c.u32()?;
    let patches = (0..n).map(|_| c.patch()).collect::<R<_>>()?;
    let n = c.u32()?;
    let stack = (0..n).map(|_| c.val()).collect::<R<_>>()?;
    let n = c.u32()?;
    let mut frames = Vec::new();
    for _ in 0..n {
        frames.push(FrameRec {
            mid: c.u32()?, proc_: c.u32()?, base: c.u32()?, pc: c.u32()?, n: c.u8()?, nk: c.u8()?, cci: c.u8()?, vis: c.u8()?,
            blk: c.u32()?, env_kind: c.u8()?, env: c.u32()?,
        });
    }
    // optional sections after the VM, each with a tag: line numbers (DBG0, put on the
    // carried ireps) and the GC's settings (GCS0, which this VM's collector has no use for);
    // then the end mark END0
    let mut lines = Vec::new();
    let mut ended = false;
    while bytes.len() - c.p >= 4 {
        match c.take(4)? {
            b"END0" => { ended = true; break; }
            b"DBG0" => {
                let n = c.u32()?;
                for _ in 0..n {
                    let id = c.u32()?;
                    let nf = c.u16()?;
                    let mut file = None;
                    let mut ents = Vec::new();
                    for _ in 0..nf {
                        let f = c.bytes()?; c.u32()?; c.u32()?;
                        if file.is_none() { file = Some(String::from_utf8_lossy(&f).into_owned()); }
                        let ne = c.u32()?;
                        for _ in 0..ne { let pc = c.u32()?; let line = c.u32()?; ents.push((pc, line)); }
                    }
                    lines.push((id, file.unwrap_or_default(), ents));
                }
            }
            b"GCS0" => { c.take(1 + 4 + 4 + 8 + 8 + 8)?; }
            _ => { c.p -= 4; break; }
        }
    }
    // the end mark: an image cut between two sections is refused too
    if !ended { return Err(String::from("truncated image (no end mark)")); }
    if c.p != bytes.len() { return Err(format!("{} bytes left after the image", bytes.len() - c.p)); }
    Ok(Image { version, writer, syms, ireps, objs, patches, stack, frames, lines })
}

const FNV_INIT: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes { h ^= *b as u64; h = h.wrapping_mul(FNV_PRIME); }
    h
}
fn fnv_u(h: u64, v: u64) -> u64 { fnv(h, &v.to_be_bytes()) }

/// The content hash of a carried irep record (the writer's FNV-1a 64 over the counts, the
/// code, the pool, the symbol names and the children's hashes), with the children's hashes
/// taken from their own records (each of which is checked in turn).
pub fn irep_hash(img: &Image, r: &IrepRec) -> u64 {
    let mut h = FNV_INIT;
    for v in [r.nlocals as u64, r.nregs as u64, r.clen as u64, r.iseq.len() as u64] { h = fnv_u(h, v); }
    h = fnv(h, &r.iseq);
    h = fnv(h, &r.catch);
    h = fnv_u(h, r.pool.len() as u64);
    for p in &r.pool {
        match p {
            PoolRec::Int(i) => { h = fnv_u(h, b'i' as u64); h = fnv_u(h, *i as u64); }
            PoolRec::Float(f) => { h = fnv_u(h, b'f' as u64); h = fnv_u(h, f.to_bits()); }
            PoolRec::BigInt(b) => { h = fnv_u(h, b'b' as u64); h = fnv_u(h, b.len() as u64); h = fnv(h, b); }
            PoolRec::Str(s) => { h = fnv_u(h, b's' as u64); h = fnv_u(h, s.len() as u64); h = fnv(h, s); }
        }
    }
    h = fnv_u(h, r.syms.len() as u64);
    for s in &r.syms {
        match (*s as usize).checked_sub(1).and_then(|i| img.syms.get(i)) {
            None => h = fnv_u(h, u64::MAX),
            Some(name) => { h = fnv_u(h, name.len() as u64); h = fnv(h, name); }
        }
    }
    h = fnv_u(h, r.reps.len() as u64);
    for c in &r.reps {
        let ch = (*c as usize).checked_sub(1).and_then(|i| img.ireps.get(i)).map_or(0, |x| x.hash);
        h = fnv_u(h, ch);
    }
    h
}
