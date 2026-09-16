//! String.
//!
//! With the feature `utf8` (on by default) a String is a sequence of characters, as it is in
//! a reference built with `MRB_UTF8_STRING`: `length`, `[]`, `index`, `chars`, `reverse`,
//! `center` and the case methods count and cut characters, while `bytesize`, `byteslice`,
//! `byteindex`, `getbyte` and friends stay bytes. Without the feature every one of them is
//! bytes, which is the reference's default build. A string `String#b` has made byte-read has one
//! position per byte in either build, whatever its bytes are (`MRB_STR_ENCODING_BINARY`).
//!
//! The three are one body of code: the helpers below (`char_len`, `char_to_byte`, `chars_of`, …)
//! take how the string is read as their last argument, and are the identity on a byte-read one.
//! A method asks `char_mode` for its receiver, and a needle or separator answers for itself: a
//! run of bytes that spells no character is found nowhere, and a byte-read one is searched for
//! as it stands.

use alloc::{format, string::String, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::ObjKind;
use crate::value::Value;
use crate::vm::Vm;

/// `String#dump`: every byte outside printable ASCII is escaped, in both builds
/// (`mrb_str_dump`).
pub fn str_dump(b: &[u8]) -> Vec<u8> {
    let mut out = vec![b'"'];
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x07 => out.extend_from_slice(b"\\a"),
            0x1b => out.extend_from_slice(b"\\e"),
            0x20..=0x7e => { if c == b'#' { if let Some(&n) = b.get(i + 1) { if n == b'{' || n == b'$' || n == b'@' { out.push(b'\\'); } } } out.push(c); }
            _ => out.extend_from_slice(format!("\\x{c:02X}").as_bytes()),
        }
    }
    out.push(b'"');
    out
}

pub fn str_inspect(b: &[u8], chars: bool) -> Vec<u8> {
    let mut out = vec![b'"'];
    let mut i = 0;
    while i < b.len() {
        // a character of more than one byte is written as itself (the reference escapes
        // it byte by byte only in a byte-string build)
        let n = utf8len(b, i, chars);
        if n > 1 { out.extend_from_slice(&b[i..i + n]); i += n; continue; }
        let c = b[i];
        let iter_peek = &b[i + 1..];
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x07 => out.extend_from_slice(b"\\a"),
            0x1b => out.extend_from_slice(b"\\e"),
            0x20..=0x7e => { if c == b'#' { if let Some(&n) = iter_peek.first() { if n == b'{' || n == b'$' || n == b'@' { out.push(b'\\'); } } } out.push(c); }
            _ => out.extend_from_slice(format!("\\x{c:02X}").as_bytes()),
        }
        i += 1;
    }
    out.push(b'"');
    out
}

// ------------------------------------------------------------------ characters

/// Whether the string `v` reads as characters: a build with the feature `utf8`, except for one
/// `String#b` has made byte-read, which has a position per byte (`RSTR_SINGLE_BYTE_P`).
pub(crate) fn char_mode(vm: &Vm, v: Value) -> bool { UTF8 && !vm.str_binary(v) }

/// Text with no string object behind it (a symbol's name, a format) reads as the build does.
pub(crate) const UTF8: bool = cfg!(feature = "utf8");

/// Bytes of the character that starts at `i` (mruby's `mrb_utf8len`: a byte that starts no
/// valid sequence is a character of its own, so no string is ever refused). A byte-read string
/// has a character per byte.
pub(crate) fn utf8len(b: &[u8], i: usize, chars: bool) -> usize {
    if !chars { return 1; }
    let c = match b.get(i) { Some(c) => *c, None => return 1 };
    // the byte length a lead byte claims (`mrb_utf8len_table`, indexed by the top 5 bits)
    let len = match c >> 3 {
        0..=15 => 1,
        24..=27 => 2,
        28 | 29 => 3,
        30 => 4,
        _ => return 1,
    };
    if len == 1 { return 1; }
    if i + len > b.len() { return 1; }
    for j in 1..len {
        if b[i + j] & 0xc0 != 0x80 { return 1; }
    }
    // overlong sequences, UTF-16 surrogates and anything above U+10FFFF are not characters
    // (RFC 3629, Unicode D93b), so their lead byte is one of its own
    match c {
        0xc0 | 0xc1 => return 1,
        0xe0 if b[i + 1] < 0xa0 => return 1,
        0xed if b[i + 1] > 0x9f => return 1,
        0xf0 if b[i + 1] < 0x90 => return 1,
        0xf4 if b[i + 1] > 0x8f => return 1,
        0xf5..=0xf7 => return 1,
        _ => {}
    }
    len
}

/// Characters in the string (`RSTRING_CHAR_LEN`).
pub(crate) fn char_len(b: &[u8], chars: bool) -> usize {
    if !chars { return b.len(); }
    // Every byte below 0x80 stands for itself, so an ASCII string has as many characters as
    // bytes. `is_ascii` reads a machine word at a time where the `utf8len` walk reads a byte
    // at a time and carries a table lookup with it, and the strings a program indexes in a
    // loop are usually ASCII. A string that is not pays one extra pass.
    if b.is_ascii() { return b.len(); }
    let (mut i, mut n) = (0, 0);
    while i < b.len() { i += utf8len(b, i, chars); n += 1; }
    n
}

/// Byte offset of character `ci` (the end of the string when it is past the last one).
pub(crate) fn char_to_byte(b: &[u8], ci: usize, chars: bool) -> usize {
    if !chars { return ci.min(b.len()); }
    // the same shortcut, over the prefix this has to walk anyway
    let head = ci.min(b.len());
    if b[..head].is_ascii() { return head; }
    let (mut i, mut n) = (0, 0);
    while i < b.len() && n < ci { i += utf8len(b, i, chars); n += 1; }
    i
}

/// Character index of byte offset `bi`.
pub(crate) fn byte_to_char(b: &[u8], bi: usize, chars: bool) -> usize {
    if !chars { return bi; }
    let (mut i, mut n) = (0, 0);
    while i < b.len() && i < bi { i += utf8len(b, i, chars); n += 1; }
    n
}

/// The byte range of `n` characters from character `ci`.
pub(crate) fn char_span(b: &[u8], ci: usize, n: usize, chars: bool) -> (usize, usize) {
    let start = char_to_byte(b, ci, chars);
    let end = char_to_byte(b, ci + n, chars);
    (start, end - start)
}

/// The characters, as byte slices.
pub(crate) fn chars_of(b: &[u8], chars: bool) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let n = utf8len(b, i, chars);
        out.push(&b[i..i + n]);
        i += n;
    }
    out
}

/// The code point of the character at `i`, or the byte itself when it starts no valid
/// sequence (`mrb_utf8_len`/`mrb_str_ord`).
pub(crate) fn code_point(b: &[u8], i: usize, chars: bool) -> u32 {
    let n = utf8len(b, i, chars);
    if n == 1 { return b[i] as u32; }
    let mut cp = (b[i] as u32) & (0x7f >> n);
    for j in 1..n { cp = (cp << 6) | (b[i + j] as u32 & 0x3f); }
    cp
}

/// The UTF-8 bytes of a code point, or `None` for a value that spells no character (below zero
/// or above U+10FFFF). A surrogate does have a spelling here, because `sprintf("%c")` and
/// `pack("U")` write one; `Integer#chr` is where such a value is refused (`mrb_utf8_to_buf`).
pub(crate) fn utf8_to_buf(cp: i64) -> Option<Vec<u8>> {
    if cp < 0 { return None; }
    Some(match cp {
        0..=0x7f => vec![cp as u8],
        0x80..=0x7ff => vec![0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8],
        0x800..=0xffff => vec![0xE0 | (cp >> 12) as u8, 0x80 | ((cp >> 6) & 0x3F) as u8, 0x80 | (cp & 0x3F) as u8],
        0x10000..=0x10FFFF => vec![0xF0 | (cp >> 18) as u8, 0x80 | ((cp >> 12) & 0x3F) as u8, 0x80 | ((cp >> 6) & 0x3F) as u8, 0x80 | (cp & 0x3F) as u8],
        _ => return None,
    })
}

/// The UTF-8 bytes of a code point (`mrb_utf8_from_uint32`); a lone byte in byte mode.
pub(crate) fn from_code_point(cp: u32) -> Vec<u8> {
    #[cfg(feature = "utf8")]
    {
        if let Some(c) = char::from_u32(cp) {
            let mut buf = [0u8; 4];
            return c.encode_utf8(&mut buf).as_bytes().to_vec();
        }
    }
    vec![cp as u8]
}

/// `casecmp?`'s folding: Unicode's where the string holds anything the tables speak about, and
/// ASCII's lower case where it does not, which a string of nothing but ASCII and a byte-read one
/// both are (`str_casecmp_p`). Bytes that spell no character have no folding, and the walk says so
/// (`mrb_str_case_convert_unicode`).
pub(crate) fn fold_case(vm: &mut Vm, b: &[u8], chars: bool) -> VmResult<Vec<u8>> {
    if !chars || b.iter().all(|c| *c < 0x80) { return Ok(b.to_ascii_lowercase()); }
    map_case_checked(vm, b, Case::Fold, chars)
}

/// `mrb_str_modify`: a method that changes the string refuses a frozen one before it looks
/// at anything else, so `'AB'.freeze.downcase!` is a FrozenError and not nil.
fn check_frozen(vm: &mut Vm, v: Value) -> VmResult<()> {
    if let Some(o) = v.obj() {
        if vm.heap.get(o).frozen { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen String")); }
    }
    Ok(())
}

fn bytes(vm: &Vm, v: Value) -> Vec<u8> {
    vm.str_bytes(v).map(|b| b.to_vec()).unwrap_or_default()
}
/// The bytes of a string without copying them, for a native whose answer is not the string
/// itself (a length, a byte, a comparison). The borrow must not be held across anything that
/// takes `&mut Vm`, so the argument conversions come first; where that is not possible the
/// method keeps `bytes` above. The empty slice for a non-String matches what `bytes` answers.
fn sbytes(vm: &Vm, v: Value) -> &[u8] { vm.str_bytes(v).unwrap_or(&[]) }
fn set(vm: &mut Vm, v: Value, b: Vec<u8>) -> VmResult<()> {
    match v.obj() {
        Some(o) => {
            if vm.heap.get(o).frozen { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen String")); }
            if let ObjKind::String(s) = &mut vm.heap.get_mut(o).kind { *s = b; }
            Ok(())
        }
        None => Err(vm.raise_type("not a string")),
    }
}

/// Resolves `(index, len)` / `index` / `range` against a length; returns `None` when out of range.
pub fn index_args(vm: &mut Vm, len: usize, a: &[Value]) -> VmResult<Option<(usize, usize)>> {
    let norm = |i: i64| -> Option<usize> { let i = if i < 0 { i + len as i64 } else { i }; if i < 0 || i as usize > len { None } else { Some(i as usize) } };
    let a: Vec<Value> = a.iter().map(|v| match v { Value::Float(f) => Value::Int(*f as i64), v => *v }).collect();
    // an index too wide for `i64` is out of range, not a type error (`mrb_as_int`)
    for v in &a { if vm.is_bigint(*v) { vm.expect_int(*v, "index")?; } }
    match &a[..] {
        [Value::Int(i)] => Ok(norm(*i).filter(|&i| i < len).map(|i| (i, 1))),
        [Value::Int(i), Value::Int(n)] => { if *n < 0 { return Ok(None); } Ok(norm(*i).map(|i| (i, (*n as usize).min(len - i)))) }
        [Value::Obj(o)] => {
            if let ObjKind::Range { begin, end, excl } = vm.heap.get(*o).kind {
                let (begin, end) = (begin.get(), end.get());
                let b = match begin { Value::Int(b) => b, Value::Nil => 0, _ => return Err(vm.raise_type("no implicit conversion into Integer")) };
                let e = match end { Value::Int(e) => e, Value::Nil => len as i64, _ => return Err(vm.raise_type("no implicit conversion into Integer")) };
                let b = match norm(b) { Some(b) => b, None => return Ok(None) };
                let mut e = if e < 0 { e + len as i64 } else { e };
                if !excl || end.is_nil() { e += 1; }
                let e = (e.max(b as i64) as usize).min(len);
                Ok(Some((b, e - b)))
            } else { Err(vm.raise_type("no implicit conversion into Integer")) }
        }
        _ => Err(vm.raise_type("no implicit conversion into Integer")),
    }
}

/// `n` characters of `pad`, repeating it (`mrb_str_justify`).
fn pad_chars(pad: &[u8], n: usize, chars: bool) -> Vec<u8> {
    let cs = chars_of(pad, chars);
    if cs.is_empty() { return Vec::new(); }
    let mut out = Vec::new();
    for i in 0..n { out.extend_from_slice(cs[i % cs.len()]); }
    out
}

/// The characters in reverse order (`mrb_str_reverse`).
fn str_reverse(b: &[u8], chars: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for c in chars_of(b, chars).into_iter().rev() { out.extend_from_slice(c); }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Case { Up, Down, Capitalize, Swap, Fold }

/// The title case of a character, where Unicode gives one that is not its upper case: the
/// Latin digraphs (`ǅ`, `ǈ`, `ǋ`, `ǲ`) and the scripts whose title case is the character
/// itself (Georgian Mkhedruli, which upper cases to Mtavruli). Everything else takes its
/// upper case (`docs/design/utf8.md` records the Greek ypogegrammeni forms as a difference).
#[cfg(feature = "utf8")]
fn titlecase(c: char, out: &mut String) {
    let cp = c as u32;
    let t = match cp {
        0x01c4..=0x01c6 => Some(0x01c5),
        0x01c7..=0x01c9 => Some(0x01c8),
        0x01ca..=0x01cc => Some(0x01cb),
        0x01f1..=0x01f3 => Some(0x01f2),
        // Georgian Mkhedruli and Mtavruli title case to themselves
        0x10d0..=0x10ff | 0x1c90..=0x1cbf => Some(cp),
        _ => None,
    };
    if let Some(t) = t.and_then(char::from_u32) { out.push(t); return; }
    // A mapping that spells more than one character takes the reference's title table:
    // the first character keeps its upper case and the rest take their lower case ("ﬁ"
    // to "Fi"), with the iota that upper case spells out written back as the combining
    // ypogegrammeni it came from ("ᾲ" to "Ὰͅ").
    for (i, u) in c.to_uppercase().enumerate() {
        if i == 0 { out.push(u); }
        else if u == '\u{399}' { out.push('\u{345}'); }
        else { out.extend(u.to_lowercase()); }
    }
}

/// The swapped case of a character: one with a lower case swaps down and one without swaps up,
/// so a mapping that spells more than one character comes back here too ("ß" to "SS"). The Latin
/// digraphs in title case swap to what neither of their cases spells, which is the swap table's
/// to say ("ǅ" to "dŽ"); `docs/design/utf8.md` records its Greek entries as a difference.
#[cfg(feature = "utf8")]
fn swapcase(c: char, out: &mut String) {
    if let Some(d) = match c as u32 {
        0x01c5 => Some("dŽ"),
        0x01c8 => Some("lJ"),
        0x01cb => Some("nJ"),
        0x01f2 => Some("dZ"),
        _ => None,
    } { out.push_str(d); return; }
    let mut low = c.to_lowercase();
    let first = low.next();
    if low.next().is_some() || first != Some(c) { out.extend(c.to_lowercase()); }
    else { out.extend(c.to_uppercase()); }
}

/// `upcase`/`downcase`/`capitalize`/`swapcase`. With the feature `utf8` the mapping is
/// Unicode's, as in a reference built without `MRB_USE_ASCII_CTYPE` (`"ß".upcase` is `"SS"`);
/// without it, ASCII only, byte by byte.
/// `map_case` with the reference's refusal: a run of bytes that spells no character has no
/// case, and the conversion says so rather than inventing one.
fn map_case_checked(vm: &mut Vm, b: &[u8], how: Case, chars: bool) -> VmResult<Vec<u8>> {
    if !valid_chars(b, chars) { return Err(vm.raise_arg("input string invalid")); }
    Ok(map_case(b, how, chars))
}

/// A string of nothing but ASCII holds no character the Unicode tables speak about, and one read
/// as bytes holds no characters at all: both convert their bytes where they stand
/// (`mrb_str_case_convert_unicode`).
fn map_case(b: &[u8], how: Case, chars: bool) -> Vec<u8> {
    #[cfg(not(feature = "utf8"))]
    let _ = chars;
    #[cfg(feature = "utf8")]
    if chars {
        if let Ok(text) = core::str::from_utf8(b) {
            let mut out = String::with_capacity(text.len());
            for (i, c) in text.chars().enumerate() {
                match how {
                    Case::Up => out.extend(c.to_uppercase()),
                    Case::Down => out.extend(c.to_lowercase()),
                    // the first character takes title case, which is not always its upper
                    // case ("ǳ" is "ǲ", a Georgian letter is itself)
                    Case::Capitalize => { if i == 0 { titlecase(c, &mut out); } else { out.extend(c.to_lowercase()); } }
                    Case::Swap => swapcase(c, &mut out),
                    // folding is the difference from the lower case, and a character whose
                    // upper case spells several folds to the lower case of each of them
                    // ("ß" to "ss"), which is what makes `casecmp?` wider than `casecmp`
                    Case::Fold => { for u in c.to_uppercase() { out.extend(u.to_lowercase()); } }
                }
            }
            return out.into_bytes();
        }
    }
    b.iter().enumerate().map(|(i, c)| match how {
        Case::Up => c.to_ascii_uppercase(),
        Case::Down => c.to_ascii_lowercase(),
        Case::Capitalize => if i == 0 { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() },
        Case::Swap => if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() },
        Case::Fold => c.to_ascii_lowercase(),
    }).collect()
}

/// True when every byte of `b` belongs to a character (`mrb_str_valid_encoding_p`). A byte-read
/// string claims no encoding, so its bytes are whatever they are.
pub(crate) fn valid_chars(b: &[u8], chars: bool) -> bool {
    if !chars { return true; }
    let mut i = 0;
    while i < b.len() {
        let n = utf8len(b, i, chars);
        if n == 1 && b[i] >= 0x80 { return false; }
        i += n;
    }
    true
}

/// True when byte `i` starts a character (or is the end of the string).
pub(crate) fn is_char_boundary(b: &[u8], i: usize, chars: bool) -> bool {
    if !chars { return true; }
    if i >= b.len() { return i == b.len(); }
    char_to_byte(b, byte_to_char(b, i, chars), chars) == i
}

/// The byte the character covering `i` starts at, or `i` itself when `i` already starts one
/// (`mrb_utf8_char_head`).
pub(crate) fn char_head(b: &[u8], i: usize, chars: bool) -> usize {
    if !chars || i >= b.len() { return i.min(b.len()); }
    let mut p = 0;
    while p < b.len() {
        let n = utf8len(b, p, chars);
        if p + n > i { return p; }
        p += n;
    }
    p
}

/// `mrb_str_check_byte_pos`: a byte offset that lands inside a character names no position
/// the string has, so a byte search refuses it rather than starting from the middle of one.
pub(crate) fn check_byte_pos(vm: &mut Vm, b: &[u8], pos: usize, chars: bool) -> VmResult<()> {
    if !is_char_boundary(b, pos, chars) {
        return Err(vm.raise(vm.core.index_error, &format!("offset {pos} does not land on character boundary")));
    }
    Ok(())
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() { return if from <= hay.len() { Some(from) } else { None }; }
    if from >= hay.len() { return None; }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// Backward search one byte at a time, from byte offset `pos` (`str_byterindex`).
fn rfind(hay: &[u8], needle: &[u8], pos: usize) -> Option<usize> {
    if hay.len() < needle.len() { return None; }
    let pos = pos.min(hay.len() - needle.len());
    if needle.is_empty() { return Some(pos); }
    (0..=pos).rev().find(|&i| hay[i..i + needle.len()] == needle[..])
}

/// Backward search stepping over characters, so a match that would start inside a
/// multi-byte character is passed over rather than reported (`str_char_rindex`).
fn char_rfind(hay: &[u8], needle: &[u8], pos: usize, chars: bool) -> Option<usize> {
    if !chars { return rfind(hay, needle, pos); }
    if hay.len() < needle.len() { return None; }
    let mut pos = pos.min(hay.len() - needle.len());
    if needle.is_empty() { return Some(pos); }
    pos = char_head(hay, pos, chars);
    loop {
        if hay.len() - pos >= needle.len() && hay[pos..pos + needle.len()] == needle[..] { return Some(pos); }
        if pos == 0 { return None; }
        pos = char_head(hay, pos - 1, chars);
    }
}

/// `String#to_i`: the value of the number at the start of the text; a value too wide for
/// `i64` becomes a wide integer (`mrb_str_to_integer`'s overflow arm).
pub(crate) fn to_i(vm: &mut Vm, b: &[u8], base: u32) -> Value {
    let s = String::from_utf8_lossy(b);
    let s = s.trim_start();
    let mut end = 0;
    let mut cs: Vec<char> = s.chars().collect();
    if end < cs.len() && (cs[end] == '-' || cs[end] == '+') { end += 1; }
    // optional radix prefix matching the base (`0x` for 16, `0b` for 2, `0o` for 8)
    let prefix = match base { 16 => Some('x'), 2 => Some('b'), 8 => Some('o'), _ => None };
    if let Some(p) = prefix {
        if cs.len() > end + 2 && cs[end] == '0' && cs[end + 1].to_ascii_lowercase() == p { cs.drain(end..end + 2); }
    }
    while end < cs.len() && (cs[end].is_digit(base) || (cs[end] == '_' && end > 0 && cs[end - 1] != '_')) { end += 1; }
    let t: String = cs[..end].iter().filter(|c| **c != '_').collect();
    let t = t.trim_start_matches('+');
    match i64::from_str_radix(t, base) {
        Ok(v) => Value::Int(v),
        Err(_) => match crate::bigint::BigInt::from_str(t.as_bytes(), base) {
            Some(v) => vm.bint_value(v),
            None => Value::Int(0),
        },
    }
}

fn to_f(b: &[u8]) -> f64 {
    let s = String::from_utf8_lossy(b);
    let s = s.trim_start();
    let mut end = 0;
    let cs: Vec<char> = s.chars().collect();
    let mut seen_dot = false; let mut seen_e = false;
    while end < cs.len() {
        let c = cs[end];
        if c == '_' {
            // a single underscore between digits is skipped; anything else ends the number
            if end > 0 && cs[end - 1].is_ascii_digit() && end + 1 < cs.len() && cs[end + 1].is_ascii_digit() { end += 1; } else { break; }
        }
        else if c.is_ascii_digit() { end += 1; }
        else if c == '.' && !seen_dot && !seen_e && end + 1 < cs.len() && cs[end + 1].is_ascii_digit() { seen_dot = true; end += 1; }
        else if (c == 'e' || c == 'E') && !seen_e && end > 0 { seen_e = true; end += 1; if end < cs.len() && (cs[end] == '-' || cs[end] == '+') { end += 1; } }
        else if (c == '-' || c == '+') && end == 0 { end += 1; }
        else { break; }
    }
    let t: String = cs[..end].iter().filter(|c| **c != '_').collect();
    t.parse().unwrap_or(0.0)
}

pub fn init(vm: &mut Vm) {
    let c = vm.core;
    vm.define_methods(c.string, &[
        ("initialize", |vm, s, a, _b| { argc!(vm, a, 0, 1); if a.len() == 1 { let b = vm.expect_str(a[0], "argument")?; set(vm, s, b)?; } Ok(s) }),
        ("initialize_copy", |vm, s, a, _b| { argc!(vm, a, 1); let b = vm.expect_str(a[0], "argument")?; set(vm, s, b)?; let binary = vm.str_binary(a[0]); vm.str_set_binary(s, binary); Ok(s) }),
        ("to_s", |_vm, s, _a, _b| Ok(s)),
        ("to_str", |_vm, s, _a, _b| Ok(s)),
        ("inspect", |vm, s, _a, _b| { let chars = char_mode(vm, s); let i = str_inspect(&bytes(vm, s), chars); Ok(vm.str_new(&i)) }),
        ("to_sym", |vm, s, _a, _b| { let b = bytes(vm, s); Ok(Value::Sym(vm.syms.intern(&b))) }),
        ("intern", |vm, s, _a, _b| { let b = bytes(vm, s); Ok(Value::Sym(vm.syms.intern(&b))) }),
        ("to_i", |vm, s, a, _b| { argc!(vm, a, 0, 1); let base = if a.len() == 1 { vm.expect_int(a[0], "base")? as u32 } else { 10 }; if !(2..=36).contains(&base) { return Err(vm.raise_arg(&format!("invalid radix {base}"))); } { let b = bytes(vm, s); Ok(to_i(vm, &b, base)) } }),
        ("to_f", |vm, s, _a, _b| Ok(Value::Float(to_f(sbytes(vm, s))))),
        ("size", |vm, s, _a, _b| { let chars = char_mode(vm, s); Ok(Value::Int(char_len(sbytes(vm, s), chars) as i64)) }),
        ("length", |vm, s, _a, _b| { let chars = char_mode(vm, s); Ok(Value::Int(char_len(sbytes(vm, s), chars) as i64)) }),
        ("byteslice", |vm, s, a, _b| { argc!(vm, a, 1, 2); let b = bytes(vm, s); match index_args(vm, b.len(), a)? { Some((i, n)) => { let piece = b[i..i + n].to_vec(); Ok(vm.str_new_like(&piece, s)) } None => Ok(Value::Nil) } }),
        ("byteindex", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let n = vm.expect_str(a[0], "argument")?;
            let b = bytes(vm, s);
            let from = if a.len() == 2 {
                let f = vm.expect_int(a[1], "offset")?;
                let f = if f < 0 { let f = f + b.len() as i64; if f < 0 { return Ok(Value::Nil); } f } else { f };
                if f > b.len() as i64 { return Ok(Value::Nil); }
                f as usize
            } else { 0 };
            check_byte_pos(vm, &b, from, char_mode(vm, s))?;
            // a needle whose bytes spell no character is found nowhere; a byte-read one claims
            // no encoding and is searched for as it is (`mrb_str_valid_encoding_p`)
            if !valid_chars(&n, char_mode(vm, a[0])) { return Ok(Value::Nil); }
            Ok(find(&b, &n, from).map(|i| Value::Int(i as i64)).unwrap_or(Value::Nil))
        }),
        ("bytesize", |vm, s, _a, _b| Ok(Value::Int(sbytes(vm, s).len() as i64))),
        ("empty?", |vm, s, _a, _b| Ok(Value::bool(sbytes(vm, s).is_empty()))),
        ("==", str_eq),
        ("eql?", str_eq),
        ("===", str_eq),
        ("hash", |vm, s, _a, _b| Ok(Value::Int(vm.value_hash(s)))),
        ("<=>", |vm, s, a, _b| { argc!(vm, a, 1); match vm.str_bytes(a[0]) { Some(o) => Ok(Value::Int(sbytes(vm, s).cmp(o) as i64)), None => Ok(Value::Nil) } }),
        // `mrb_str_plus`: the sum is a string with no history, so its reading comes from the
        // bytes it was built out of rather than from either operand's standing — two byte-read
        // operands stay that way, and one byte-read operand carrying a byte above ASCII hands the
        // sum bytes no other reading holds
        ("+", |vm, s, a, _b| { argc!(vm, a, 1); if vm.str_bytes(a[0]).is_none() { let d = vm.describe_for_error(a[0]); return Err(vm.raise_type(&format!("{d} cannot be converted to String"))); } let (x, y) = (vm.str_binary(s), vm.str_binary(a[0])); let (binary, joined) = { let (b, o) = (sbytes(vm, s), sbytes(vm, a[0])); let ascii = |v: &[u8]| v.iter().all(|c| *c < 0x80); let binary = (x && y) || (x && !ascii(b)) || (y && !ascii(o)); let mut j = Vec::with_capacity(b.len() + o.len()); j.extend_from_slice(b); j.extend_from_slice(o); (binary, j) }; let r = vm.str_new(&joined); vm.str_set_binary(r, binary); Ok(r) }),
        ("*", |vm, s, a, _b| { argc!(vm, a, 1); let n = vm.expect_int(a[0], "argument")?; if n < 0 { return Err(vm.raise_arg("negative argument")); } let b = bytes(vm, s); if (b.len() as i64).checked_mul(n).map(|t| t > i32::MAX as i64).unwrap_or(true) { return Err(vm.raise_arg("argument too big")); } let b = b.repeat(n as usize); Ok(vm.str_new_like(&b, s)) }),
        ("<<", str_concat),
        ("concat", str_concat),
        ("[]", str_aref),
        ("slice", str_aref),
        ("[]=", str_aset),
        ("upcase", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = map_case_checked(vm, &bytes(vm, s), Case::Up, chars)?; Ok(vm.str_new_like(&b, s)) }),
        ("downcase", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = map_case_checked(vm, &bytes(vm, s), Case::Down, chars)?; Ok(vm.str_new_like(&b, s)) }),
        ("upcase!", |vm, s, _a, _b| { check_frozen(vm, s)?; let chars = char_mode(vm, s); let b = bytes(vm, s); let u = map_case_checked(vm, &b, Case::Up, chars)?; if u == b { Ok(Value::Nil) } else { set(vm, s, u)?; Ok(s) } }),
        ("downcase!", |vm, s, _a, _b| { check_frozen(vm, s)?; let chars = char_mode(vm, s); let b = bytes(vm, s); let u = map_case_checked(vm, &b, Case::Down, chars)?; if u == b { Ok(Value::Nil) } else { set(vm, s, u)?; Ok(s) } }),
        ("capitalize", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = map_case_checked(vm, &bytes(vm, s), Case::Capitalize, chars)?; Ok(vm.str_new_like(&b, s)) }),
        ("capitalize!", |vm, s, _a, _b| { check_frozen(vm, s)?; let chars = char_mode(vm, s); let b = bytes(vm, s); let u = map_case_checked(vm, &b, Case::Capitalize, chars)?; if u == b { Ok(Value::Nil) } else { set(vm, s, u)?; Ok(s) } }),
        ("swapcase!", |vm, s, _a, _b| { check_frozen(vm, s)?; let chars = char_mode(vm, s); let b = bytes(vm, s); let u = map_case_checked(vm, &b, Case::Swap, chars)?; if u == b { Ok(Value::Nil) } else { set(vm, s, u)?; Ok(s) } }),
        ("chomp!", |vm, s, a, _b| { check_frozen(vm, s)?; argc!(vm, a, 0, 1); let b = bytes(vm, s); let rs = match a.first() { Some(Value::Nil) => return Ok(Value::Nil), Some(v) => Some(vm.expect_str(*v, "separator")?), None => None }; let n = chomp_bytes(&b, rs.as_deref(), char_mode(vm, s)); if n == b.len() { Ok(Value::Nil) } else { set(vm, s, b[..n].to_vec())?; Ok(s) } }),
        ("chop!", |vm, s, _a, _b| { check_frozen(vm, s)?; let mut b = bytes(vm, s); if b.is_empty() { return Ok(Value::Nil); } let n = chop_len(&b, char_mode(vm, s)); b.truncate(n); set(vm, s, b)?; Ok(s) }),
        ("setbyte", |vm, s, a, _b| { argc!(vm, a, 2); let i = vm.expect_int(a[0], "index")?; let v = vm.expect_int(a[1], "byte")?; let mut b = bytes(vm, s); let idx = if i < 0 { i + b.len() as i64 } else { i }; if idx < 0 || idx as usize >= b.len() { return Err(vm.raise(vm.core.index_error, &format!("index {i} out of string"))); } b[idx as usize] = v as u8; set(vm, s, b)?; Ok(a[1]) }),
        ("byterindex", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let n = vm.expect_str(a[0], "argument")?;
            let b = bytes(vm, s);
            let mut pos = if a.len() == 2 { vm.expect_int(a[1], "pos")? } else { b.len() as i64 };
            if a.len() == 2 {
                if pos < 0 { pos += b.len() as i64; if pos < 0 { return Ok(Value::Nil); } }
                if pos > b.len() as i64 { pos = b.len() as i64; }
            }
            let pos = pos as usize;
            check_byte_pos(vm, &b, pos, char_mode(vm, s))?;
            // see `byteindex`
            if !valid_chars(&n, char_mode(vm, a[0])) { return Ok(Value::Nil); }
            Ok(rfind(&b, &n, pos).map(|i| Value::Int(i as i64)).unwrap_or(Value::Nil))
        }),
        ("bytesplice", |vm, s, a, _b| { argc!(vm, a, 2, 5); let b = bytes(vm, s); let first_is_range = matches!(a[0].obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Range { .. })); let (range_args, rest) = if first_is_range { (&a[..1], &a[1..]) } else { (&a[..2], &a[2..]) }; let (i, n) = match index_args(vm, b.len(), range_args)? { Some(x) => x, None => return Err(vm.raise(vm.core.index_error, "index out of string")) }; let rep = vm.expect_str(rest[0], "replacement")?; let rep = if rest.len() >= 2 { let (ri, rn) = match index_args(vm, rep.len(), &rest[1..])? { Some(x) => x, None => return Err(vm.raise(vm.core.index_error, "index out of string")) }; rep[ri..ri + rn].to_vec() } else { rep }; let mut nb = b.clone(); nb.splice(i..i + n, rep); set(vm, s, nb)?; Ok(s) }),
        ("swapcase", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = map_case_checked(vm, &bytes(vm, s), Case::Swap, chars)?; Ok(vm.str_new_like(&b, s)) }),
        ("reverse", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = str_reverse(&bytes(vm, s), chars); Ok(vm.str_new_like(&b, s)) }),
        ("reverse!", |vm, s, _a, _b| { check_frozen(vm, s)?; let chars = char_mode(vm, s); let b = str_reverse(&bytes(vm, s), chars); set(vm, s, b)?; Ok(s) }),
        ("strip", |vm, s, _a, _b| { let b = bytes(vm, s); let t = String::from_utf8_lossy(&b).trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0').as_bytes().to_vec(); Ok(vm.str_new_like(&t, s)) }),
        ("lstrip", |vm, s, _a, _b| { let b = bytes(vm, s); let t = String::from_utf8_lossy(&b).trim_start().as_bytes().to_vec(); Ok(vm.str_new_like(&t, s)) }),
        ("rstrip", |vm, s, _a, _b| { let b = bytes(vm, s); let t = String::from_utf8_lossy(&b).trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\0').as_bytes().to_vec(); Ok(vm.str_new_like(&t, s)) }),
        ("chomp", |vm, s, a, _b| { argc!(vm, a, 0, 1); let b = bytes(vm, s); let rs = match a.first() { Some(Value::Nil) => return Ok(vm.str_new_like(&b, s)), Some(v) => Some(vm.expect_str(*v, "separator")?), None => None }; let n = chomp_bytes(&b, rs.as_deref(), char_mode(vm, s)); let t = b[..n].to_vec(); Ok(vm.str_new_like(&t, s)) }),
        ("chop", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = bytes(vm, s); let n = chop_len(&b, chars); let t = b[..n].to_vec(); Ok(vm.str_new_like(&t, s)) }),
        ("chars", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = bytes(vm, s); let parts: Vec<Vec<u8>> = chars_of(&b, chars).into_iter().map(|c| c.to_vec()).collect(); let items: Vec<Value> = parts.iter().map(|c| vm.str_new_like(c, s)).collect(); Ok(vm.ary_new(items)) }),
        ("bytes", |vm, s, _a, _b| { let items: Vec<Value> = bytes(vm, s).iter().map(|c| Value::Int(*c as i64)).collect(); Ok(vm.ary_new(items)) }),
        ("each_char", |vm, s, _a, b| { let chars = char_mode(vm, s); let src = bytes(vm, s); let parts: Vec<Vec<u8>> = chars_of(&src, chars).into_iter().map(|c| c.to_vec()).collect(); for c in parts { let ch = vm.str_new_like(&c, s); vm.call_block(b, &[ch])?; } Ok(s) }),
        ("each_byte", |vm, s, _a, b| { for c in bytes(vm, s) { vm.call_block(b, &[Value::Int(c as i64)])?; } Ok(s) }),
        ("getbyte", |vm, s, a, _b| { argc!(vm, a, 1); let i = vm.expect_int(a[0], "index")?; let b = sbytes(vm, s); let i = if i < 0 { i + b.len() as i64 } else { i }; Ok(b.get(i as usize).map(|c| Value::Int(*c as i64)).unwrap_or(Value::Nil)) }),
        ("ord", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = bytes(vm, s); if b.is_empty() { return Err(vm.raise_arg("empty string")); } Ok(Value::Int(char_code(vm, &b, 0, chars)?)) }),
        ("include?", |vm, s, a, _b| { argc!(vm, a, 1); let n = vm.expect_str(a[0], "argument")?; if !valid_chars(&n, char_mode(vm, a[0])) { return Ok(Value::False); } Ok(Value::bool(find(&bytes(vm, s), &n, 0).is_some())) }),
        ("index", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let n = vm.expect_str(a[0], "argument")?;
            let b = bytes(vm, s);
            let chars = char_mode(vm, s);
            let clen = char_len(&b, chars);
            let from = if a.len() == 2 { let f = vm.expect_int(a[1], "offset")?; if f < 0 { let f = f + clen as i64; if f < 0 { return Ok(Value::Nil); } f as usize } else { f as usize } } else { 0 };
            if from > clen { return Ok(Value::Nil); }
            // a needle whose bytes spell no character is found nowhere (`mrb_str_valid_encoding_p`)
            if !valid_chars(&n, char_mode(vm, a[0])) { return Ok(Value::Nil); }
            let from = char_to_byte(&b, from, chars);
            Ok(find(&b, &n, from).map(|i| Value::Int(byte_to_char(&b, i, chars) as i64)).unwrap_or(Value::Nil))
        }),
        ("rindex", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let n = vm.expect_str(a[0], "argument")?;
            let b = bytes(vm, s);
            let chars = char_mode(vm, s);
            // `pos` counts characters, and a negative one counts them back from the end
            let pos = if a.len() == 2 {
                let p = vm.expect_int(a[1], "pos")?;
                let ci = if p < 0 { let ci = p + char_len(&b, chars) as i64; if ci < 0 { return Ok(Value::Nil); } ci } else { p };
                char_to_byte(&b, ci as usize, chars)
            } else { b.len() };
            // see `byteindex`
            if !valid_chars(&n, char_mode(vm, a[0])) { return Ok(Value::Nil); }
            Ok(char_rfind(&b, &n, pos, chars).map(|i| Value::Int(byte_to_char(&b, i, chars) as i64)).unwrap_or(Value::Nil))
        }),
        ("start_with?", |vm, s, a, _b| { let b = bytes(vm, s); for p in a { let p = vm.expect_str(*p, "argument")?; if b.starts_with(&p) { return Ok(Value::True); } } Ok(Value::False) }),
        ("end_with?", |vm, s, a, _b| { let b = bytes(vm, s); for v in a { let p = vm.expect_str(*v, "argument")?; if !valid_chars(&p, char_mode(vm, *v)) { continue; } if b.ends_with(&p) { return Ok(Value::True); } } Ok(Value::False) }),
        ("split", str_split),
        // `str_replace`: the receiver takes the other string's bytes and the reading that goes
        // with them (`RSTR_ENC_CR_COPY`)
        ("replace", |vm, s, a, _b| { argc!(vm, a, 1); let b = vm.expect_str(a[0], "argument")?; set(vm, s, b)?; let binary = vm.str_binary(a[0]); vm.str_set_binary(s, binary); Ok(s) }),
        ("clear", |vm, s, _a, _b| { set(vm, s, vec![])?; Ok(s) }),
        ("dup", |vm, s, _a, _b| { let b = bytes(vm, s); let c = vm.real_class_of(s); let d = Value::Obj(vm.heap.alloc(c, ObjKind::String(b))); let binary = vm.str_binary(s); vm.str_set_binary(d, binary); Ok(d) }),
        ("freeze", |vm, s, _a, _b| { if let Some(o) = s.obj() { vm.heap.get_mut(o).frozen = true; } Ok(s) }),
        ("frozen?", |vm, s, _a, _b| Ok(Value::bool(s.obj().map(|o| vm.heap.get(o).frozen).unwrap_or(true)))),
        ("succ", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = str_succ_mode(&bytes(vm, s), chars); Ok(vm.str_new_like(&b, s)) }),
        ("next", |vm, s, _a, _b| { let chars = char_mode(vm, s); let b = str_succ_mode(&bytes(vm, s), chars); Ok(vm.str_new_like(&b, s)) }),
        ("center", |vm, s, a, _b| { argc!(vm, a, 1, 2); let w = vm.expect_int(a[0], "width")? as usize; let pad = if a.len() == 2 { vm.expect_str(a[1], "pad")? } else { b" ".to_vec() }; let chars = char_mode(vm, s); let b = bytes(vm, s); let clen = char_len(&b, chars); if w <= clen || pad.is_empty() { return Ok(vm.str_new(&b)); } let total = w - clen; let left = total / 2; let mut out = pad_chars(&pad, left, chars); out.extend(&b); out.extend(pad_chars(&pad, total - left, chars)); Ok(vm.str_new_like(&out, s)) }),
        ("ljust", |vm, s, a, _b| { argc!(vm, a, 1, 2); let w = vm.expect_int(a[0], "width")? as usize; let pad = if a.len() == 2 { vm.expect_str(a[1], "pad")? } else { b" ".to_vec() }; let chars = char_mode(vm, s); let mut b = bytes(vm, s); let clen = char_len(&b, chars); if w > clen && !pad.is_empty() { b.extend(pad_chars(&pad, w - clen, chars)); } Ok(vm.str_new_like(&b, s)) }),
        ("rjust", |vm, s, a, _b| { argc!(vm, a, 1, 2); let w = vm.expect_int(a[0], "width")? as usize; let pad = if a.len() == 2 { vm.expect_str(a[1], "pad")? } else { b" ".to_vec() }; let chars = char_mode(vm, s); let b = bytes(vm, s); let clen = char_len(&b, chars); if w <= clen || pad.is_empty() { return Ok(vm.str_new(&b)); } let mut out = pad_chars(&pad, w - clen, chars); out.extend(&b); Ok(vm.str_new_like(&out, s)) }),
        ("tr", |vm, s, a, _b| { argc!(vm, a, 2); let from = vm.expect_str(a[0], "from")?; let to = vm.expect_str(a[1], "to")?; let b: Vec<u8> = bytes(vm, s).iter().map(|c| match from.iter().position(|f| f == c) { Some(i) => *to.get(i).or(to.last()).unwrap_or(c), None => *c }).collect(); Ok(vm.str_new_like(&b, s)) }),
        ("delete", |vm, s, a, _b| { argc!(vm, a, 1); let del = vm.expect_str(a[0], "argument")?; let b: Vec<u8> = bytes(vm, s).into_iter().filter(|c| !del.contains(c)).collect(); Ok(vm.str_new_like(&b, s)) }),
        ("count", |vm, s, a, _b| { argc!(vm, a, 1); let set_ = vm.expect_str(a[0], "argument")?; Ok(Value::Int(bytes(vm, s).iter().filter(|c| set_.contains(c)).count() as i64)) }),
        ("__sub_replace", |vm, s, a, _b| {
            argc!(vm, a, 3);
            let (rep, pat) = (vm.expect_str(a[0], "replacement")?, vm.expect_str(a[1], "pattern")?);
            let found = vm.expect_int(a[2], "offset")? as usize;
            let me = bytes(vm, s);
            if found > me.len() { return Err(vm.raise(vm.core.runtime_error, "argument out of range")); }
            let out = sub_replace(&me, &rep, &pat, found);
            Ok(vm.str_new_like(&out, s))
        }),
        ("lines", |vm, s, _a, _b| { let b = bytes(vm, s); let mut items = vec![]; let mut start = 0; for (i, c) in b.iter().enumerate() { if *c == b'\n' { items.push(vm.str_new_like(&b[start..=i].to_vec(), s)); start = i + 1; } } if start < b.len() { items.push(vm.str_new_like(&b[start..].to_vec(), s)); } Ok(vm.ary_new(items)) }),
        ("__upto_endless", |vm, _s, _a, _b| Err(vm.raise(vm.core.not_implemented_error, "endless string range"))),
        ("upto", |vm, s, a, b| { argc!(vm, a, 1, 2); let last = vm.expect_str(a[0], "argument")?; let excl = a.len() == 2 && a[1].truthy(); let mut cur = bytes(vm, s); let mut n = 0; loop { if cur.len() > last.len() { break; } if cur == last { if !excl { let v = vm.str_new(&cur); vm.call_block(b, &[v])?; } break; } let v = vm.str_new(&cur); vm.call_block(b, &[v])?; cur = str_succ(&cur); n += 1; if n > 1_000_000 { break; } } Ok(s) }),
    ]);
    // `MRB_MT_PRIVATE` in the reference's ROM table for this class (src/string.c)
    vm.mark_private(c.string, &["initialize", "initialize_copy"]);
}

/// The String methods that must be registered after mrblib, because mrblib defines the same
/// names in Ruby and the later definition wins. With the feature `regexp` mruby-regexp's
/// `init` does this itself for the whole set it answers; this is the same move for the two
/// names a build without the gem still has (`Vm::load_mrblib`).
#[cfg(not(feature = "regexp"))]
pub fn post_mrblib(vm: &mut Vm) {
    let string = vm.core.string;
    vm.define_methods(string, &[
        ("sub", |vm, s, a, b| str_sub(vm, s, a, b, false)),
        ("gsub", |vm, s, a, b| str_sub(vm, s, a, b, true)),
    ]);
}

/// Length after `chomp(rs)`: no separator = one trailing newline (`\r\n`, `\n` or `\r`);
/// empty separator = all trailing newlines; otherwise the separator once.
fn chomp_bytes(b: &[u8], rs: Option<&[u8]>, chars: bool) -> usize {
    match rs {
        None => { if b.ends_with(b"\r\n") { b.len() - 2 } else if b.ends_with(b"\n") || b.ends_with(b"\r") { b.len() - 1 } else { b.len() } }
        Some(rs) if rs.is_empty() => { let mut n = b.len(); while n > 0 && b[n - 1] == b'\n' { n -= 1; if n > 0 && b[n - 1] == b'\r' { n -= 1; } } n }
        Some(rs) if rs == b"\n" => { if b.ends_with(b"\r\n") { b.len() - 2 } else if b.ends_with(b"\n") || b.ends_with(b"\r") { b.len() - 1 } else { b.len() } }
        // the separator is matched byte by byte, so it can line up with the tail of a
        // character; cutting there would leave bytes that spell nothing, so it is no match
        Some(rs) => { if b.ends_with(rs) && is_char_boundary(b, b.len() - rs.len(), chars) { b.len() - rs.len() } else { b.len() } }
    }
}

/// The bytes `chop` leaves: the last character goes, and `"\r\n"` counts as one
/// (`mrb_str_chop`).
fn chop_len(b: &[u8], chars: bool) -> usize {
    if b.ends_with(b"\r\n") { return b.len() - 2; }
    if b.is_empty() { return 0; }
    char_to_byte(b, char_len(b, chars) - 1, chars)
}

/// The code point of the character at `i`, refusing a run of bytes that spells none: mruby's
/// `mrb_utf8_decode` hands such a sequence back as its lead byte, and String says so rather than
/// answering with a character the string does not hold (`utf8code`).
pub(crate) fn char_code(vm: &mut Vm, b: &[u8], i: usize, chars: bool) -> VmResult<i64> {
    if chars && utf8len(b, i, chars) == 1 && b[i] >= 0x80 {
        return Err(vm.raise_arg("invalid UTF-8 byte sequence"));
    }
    Ok(code_point(b, i, chars) as i64)
}

fn str_eq(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    Ok(Value::bool(match vm.str_bytes(a[0]) { Some(o) => o == sbytes(vm, s), None => false }))
}

fn str_concat(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    let mut b = bytes(vm, s);
    match a[0] {
        Value::Int(i) => { if !(0..=255).contains(&i) { return Err(vm.raise(vm.core.range_error, &format!("{i} out of char range"))); } b.push(i as u8); }
        v => { let o = vm.expect_str(v, "argument")?; b.extend(o); }
    }
    set(vm, s, b)?;
    Ok(s)
}

/// `String#[]=`. Named, not a closure in the table, because `OP_SETIDX` records it as the
/// implementation it stands in for and calls it directly (`Vm::op_setidx`); mruby-regexp takes
/// the name and re-arms the slot on the same terms (`Vm::idx_op_rearm`).
pub(crate) fn str_aset(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    // the arguments are read in order, so a replacement that is no String is reported
    // before the count is (`mrb_str_aset_m`'s `mrb_get_args("oo|S!")`)
    if a.len() >= 3 && !a[2].is_nil() { vm.expect_str(a[2], "value")?; }
    argc!(vm, a, 2, 3);
    let mut b = bytes(vm, s);
    let val = vm.expect_str(a[a.len() - 1], "value")?;
    if a.len() == 2 { if let Some(n) = vm.str_bytes(a[0]).map(|n| n.to_vec()) { match find(&b, &n, 0) { Some(i) => { b.splice(i..i + n.len(), val.iter().copied()); set(vm, s, b)?; return Ok(a[1]); } None => { let d = vm.inspect_str(a[0])?; return Err(vm.raise(vm.core.index_error, &format!("string not matched: {d}"))); } } } }
    if a.len() == 3 { let n = vm.expect_int(a[1], "length")?; if n < 0 { return Err(vm.raise(vm.core.index_error, &format!("negative length {n}"))); } }
    // a numeric index may name the end of the string, where the assignment appends
    // (`mrb_str_aset` refuses `beg > charlen`, not `beg == charlen`), and a length
    // past the end is cut back to what is there
    let chars = char_mode(vm, s);
    let num = match a[0] { Value::Int(i) => Some(i), Value::Float(f) => Some(f as i64), _ => None };
    if let Some(i) = num {
        let clen = char_len(&b, chars);
        let i = if i < 0 { i + clen as i64 } else { i };
        if i < 0 || i > clen as i64 {
            let d = vm.inspect_str(a[0])?;
            return Err(vm.raise(vm.core.index_error, &format!("index {d} out of string")));
        }
        let i = i as usize;
        let n = if a.len() == 3 { (vm.expect_int(a[1], "length")? as usize).min(clen - i) } else { (clen - i).min(1) };
        let (bi, bn) = char_span(&b, i, n, chars);
        b.splice(bi..bi + bn, val.iter().copied());
        set(vm, s, b)?;
        return Ok(a[a.len() - 1]);
    }
    match index_args(vm, char_len(&b, chars), &a[..a.len() - 1])? {
        Some((i, n)) => { let (bi, bn) = char_span(&b, i, n, chars); b.splice(bi..bi + bn, val.iter().copied()); set(vm, s, b)?; Ok(a[a.len() - 1]) }
        None => { let d = vm.inspect_str(a[0])?; Err(vm.raise(vm.core.index_error, &format!("index {d} out of string"))) }
    }
}

pub(crate) fn str_aref(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    // The receiver's bytes are borrowed for each step and never copied whole: `index_args`
    // needs `&mut Vm` between them (it can call `to_int` and it can raise), so the borrow is
    // taken three times rather than held, and only the piece that comes out is allocated.
    if a.len() == 1 {
        if let Some(n) = vm.str_bytes(a[0]).map(|n| n.to_vec()) {
            // a needle whose bytes spell no character is found nowhere (`str_index_str`)
            if !valid_chars(&n, char_mode(vm, a[0])) { return Ok(Value::Nil); }
            let found = vm.str_bytes(s).map(|b| find(b, &n, 0).is_some()).unwrap_or(false);
            return Ok(if found { vm.str_new(&n) } else { Value::Nil });
        }
    }
    let chars = char_mode(vm, s);
    let len = vm.str_bytes(s).map(|b| char_len(b, chars)).unwrap_or(0);
    match index_args(vm, len, a)? {
        Some((i, n)) => {
            let piece = match vm.str_bytes(s) {
                Some(b) => { let (bi, bn) = char_span(b, i, n, chars); b[bi..bi + bn].to_vec() }
                None => Vec::new(),
            };
            Ok(vm.str_new_like(&piece, s))
        }
        None => Ok(Value::Nil),
    }
}

fn str_split(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0, 2);
    let b = bytes(vm, s);
    let chars = char_mode(vm, s);
    let limit = if a.len() == 2 { vm.expect_int(a[1], "limit")? } else { 0 };
    let sep: Option<Vec<u8>> = match a.first() { None | Some(Value::Nil) => None, Some(v) => Some(vm.expect_str(*v, "separator")?) };
    let mut parts: Vec<Vec<u8>> = vec![];
    match sep.as_deref() {
        None | Some(b" ") => {
            let mut cur: Vec<u8> = vec![];
            let mut i = 0;
            while i < b.len() {
                if b[i].is_ascii_whitespace() {
                    if !cur.is_empty() { parts.push(core::mem::take(&mut cur)); }
                    if limit > 0 && parts.len() as i64 == limit - 1 { let rest: Vec<u8> = b[i..].iter().skip_while(|c| c.is_ascii_whitespace()).copied().collect(); if !rest.is_empty() { parts.push(rest); } cur.clear(); break; }
                } else { cur.push(b[i]); }
                i += 1;
            }
            if !cur.is_empty() { parts.push(cur); }
        }
        Some(sep) if sep.is_empty() => {
            let mut cut = false;
            for (i, c) in chars_of(&b, chars).into_iter().enumerate() {
                if limit > 0 && parts.len() as i64 == limit - 1 { let rest: usize = chars_of(&b, chars)[..i].iter().map(|x| x.len()).sum(); parts.push(b[rest..].to_vec()); cut = true; break; }
                parts.push(c.to_vec());
            }
            // the split after the last character is a field of its own, which only a limit of 0
            // drops (`"abc".split("", -1)` is four fields)
            if !cut && limit != 0 && !b.is_empty() { parts.push(Vec::new()); }
        }
        Some(sep) => {
            let mut start = 0;
            while let Some(p) = find(&b, sep, start) {
                if limit > 0 && parts.len() as i64 == limit - 1 { break; }
                parts.push(b[start..p].to_vec());
                start = p + sep.len();
            }
            parts.push(b[start..].to_vec());
            if limit == 0 { while parts.last().map(|p| p.is_empty()).unwrap_or(false) { parts.pop(); } }
        }
    }
    let items: Vec<Value> = parts.iter().map(|p| vm.str_new_like(p, s)).collect();
    Ok(vm.ary_new(items))
}

/// Expands `\\`, `` \` ``, `\&`/`\0`, `\'` in a replacement (there are no regexp groups
/// here, so `\1`..`\9` expand to nothing) — `mrb_str_sub_replace`.
pub(crate) fn sub_replace(me: &[u8], rep: &[u8], pat: &[u8], found: usize) -> Vec<u8> {
    let mut out = vec![];
    let mut i = 0;
    while i < rep.len() {
        if rep[i] != b'\\' || i + 1 == rep.len() { out.push(rep[i]); i += 1; continue; }
        i += 1;
        match rep[i] {
            b'\\' => out.push(b'\\'),
            b'`' => out.extend_from_slice(&me[..found]),
            b'&' | b'0' => out.extend_from_slice(pat),
            b'\'' => { let off = found + pat.len(); if me.len() > off { out.extend_from_slice(&me[off..]); } }
            b'1'..=b'9' => {}
            c => { out.push(b'\\'); out.push(c); }
        }
        i += 1;
    }
    out
}

/// `sub`/`gsub` with a String pattern. mruby's mrblib writes these in Ruby with a mixture of
/// character and byte positions, which only works while the two are the same; in the
/// reference mruby-regexp replaces them with C ones, and this is the same move. Which
/// definition wins is decided after mrblib is loaded: with the feature `regexp` it is
/// mruby-regexp's (it answers a Regexp pattern too), without it [`post_mrblib`] puts these
/// in the mrblib ones' place, so that a String pattern reads the same in either build.
#[cfg(not(feature = "regexp"))]
fn str_sub(vm: &mut Vm, s: Value, a: &[Value], blk: Value, global: bool) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    if a.len() == 1 && blk.is_nil() {
        // `gsub(pattern)` with no block is an Enumerator; `sub` has no such form, and asks
        // for the replacement it was not given
        if !global { return Err(vm.raise_type("no implicit conversion of nil into String")); }
        let to_enum = vm.intern("to_enum");
        let name = vm.intern("gsub");
        return vm.funcall(s, to_enum, &[Value::Sym(name), a[0]], Value::Nil);
    }
    let pat = vm.expect_str(a[0], "pattern")?;
    let b = bytes(vm, s);
    // the answer holds bytes of the receiver, of the pattern and of the replacement, so it is
    // read as bytes exactly when one of those sources is byte-read and goes above ASCII, the
    // same as any other append (`str_sub_replace`)
    let ascii = |v: &[u8]| v.iter().all(|c| *c < 0x80);
    let mut binary = (vm.str_binary(s) && !ascii(&b)) || (vm.str_binary(a[0]) && !ascii(&pat));
    let mut out = vec![];
    let mut start = 0;
    while let Some(p) = find(&b, &pat, start) {
        out.extend_from_slice(&b[start..p]);
        let rep = if a.len() == 2 {
            let r = vm.expect_str(a[1], "replacement")?;
            binary |= vm.str_binary(a[1]) && !ascii(&r);
            sub_replace(&b, &r, &pat, p)
        } else {
            let m = vm.str_new_like(&pat, a[0]);
            let r = vm.call_block(blk, &[m])?;
            let rb = vm.as_string(r)?;
            binary |= vm.str_binary(r) && !ascii(&rb);
            rb
        };
        out.extend(rep);
        start = p + pat.len();
        // an empty pattern matches before every character, so the walk steps one along
        if pat.is_empty() {
            let n = if start < b.len() { utf8len(&b, start, char_mode(vm, s)) } else { 1 };
            if start < b.len() { out.extend_from_slice(&b[start..start + n]); }
            start += n;
            if start > b.len() { break; }
        }
        if !global { break; }
    }
    if start <= b.len() { out.extend_from_slice(&b[start.min(b.len())..]); }
    let r = vm.str_new(&out);
    vm.str_set_binary(r, binary);
    Ok(r)
}

/// What stepping one character came to (`enum succ_step`).
#[derive(Clone, Copy, PartialEq)]
enum Succ {
    /// Not one this walk steps; left as it is and the walk moves on.
    NotChar,
    /// Stepped in place; the walk is done.
    Found,
    /// Wrapped to the start of its run; the walk carries on.
    Wrapped,
}

/// `str_succ_bang`: the rightmost alphanumeric character steps; one that wraps carries into the
/// alphanumeric before it, across whatever is not one, but a letter does not carry into a digit
/// nor a digit into a letter across such a gap ("1.9" is "2.0" and "a-z" is "b-a", while "1-z"
/// is "1-aa"). A string with no alphanumeric steps its last character instead.
pub fn str_succ(b: &[u8]) -> Vec<u8> {
    str_succ_mode(b, cfg!(feature = "utf8"))
}

/// `chars` steps by characters (a string read as UTF-8) rather than by bytes (a byte-read one,
/// and every string of a build without the feature `utf8`).
pub fn str_succ_mode(b: &[u8], chars: bool) -> Vec<u8> {
    if b.is_empty() { return Vec::new(); }
    let mut v = b.to_vec();
    let mut carry: Vec<u8> = vec![1];
    let mut carry_pos = 0usize;
    let mut last_alnum: Option<usize> = None;
    let mut found_alnum = false;
    let mut step = Succ::Found;
    let mut p = v.len();
    while p > 0 {
        p = prev_char(&v, p, chars);
        if step == Succ::NotChar {
            if let Some(la) = last_alnum {
                let c = v[la];
                let stop = if c.is_ascii_alphabetic() { v[p].is_ascii_digit() }
                    else if c.is_ascii_digit() { v[p].is_ascii_alphabetic() }
                    else { false };
                if stop { break; }
            }
        }
        let len = succ_char_len(&v, p, chars);
        if len == 0 { continue; }
        step = succ_alnum(&mut v, p, len, &mut carry);
        match step {
            Succ::NotChar => continue,
            Succ::Found => return v,
            Succ::Wrapped => {}
        }
        last_alnum = Some(p);
        found_alnum = true;
        carry_pos = p;
    }
    // No alphanumeric: the last character steps instead, and one that wraps carries into the
    // character before it.
    if !found_alnum {
        let mut p = v.len();
        while p > 0 {
            p = prev_char(&v, p, chars);
            let len = succ_char_len(&v, p, chars);
            if len == 0 { continue; }
            if succ_char(&mut v, p, len, chars) == Succ::Found { return v; }
            carry_pos = p;
        }
    }
    // Everything that could carry has wrapped, so the carry goes in before the leftmost
    // character that did: "zz" to "aaa", and "ת" to "אא", whose carry is as wide as the run it
    // wrapped in.
    let mut out = Vec::with_capacity(v.len() + carry.len());
    out.extend_from_slice(&v[..carry_pos]);
    out.extend_from_slice(&carry);
    out.extend_from_slice(&v[carry_pos..]);
    out
}

/// Where the character before `p` starts, in the reading the string has (`succ_prev_char`).
fn prev_char(v: &[u8], p: usize, chars: bool) -> usize {
    if chars { char_head(v, p - 1, chars) } else { p - 1 }
}

/// How many bytes the character at `p` covers; 0 for a run of bytes that spells none, which the
/// walks step over without touching (`succ_char_len`).
fn succ_char_len(v: &[u8], p: usize, chars: bool) -> usize {
    if !chars { return 1; }
    let len = utf8len(v, p, chars);
    if len == 1 && v[p] >= 0x80 { 0 } else { len }
}

/// The first code point that takes `len` bytes in UTF-8 (`succ_first_of_len`).
#[cfg(feature = "utf8")]
fn first_of_len(len: usize) -> u32 {
    match len { 2 => 0x80, 3 => 0x800, 4 => 0x10000, _ => 0 }
}

/// Whether `cp` spells a character of `len` bytes, writing it at `p` when it does. A surrogate
/// spells no character, and neither does the byte length growing, since a wider character would
/// move every character behind it (`succ_utf8_write`).
#[cfg(feature = "utf8")]
fn succ_write(v: &mut [u8], p: usize, len: usize, cp: u32) -> bool {
    if (0xd800..=0xdfff).contains(&cp) { return false; }
    let buf = from_code_point(cp);
    if buf.len() != len { return false; }
    v[p..p + len].copy_from_slice(&buf);
    true
}

/// The code point after `cp`, over the surrogates, which spell no character (`succ_next_cp`).
#[cfg(feature = "utf8")]
fn next_cp(cp: u32) -> u32 { if cp + 1 == 0xd800 { 0xe000 } else { cp + 1 } }

/// Steps the character at `p` as an alphanumeric, which is what `String#succ` looks for first: a
/// letter or a digit steps within its own run, to the next one of its kind where there is one and
/// back to the first of the run where there is not, leaving the run's carry (its first member, or
/// the digit after it) in `carry` for the character before it. Which characters those are above
/// ASCII is [`str_alnum`]'s to say (`succ_alnum`).
fn succ_alnum(v: &mut [u8], p: usize, len: usize, carry: &mut Vec<u8>) -> Succ {
    if len == 1 {
        let c = v[p];
        if !c.is_ascii_alphanumeric() { return Succ::NotChar; }
        *carry = vec![match c { b'9' => b'1', b'z' => b'a', b'Z' => b'A', _ => { v[p] = c + 1; return Succ::Found; } }];
        v[p] = match c { b'9' => b'0', b'z' => b'a', _ => b'A' };
        return Succ::Wrapped;
    }
    #[cfg(feature = "utf8")]
    {
        let cp = code_point(v, p, true);
        let (kind, start) = super::str_alnum::run_of(cp);
        if kind == 0 { return Succ::NotChar; }
        // the step over one code point that is not of the kind is CRuby's too: it is what takes
        // "Ρ" to "Σ" over the unassigned U+03A2
        let mut next = cp;
        for _ in 0..2 {
            next = next_cp(next);
            if super::str_alnum::run_of(next).0 == kind && succ_write(v, p, len, next) { return Succ::Found; }
        }
        // Nothing of the kind is left at this byte length, so the run wraps. A character alone in
        // its run has nowhere to wrap to and is not one this walk steps.
        let first = start.max(first_of_len(len));
        if first == cp { return Succ::NotChar; }
        succ_write(v, p, len, first);
        *carry = from_code_point(if kind == super::str_alnum::DIGIT { first + 1 } else { first });
        return Succ::Wrapped;
    }
    #[allow(unreachable_code)]
    { let _ = (v, p, carry); Succ::NotChar }
}

/// Steps the character at `p` as a character, which is what `String#succ` falls back to when the
/// string holds no alphanumeric: the next one that spells a character of the same byte length
/// (`succ_char`).
fn succ_char(v: &mut [u8], p: usize, len: usize, chars: bool) -> Succ {
    #[cfg(feature = "utf8")]
    if chars {
        let cp = code_point(v, p, true);
        if succ_write(v, p, len, next_cp(cp)) { return Succ::Found; }
        succ_write(v, p, len, first_of_len(len));
        return Succ::Wrapped;
    }
    let _ = (len, chars);
    if v[p] == 0xFF { v[p] = 0; return Succ::Wrapped; }
    v[p] += 1;
    Succ::Found
}
