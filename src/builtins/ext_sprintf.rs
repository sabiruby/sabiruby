//! mruby-sprintf (`mrbgems/mruby-sprintf/src/sprintf.c`): `Kernel#sprintf` /
//! `format`. `String#%` is the gem's Ruby part (`src/mrblib/sprintf.mrb`).
//! The state machine (flags, `n$`, `<name>`, `*`) is the reference's; floats
//! are rendered with `core::fmt` (correctly rounded) and reshaped to C's
//! `%f`/`%e`/`%g` conventions.

use alloc::{format, string::{String, ToString}, vec, vec::Vec};

use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::Value;
use crate::vm::Vm;

const FSHARP: u32 = 1;
const FMINUS: u32 = 2;
const FPLUS: u32 = 4;
const FZERO: u32 = 8;
const FSPACE: u32 = 16;
const FWIDTH: u32 = 32;
const FPREC: u32 = 64;
const FPREC0: u32 = 128;
const INT_MAX: i64 = 2147483647;

struct Fmt<'a> {
    args: &'a [Value],
    /// 0: nothing yet; >0: next unnumbered index; -1: numbered used; -2: named used
    posarg: i64,
    nextarg: i64,
    hash: Option<Value>,
}

fn arg_err(vm: &mut Vm, msg: &str) -> crate::error::VmError { vm.raise_arg(msg) }

impl<'a> Fmt<'a> {
    /// GETNTHARG (1-based, argv[0] being the format string)
    fn nth(&self, vm: &mut Vm, nth: i64) -> VmResult<Value> {
        if nth < 1 || nth as usize > self.args.len() { return Err(arg_err(vm, "too few arguments")); }
        Ok(self.args[nth as usize - 1])
    }
    fn next(&mut self, vm: &mut Vm) -> VmResult<Value> {
        match self.posarg {
            -1 => return Err(arg_err(vm, &format!("unnumbered({}) mixed with numbered", self.nextarg))),
            -2 => return Err(arg_err(vm, &format!("unnumbered({}) mixed with named", self.nextarg))),
            _ => {}
        }
        self.posarg = self.nextarg;
        self.nextarg += 1;
        self.nth(vm, self.posarg)
    }
    fn pos(&mut self, vm: &mut Vm, n: i64) -> VmResult<Value> {
        if self.posarg > 0 { return Err(arg_err(vm, &format!("numbered({n}) after unnumbered({})", self.posarg))); }
        if self.posarg == -2 { return Err(arg_err(vm, &format!("numbered({n}) after named"))); }
        if n < 1 { return Err(arg_err(vm, &format!("invalid index - {n}$"))); }
        self.posarg = -1;
        self.nth(vm, n)
    }
    fn named(&mut self, vm: &mut Vm, name: &[u8]) -> VmResult<Value> {
        let shown = String::from_utf8_lossy(name).into_owned();
        if self.posarg > 0 { return Err(arg_err(vm, &format!("named{shown} after unnumbered({})", self.posarg))); }
        if self.posarg == -1 { return Err(arg_err(vm, &format!("named{shown} after numbered"))); }
        self.posarg = -2;
        if self.hash.is_none() {
            if self.args.len() != 1 { return Err(arg_err(vm, "one hash required")); }
            let h = self.args[0];
            if !matches!(h.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Hash(_))) { return Err(arg_err(vm, "one hash required")); }
            self.hash = Some(h);
        }
        let key = String::from_utf8_lossy(&name[1..name.len() - 1]).into_owned();
        let sym = vm.intern(&key);
        match vm.hash_get(self.hash.unwrap(), Value::Sym(sym)) {
            Some(v) => Ok(v),
            None => Err(vm.raise(vm.core.key_error, &format!("key{shown} not found"))),
        }
    }
}

/// `get_num`: decimal digits at `p`; None when they do not fit an int.
fn get_num(f: &[u8], p: &mut usize) -> Option<i64> {
    let mut n: i64 = 0;
    let start = *p;
    while *p < f.len() && f[*p].is_ascii_digit() {
        n = n.checked_mul(10)?.checked_add((f[*p] - b'0') as i64)?;
        if n > INT_MAX { return None; }
        *p += 1;
    }
    let _ = start;
    Some(n)
}

fn digits_u64(mut u: u64, base: u32) -> Vec<u8> {
    if u == 0 { return b"0".to_vec(); }
    let mut d = vec![];
    while u > 0 { let r = (u % base as u64) as u8; d.push(if r < 10 { b'0' + r } else { b'a' + r - 10 }); u /= base as u64; }
    d.reverse();
    d
}

/// `mrb_uint_to_cstr` + `remove_sign_bits` for a negative value.
fn uint_to_cstr(v: i64, base: u32) -> Vec<u8> {
    let mut d = digits_u64(v as u64, base);
    if v < 0 {
        if base == 8 {
            // the top octal digit of a 64-bit value holds one bit: sign-extend it
            d[0] |= 6;
        }
        let fc = match base { 16 => b'f', 8 => b'7', _ => b'1' };
        let skip = d.iter().take_while(|c| **c == fc).count();
        d.drain(..skip);
    }
    d
}

/// `mrb_str_to_integer(str, 0, badcheck=TRUE)`: whole string must be an integer
/// (optional sign, `0x`/`0b`/`0o`/`0d` prefix, underscores between digits).
fn str_to_int_strict(vm: &mut Vm, b: &[u8]) -> VmResult<i64> {
    let s = String::from_utf8_lossy(b).into_owned();
    let bad = |vm: &mut Vm| -> crate::error::VmError { let shown = String::from_utf8_lossy(b).into_owned(); vm.raise_arg(&format!("invalid string for number(\"{shown}\")")) };
    let t = s.trim();
    let mut cs = t.chars().peekable();
    let mut neg = false;
    if let Some(&c) = cs.peek() { if c == '+' || c == '-' { neg = c == '-'; cs.next(); } }
    let rest: String = cs.collect();
    let (base, body) = if rest.len() >= 2 && rest.starts_with('0') {
        match rest.as_bytes()[1].to_ascii_lowercase() { b'x' => (16, &rest[2..]), b'b' => (2, &rest[2..]), b'o' => (8, &rest[2..]), b'd' => (10, &rest[2..]), _ => (8, &rest[1..]) }
    } else { (10, &rest[..]) };
    if body.is_empty() { if rest == "0" { return Ok(0); } return Err(bad(vm)); }
    let mut val: i64 = 0;
    let mut last_us = true;
    for c in body.chars() {
        if c == '_' { if last_us { return Err(bad(vm)); } last_us = true; continue; }
        let d = match c.to_digit(base) { Some(d) => d as i64, None => return Err(bad(vm)) };
        val = match val.checked_mul(base as i64).and_then(|v| v.checked_add(d)) { Some(v) => v, None => return Err(bad(vm)) };
        last_us = false;
    }
    if last_us { return Err(bad(vm)); }
    Ok(if neg { -val } else { val })
}

/// C-style `%f` / `%e` / `%g` of a non-negative finite value, without sign.
fn float_body(x: f64, fmt: u8, prec: i64, alt: bool) -> String {
    let p = prec.max(0) as usize;
    match fmt {
        b'f' => { let mut s = format!("{:.*}", p, x); if alt && p == 0 { s.push('.'); } s }
        b'e' | b'E' => {
            let s = format!("{:.*e}", p, x);
            let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
            let ev: i64 = e.parse().unwrap_or(0);
            let mut out = String::from(m);
            if alt && p == 0 { out.push('.'); }
            out.push(if fmt == b'E' { 'E' } else { 'e' });
            out.push(if ev < 0 { '-' } else { '+' });
            out.push_str(&format!("{:02}", ev.abs()));
            out
        }
        _ => {
            // g / G
            let pp = if p == 0 { 1 } else { p };
            let probe = format!("{:.*e}", pp - 1, x);
            let ev: i64 = probe.split_once('e').map(|(_, e)| e.parse().unwrap_or(0)).unwrap_or(0);
            let mut s = if x != 0.0 && (ev < -4 || ev >= pp as i64) {
                float_body(x, if fmt == b'G' { b'E' } else { b'e' }, pp as i64 - 1, alt)
            } else {
                let fp = if x == 0.0 { pp as i64 - 1 } else { pp as i64 - 1 - ev };
                float_body(x, b'f', fp.max(0), alt)
            };
            if !alt {
                // strip trailing zeros of the mantissa
                let (mant, exp) = match s.find(|c| c == 'e' || c == 'E') { Some(i) => (s[..i].to_string(), s[i..].to_string()), None => (s.clone(), String::new()) };
                let mant = if mant.contains('.') { let t = mant.trim_end_matches('0'); let t = t.trim_end_matches('.'); t.to_string() } else { mant };
                s = mant + &exp;
            }
            s
        }
    }
}

/// `fmt_float`: sign, zero padding and width.
fn fmt_float(f: f64, fmt: u8, flags: u32, width: i64, prec: i64) -> Vec<u8> {
    let alt = flags & FSHARP != 0;
    let mut sign: Option<u8> = None;
    if flags & FPLUS != 0 { sign = Some(b'+'); }
    if flags & FSPACE != 0 { sign = Some(b' '); }
    let neg = f.is_sign_negative();
    let body = float_body(if neg { -f } else { f }, fmt, if prec < 0 { 6 } else { prec }, alt);
    let mut buf: Vec<u8> = vec![];
    if neg { buf.push(b'-'); } else if let Some(c) = sign { buf.push(c); }
    buf.extend_from_slice(body.as_bytes());
    let len = buf.len() as i64;
    let left = flags & FMINUS != 0;
    let zero = flags & FZERO != 0;
    if len >= width { return buf; }
    let pad = (width - len) as usize;
    if left { buf.extend(core::iter::repeat(b' ').take(pad)); return buf; }
    if zero {
        let has_sign = buf[0] < b'0';
        let mut out = vec![];
        if has_sign { out.push(buf[0]); }
        out.extend(core::iter::repeat(b'0').take(pad));
        out.extend_from_slice(if has_sign { &buf[1..] } else { &buf[..] });
        return out;
    }
    let mut out: Vec<u8> = core::iter::repeat(b' ').take(pad).collect();
    out.extend_from_slice(&buf);
    out
}

fn as_float(vm: &mut Vm, v: Value) -> VmResult<f64> {
    match v {
        Value::Float(f) => Ok(f),
        Value::Int(i) => Ok(i as f64),
        _ if vm.is_bigint(v) => Ok(super::numeric::num_f64(vm, v).unwrap_or(0.0)),
        _ => { let d = vm.describe_for_type_error(v); Err(vm.raise_type(&format!("can't convert {d} into Float"))) }
    }
}

/// `mrb_str_format`.
pub fn format_str(vm: &mut Vm, fmt: &[u8], args: &[Value]) -> VmResult<Vec<u8>> {
    let mut binary = false;
    format_str_enc(vm, fmt, args, &mut binary)
}

/// `mrb_str_format`, also answering whether the string it built is to be read as bytes: bytes
/// that were read as bytes and go above ASCII spell no character in the string they are written
/// into, so they hand it the byte reading along with themselves (`mark_written_bytes`).
pub fn format_str_enc(vm: &mut Vm, fmt: &[u8], args: &[Value], binary: &mut bool) -> VmResult<Vec<u8>> {
    let mut st = Fmt { args, posarg: 0, nextarg: 1, hash: None };
    let f = fmt;
    let mut out: Vec<u8> = Vec::with_capacity(f.len() + 16);
    let mut p = 0usize;
    let end = f.len();
    while p < end {
        let mut t = p;
        while t < end && f[t] != b'%' { t += 1; }
        if t + 1 == end { return Err(arg_err(vm, "incomplete format specifier; use %% (double %) instead")); }
        out.extend_from_slice(&f[p..t]);
        if t >= end { break; }
        p = t + 1;
        let mut width: i64 = -1;
        let mut prec: i64 = -1;
        let mut nextvalue: Option<Value> = None;
        let mut flags: u32 = 0;
        let mut id: Option<Vec<u8>> = None;
        // the specifier proper
        'spec: loop {
            if p >= end { return Err(arg_err(vm, "malformed format string - unexpected end")); }
            let c = f[p];
            match c {
                b' ' | b'#' | b'+' | b'-' | b'0' => {
                    if flags & FWIDTH != 0 { return Err(arg_err(vm, "flag after width")); }
                    if flags & FPREC0 != 0 { return Err(arg_err(vm, "flag after precision")); }
                    flags |= match c { b' ' => FSPACE, b'#' => FSHARP, b'+' => FPLUS, b'-' => FMINUS, _ => FZERO };
                    p += 1;
                }
                b'1'..=b'9' => {
                    let n = match get_num(f, &mut p) { Some(n) => n, None => return Err(arg_err(vm, "width too big")) };
                    if p < end && f[p] == b'$' {
                        if nextvalue.is_some() { return Err(arg_err(vm, &format!("value given twice - {n}$"))); }
                        nextvalue = Some(st.pos(vm, n)?);
                        p += 1;
                        continue;
                    }
                    if flags & FWIDTH != 0 { return Err(arg_err(vm, "width given twice")); }
                    if flags & FPREC0 != 0 { return Err(arg_err(vm, "width after precision")); }
                    width = n;
                    flags |= FWIDTH;
                }
                b'<' | b'{' => {
                    let term = if c == b'<' { b'>' } else { b'}' };
                    let start = p;
                    while p < end && f[p] != term { p += 1; }
                    let name = f[start..(p + 1).min(end)].to_vec();
                    if let Some(prev) = &id { let a = String::from_utf8_lossy(&name).into_owned(); let b = String::from_utf8_lossy(&prev[1..prev.len() - 1]).into_owned(); return Err(arg_err(vm, &format!("name{a} after <{b}>"))); }
                    let v = st.named(vm, &name)?;
                    nextvalue = Some(v);
                    id = Some(name);
                    if term == b'}' {
                        // format_s
                        let s = format_string(vm, v, false, flags, width, prec)?;
                        out.extend_from_slice(&s);
                        break 'spec;
                    }
                    p += 1;
                }
                b'*' => {
                    if flags & FWIDTH != 0 { return Err(arg_err(vm, "width given twice")); }
                    if flags & FPREC0 != 0 { return Err(arg_err(vm, "width after precision")); }
                    flags |= FWIDTH;
                    let v = getaster(vm, &mut st, f, &mut p)?;
                    let w = vm.expect_int(v, "width")?;
                    if w > 32767 || w < -32768 { return Err(arg_err(vm, "width too big")); }
                    width = w;
                    if width < 0 { flags |= FMINUS; width = -width; }
                    p += 1;
                }
                b'.' => {
                    if flags & FPREC0 != 0 { return Err(arg_err(vm, "precision given twice")); }
                    flags |= FPREC | FPREC0;
                    p += 1;
                    if p < end && f[p] == b'*' {
                        let v = getaster(vm, &mut st, f, &mut p)?;
                        prec = vm.expect_int(v, "precision")?;
                        if prec < 0 { flags &= !FPREC; }
                        p += 1;
                        continue;
                    }
                    prec = match get_num(f, &mut p) { Some(n) => n, None => return Err(arg_err(vm, "precision too big")) };
                }
                b'\n' | 0 | b'%' => {
                    if c != b'%' { p -= 1; }
                    if flags != 0 { return Err(arg_err(vm, "invalid format character - %")); }
                    out.push(b'%');
                    break 'spec;
                }
                b'c' => {
                    let val = match nextvalue.take() { Some(v) => v, None => st.next(vm)? };
                    let ch: Vec<u8> = match val {
                        // a code point with the feature `utf8`, a byte without it; a value that
                        // spells no character writes no byte, which CRuby reports here as an
                        // invalid character rather than as a range error
                        Value::Int(code) => {
                            if super::string::UTF8 {
                                match super::string::utf8_to_buf(code) { Some(b) => b, None => return Err(arg_err(vm, "invalid character")) }
                            } else { vec![(code & 0xff) as u8] }
                        }
                        _ => {
                            let s = match vm.str_bytes(val) { Some(b) => b.to_vec(), None => { let to_str = vm.intern("to_str"); if vm.respond_to(val, to_str) { let r = vm.funcall(val, to_str, &[], Value::Nil)?; match vm.str_bytes(r) { Some(b) => b.to_vec(), None => return Err(arg_err(vm, "invalid character")) } } else { return Err(arg_err(vm, "invalid character")) } } };
                            // one byte, as the reference asks in both builds ("%c" % "い" is
                            // refused there too: `RSTRING_LEN(tmp) != 1`)
                            if s.len() != 1 { return Err(arg_err(vm, "%c requires a character")); }
                            if vm.str_binary(val) && s[0] >= 0x80 { *binary = true; }
                            s
                        }
                    };
                    if flags & FWIDTH == 0 { out.extend_from_slice(&ch); }
                    else if flags & FMINUS != 0 { out.extend_from_slice(&ch); if width > 0 { out.extend(core::iter::repeat(b' ').take((width - 1).max(0) as usize)); } }
                    else { if width > 0 { out.extend(core::iter::repeat(b' ').take((width - 1).max(0) as usize)); } out.extend_from_slice(&ch); }
                    break 'spec;
                }
                b's' | b'p' => {
                    let val = match nextvalue.take() { Some(v) => v, None => st.next(vm)? };
                    // `inspect` builds a string of its own, so only `%s` hands the reading over
                    if c == b's' && vm.str_binary(val) {
                        if let Some(b) = vm.str_bytes(val) { if b.iter().any(|x| *x >= 0x80) { *binary = true; } }
                    }
                    let s = format_string(vm, val, c == b'p', flags, width, prec)?;
                    out.extend_from_slice(&s);
                    break 'spec;
                }
                b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'b' | b'B' => {
                    let val = match nextvalue.take() { Some(v) => v, None => st.next(vm)? };
                    let s = format_int(vm, val, c, flags, width, prec)?;
                    out.extend_from_slice(&s);
                    break 'spec;
                }
                b'f' | b'e' | b'E' | b'g' | b'G' => {
                    let val = match nextvalue.take() { Some(v) => v, None => st.next(vm)? };
                    let fval = as_float(vm, val)?;
                    if !fval.is_finite() {
                        let expr: &[u8] = if fval.is_nan() { b"NaN" } else { b"Inf" };
                        let sign: Option<u8> = if !fval.is_nan() && fval < 0.0 { Some(b'-') } else if flags & (FPLUS | FSPACE) != 0 { Some(if flags & FPLUS != 0 { b'+' } else { b' ' }) } else { None };
                        let mut need = 3 + if sign.is_some() { 1 } else { 0 };
                        if flags & FWIDTH != 0 && need < width { need = width; }
                        let mut body = vec![];
                        if let Some(s) = sign { body.push(s); }
                        body.extend_from_slice(expr);
                        let pad = (need - body.len() as i64).max(0) as usize;
                        if flags & FMINUS != 0 { out.extend_from_slice(&body); out.extend(core::iter::repeat(b' ').take(pad)); }
                        else { out.extend(core::iter::repeat(b' ').take(pad)); out.extend_from_slice(&body); }
                        break 'spec;
                    }
                    // the reference's size guard against absurd width/precision
                    let mut need: i64 = 0;
                    if c != b'e' && c != b'E' {
                        let (_, e) = libm::frexp(fval);
                        if e > 0 { need = (e as i64 * 146) / 485 + 1; }
                    }
                    let pr = if flags & FPREC != 0 { prec } else { 6 };
                    if need > INT_MAX - pr { return Err(arg_err(vm, if width > prec { "width too big" } else { "prec too big" })); }
                    need += pr;
                    if flags & FWIDTH != 0 && need < width { need = width; }
                    if need > INT_MAX - 20 { return Err(arg_err(vm, if width > prec { "width too big" } else { "prec too big" })); }
                    let s = fmt_float(fval, c, flags, width, if flags & FPREC != 0 { prec } else { -1 });
                    out.extend_from_slice(&s);
                    break 'spec;
                }
                _ => { return Err(arg_err(vm, &format!("malformed format string - %{}", c as char))); }
            }
        }
        p += 1;
    }
    Ok(out)
}

/// GETASTER: `*` (optionally `*N$`) reads width/precision from an argument.
fn getaster(vm: &mut Vm, st: &mut Fmt, f: &[u8], p: &mut usize) -> VmResult<Value> {
    let t = *p;
    *p += 1;
    let n = match get_num(f, p) { Some(n) => n, None => return Err(arg_err(vm, "width too big")) };
    if *p < f.len() && f[*p] == b'$' {
        st.pos(vm, n)
    } else {
        let v = st.next(vm)?;
        *p = t;
        Ok(v)
    }
}

fn format_string(vm: &mut Vm, arg: Value, inspect: bool, flags: u32, width: i64, prec: i64) -> VmResult<Vec<u8>> {
    let s = if inspect { vm.inspect(arg)? } else { vm.as_string(arg)? };
    // the width and the precision count bytes, as the reference does in both builds
    let mut len = s.len() as i64;
    let mut out = vec![];
    if flags & (FPREC | FWIDTH) != 0 {
        let mut slen = len;
        if flags & FPREC != 0 && prec < slen { slen = prec; len = prec; }
        if flags & FWIDTH != 0 && width > slen {
            let pad = (width - slen) as usize;
            if flags & FMINUS == 0 { out.extend(core::iter::repeat(b' ').take(pad)); }
            out.extend_from_slice(&s[..len as usize]);
            if flags & FMINUS != 0 { out.extend(core::iter::repeat(b' ').take(pad)); }
            return Ok(out);
        }
    }
    out.extend_from_slice(&s[..len as usize]);
    Ok(out)
}

fn format_int(vm: &mut Vm, val: Value, c: u8, flags: u32, mut width: i64, mut prec: i64) -> VmResult<Vec<u8>> {
    let (base, upper, signed) = match c { b'd' | b'i' | b'u' => (10u32, false, true), b'o' => (8, false, false), b'x' => (16, false, false), b'X' => (16, true, false), b'b' => (2, false, false), _ => (2, true, false) };
    let mut prefix: Option<&[u8]> = if flags & FSHARP != 0 { match (base, upper) { (8, _) => Some(b"0"), (16, false) => Some(b"0x"), (16, true) => Some(b"0X"), (2, false) => Some(b"0b"), (2, true) => Some(b"0B"), _ => None } } else { None };
    // a Float or a String is converted first (`bin_retry`), and may itself be wide
    let val = match val {
        Value::Float(f) if f.is_finite() => { let t = libm::trunc(f); if (-9223372036854775808.0..9223372036854775808.0).contains(&t) { Value::Int(t as i64) } else { vm.bint_value(crate::bigint::BigInt::from_f64(t)) } }
        Value::Int(_) | Value::Float(_) => val,
        v if vm.is_bigint(v) => val,
        _ => match vm.str_bytes(val) { Some(b) => { let b = b.to_vec(); Value::Int(str_to_int_strict(vm, &b)?) } None => Value::Int(vm.expect_int(val, "value")?) },
    };
    let mut sc: Option<u8> = None;
    let mut dots = false;
    let mut s: Vec<u8>;
    let v: i64;
    if let Some(b) = vm.as_bigint(val).filter(|_| vm.is_bigint(val)) {
        // `mrb_bint_to_s` writes the digits and, for `%d`, the sign; the sign character and
        // the width it takes are the reference's `str_skip`, which leaves both alone
        let need_dots = flags & FPLUS == 0 && matches!(base, 16 | 8 | 2) && b.sign() < 0;
        let t = if need_dots { crate::bigint::BigInt { neg: false, mag: b.two_comp_mag() } } else { b };
        s = t.to_string_radix(base).into_bytes();
        dots = need_dots;
        v = if need_dots { -1 } else { 0 };
    } else {
        v = match val { Value::Int(i) => i, _ => vm.expect_int(val, "value")? };
        if signed {
            if v >= 0 { if flags & FPLUS != 0 { sc = Some(b'+'); width -= 1; } else if flags & FSPACE != 0 { sc = Some(b' '); width -= 1; } }
            else { sc = Some(b'-'); width -= 1; }
            s = digits_u64(v.unsigned_abs(), base);
        } else {
            s = uint_to_cstr(v, base);
            if v < 0 { dots = true; }
        }
    }
    let mut fc: u8 = match base { 16 => b'f', 8 => b'7', 2 => b'1', _ => 0 };
    if dots {
        if base == 8 && (s.first() == Some(&b'1') || s.first() == Some(&b'3')) { s.remove(0); }
        while s.first() == Some(&fc) { s.remove(0); }
    }
    if upper { for ch in s.iter_mut() { *ch = ch.to_ascii_uppercase(); } if base == 16 { fc = b'F'; } }
    let mut len = s.len() as i64;
    if prefix == Some(b"0") {
        if dots { prefix = None; }
        else if len == 1 && s[0] == b'0' { len = 0; if flags & FPREC != 0 { prec -= 1; } }
        else if flags & FPREC != 0 && prec > len { prefix = None; }
    } else if len == 1 && s[0] == b'0' { prefix = None; }
    if let Some(px) = prefix { width -= px.len() as i64; }
    if flags & (FZERO | FMINUS | FPREC) == FZERO { prec = width; width = 0; }
    else {
        if prec < len { if prefix.is_none() && prec == 0 && len == 1 && s[0] == b'0' { len = 0; } prec = len; }
        width -= prec;
    }
    let mut out = vec![];
    if flags & FMINUS == 0 && width > 0 { out.extend(core::iter::repeat(b' ').take(width as usize)); width = 0; }
    if let Some(c) = sc { out.push(c); }
    if let Some(px) = prefix { out.extend_from_slice(px); }
    if dots {
        prec -= 2; width -= 2;
        out.extend_from_slice(b"..");
        if s.first() != Some(&fc) { out.push(fc); prec -= 1; width -= 1; }
    }
    if prec > len {
        if flags & (FMINUS | FPREC) != FMINUS { out.extend(core::iter::repeat(b'0').take((prec - len) as usize)); }
        else if v < 0 { out.extend(core::iter::repeat(fc).take((prec - len) as usize)); }
    }
    out.extend_from_slice(&s[..len as usize]);
    if width > 0 { out.extend(core::iter::repeat(b' ').take(width as usize)); }
    Ok(out)
}

/// What `Kernel#sprintf` makes of `a` (the format string first): the bytes and whether they
/// read as bytes. `Kernel#printf` wants the same bytes without a String around them.
pub(crate) fn sprintf_bytes(vm: &mut Vm, a: &[Value]) -> VmResult<(Vec<u8>, bool)> {
    if a.is_empty() { return Err(vm.raise_arg("too few arguments")); }
    let fmt = match vm.str_bytes(a[0]) { Some(b) => b.to_vec(), None => { let d = vm.describe_for_type_error(a[0]); return Err(vm.raise_type(&format!("{d} cannot be converted to String"))) } };
    // the format string lays its own bytes down as they are, so the answer is read the way it is
    let mut binary = vm.str_binary(a[0]);
    let out = format_str_enc(vm, &fmt, &a[1..], &mut binary)?;
    Ok((out, binary))
}

fn sprintf(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    let (out, binary) = sprintf_bytes(vm, a)?;
    let r = vm.str_new(&out);
    vm.str_set_binary(r, binary);
    Ok(r)
}

pub fn init(vm: &mut Vm) {
    let k = vm.core.kernel;
    // both are module functions in the reference (sprintf.c)
    for name in ["sprintf", "format"] {
        vm.define_module_function(k, name, sprintf).expect("Kernel singleton");
    }
}
