//! mruby-regexp (`mrbgems/mruby-regexp/src/regexp.c`): the `Regexp` and `MatchData` classes and
//! the String and Symbol methods whose regexp form the gem answers. The engine under them is
//! [`crate::regexp`].
//!
//! `$~` is the one name a match publishes; `$&`, `` $` ``, `$'`, `$+` and `$1` onward are
//! readings of it the compiler derives (`gen_match_ref`), so publishing and clearing are each
//! one write of the special variable the owning scope holds (`Vm::svar_set`).

use alloc::{format, string::String, vec, vec::Vec};

use crate::argc;
use crate::error::VmResult;
use crate::object::ObjKind;
use alloc::sync::Arc;

use crate::regexp::{self, Pattern};
use crate::value::Value;
use crate::vm::Vm;

// ------------------------------------------------------------------ helpers

fn bytes(vm: &Vm, v: Value) -> Vec<u8> { vm.str_bytes(v).map(|b| b.to_vec()).unwrap_or_default() }

/// The pattern text a reader answers from, or TypeError when there is none (`re_check_initialized`).
fn check_initialized(vm: &mut Vm, re: Value) -> VmResult<Vec<u8>> {
    let src = re.obj().map(|o| vm.heap.ivar_get(o, vm.s.source)).unwrap_or(Value::Nil);
    let has_pat = re.obj().map(|o| matches!(vm.heap.get(o).kind, ObjKind::Regexp(_))).unwrap_or(false);
    match (has_pat, vm.str_bytes(src)) {
        (true, Some(b)) => Ok(b.to_vec()),
        _ => Err(vm.raise_type("uninitialized Regexp")),
    }
}

/// The compiled pattern of a Regexp, refusing one that has none (`re_search_binary`'s second
/// half: `DATA_GET_PTR` then `re_uninitialized_p`).
///
/// The pattern comes back as a shared handle rather than a reference into the heap: the callers
/// search with it while they go on using the VM (`&mut Vm`: MatchData, `$~`, and in `gsub` a block
/// that runs any Ruby), and the handle keeps the pattern alive whatever happens to the object
/// meanwhile.
fn pattern_of(vm: &mut Vm, re: Value) -> VmResult<Arc<Pattern>> {
    if let Some(o) = re.obj() {
        match &vm.heap.get(o).kind {
            ObjKind::Regexp(Some(p)) => return Ok(Arc::clone(p)),
            // a compile that raised leaves the slot behind and no pattern, which is the object
            // saying it holds nothing to search with (`re_uninitialized_p`)
            ObjKind::Regexp(None) => return Err(vm.raise_arg("uninitialized Regexp")),
            _ => {}
        }
    }
    // an object that never went through `initialize` holds no pattern at all, which is what
    // `DATA_GET_PTR` reports
    Err(vm.raise_type("uninitialized Regexp"))
}

/// Internal flags of a Regexp (`get_iflags`).
fn iflags(vm: &mut Vm, re: Value) -> u32 {
    match re.obj().map(|o| vm.heap.ivar_get(o, vm.s.rflags)) {
        Some(Value::Int(i)) => i as u32,
        _ => 0,
    }
}

/// Flags from a string or an integer argument (`parse_flags`).
fn parse_flags(vm: &mut Vm, v: Value) -> u32 {
    let mut flags = 0;
    match v {
        Value::Int(f) => {
            if f & 1 != 0 { flags |= regexp::FLAG_IGNORECASE; }
            if f & 2 != 0 { flags |= regexp::FLAG_EXTENDED; }
            if f & 4 != 0 { flags |= regexp::FLAG_MULTILINE | regexp::FLAG_DOTALL; }
            flags
        }
        v if vm.str_bytes(v).is_some() => {
            for c in bytes(vm, v) {
                match c {
                    b'i' => flags |= regexp::FLAG_IGNORECASE,
                    b'm' => flags |= regexp::FLAG_MULTILINE | regexp::FLAG_DOTALL,
                    b'x' => flags |= regexp::FLAG_EXTENDED,
                    _ => {}
                }
            }
            flags
        }
        v => { if v.truthy() { flags |= regexp::FLAG_IGNORECASE; } flags }
    }
}

/// Whether a string is byte-read, which is the one thing the compiler asks of the pattern it is
/// handed (`re_pattern_binary`).
fn pattern_binary(vm: &Vm, str: Value) -> bool { vm.str_binary(str) }

/// How a search reads the subject: by byte when it is byte-read. A subject holding a byte that
/// spells no character is refused, as CRuby refuses one (`re_subject_binary`).
fn subject_binary(vm: &mut Vm, str: Value, unread: bool) -> VmResult<bool> {
    let b = bytes(vm, str);
    if !unread && !crate::builtins::string::valid_chars(&b, crate::builtins::string::char_mode(vm, str)) {
        return Err(vm.raise_arg("invalid byte sequence in UTF-8"));
    }
    Ok(vm.str_binary(str))
}

/// A byte range of the subject, as `mrb_str_byte_subseq` hands one out: the engine records every
/// offset in bytes (`re_byte_substr`).
fn byte_substr(vm: &mut Vm, str: Value, beg: i32, len: i32) -> Value {
    let b = bytes(vm, str);
    if beg < 0 || len < 0 || (beg + len) as usize > b.len() { return Value::Nil; }
    let piece = b[beg as usize..(beg + len) as usize].to_vec();
    vm.str_new_like(&piece, str)
}

/// A byte offset as a character offset, so `MatchData#begin` and `#end` report character
/// positions (`re_byte_to_char`).
fn byte_to_char(vm: &Vm, str: Value, byte_off: i32) -> i32 {
    if byte_off < 0 { return byte_off; }
    let b = bytes(vm, str);
    let chars = crate::builtins::string::char_mode(vm, str);
    let off = (byte_off as usize).min(b.len());
    crate::builtins::string::byte_to_char(&b, off, chars) as i32
}

/// A `pos` argument as a byte offset, or `None` for one out of range, which Ruby treats as no
/// match (`re_char_to_byte`).
fn char_to_byte(vm: &Vm, str: Value, char_off: i64) -> Option<usize> {
    let b = bytes(vm, str);
    let chars = crate::builtins::string::char_mode(vm, str);
    let mut off = char_off;
    if off < 0 {
        off += crate::builtins::string::char_len(&b, chars) as i64;
        if off < 0 { return None; }
    }
    let byte_off = crate::builtins::string::char_to_byte(&b, off as usize, chars);
    if byte_off > b.len() { return None; }
    Some(byte_off)
}

/// The string a match operates on: a Symbol is matched against its name (`match_operand`).
fn match_operand(vm: &mut Vm, obj: Value) -> VmResult<Value> {
    if let Value::Sym(s) = obj {
        let name = vm.sym_name(s).as_bytes().to_vec();
        return Ok(vm.str_new(&name));
    }
    if vm.str_bytes(obj).is_some() { return Ok(obj); }
    let d = vm.describe_for_type_error(obj);
    Err(vm.raise_type(&format!("{d} cannot be converted to String")))
}

/// Make a MatchData from the captures and publish it as `$~` (`create_matchdata`).
fn create_matchdata(vm: &mut Vm, regexp: Value, str: Value, captures: Vec<i32>) -> Value {
    // the subject is snapshot: a MatchData reports the string as it was at match time
    let b = bytes(vm, str);
    let snap = vm.str_new_like(&b, str);
    if let Some(o) = snap.obj() { vm.heap.get_mut(o).frozen = true; }
    let md = vm.heap.alloc(vm.core.match_data, ObjKind::MatchData {
        source: crate::value::Slot::from(snap),
        regexp: crate::value::Slot::from(regexp),
        captures,
    });
    let md = Value::Obj(md);
    vm.svar_set(md);
    md
}

fn clear_match(vm: &mut Vm) { vm.svar_set(Value::Nil); }

/// The pieces of a MatchData.
fn md_parts(vm: &Vm, md: Value) -> Option<(Value, Value, Vec<i32>)> {
    let o = md.obj()?;
    match &vm.heap.get(o).kind {
        ObjKind::MatchData { source, regexp, captures } => Some((source.get(), regexp.get(), captures.clone())),
        _ => None,
    }
}

fn md_check(vm: &mut Vm, md: Value) -> VmResult<(Value, Value, Vec<i32>)> {
    match md_parts(vm, md) {
        Some(x) => Ok(x),
        None => Err(vm.raise_type("uninitialized MatchData")),
    }
}

// ------------------------------------------------------------------ searching

/// Execute a match and make a MatchData from it, publishing it as `$~` and clearing the name on
/// a miss (`exec_match`). `literal` says the caller quoted a String pattern into `re` to have
/// something to search with, and such a match carries no Regexp.
fn exec_match(vm: &mut Vm, re: Value, str: Value, pos: usize, unread: bool, literal: bool) -> VmResult<Value> {
    let binary = subject_binary(vm, str, unread)?;
    let pat = pattern_of(vm, re)?;
    let b = bytes(vm, str);
    let caps = regexp::exec(&pat, &b, pos, true);
    let _ = binary;
    match caps {
        None => { clear_match(vm); Ok(Value::Nil) }
        Some(caps) => Ok(create_matchdata(vm, if literal { Value::Nil } else { re }, str, caps)),
    }
}

/// The search of every String-side entry point (`re_search`).
fn re_search(vm: &mut Vm, re: Value, str: Value, pos: i64, literal: bool) -> VmResult<Value> {
    if str.is_nil() { clear_match(vm); return Ok(Value::Nil); }
    let str = match_operand(vm, str)?;
    let Some(pos) = char_to_byte(vm, str, pos) else { clear_match(vm); return Ok(Value::Nil) };
    exec_match(vm, re, str, pos, literal, literal)
}

/// The backward search of `rindex`, `byterindex` and `rpartition`: the last match that starts at
/// or before `limit`, a byte offset (`re_byte_rsearch`).
fn re_byte_rsearch(vm: &mut Vm, re: Value, str: Value, limit: usize) -> VmResult<Value> {
    let binary = subject_binary(vm, str, false)?;
    let pat = pattern_of(vm, re)?;
    let b = bytes(vm, str);
    let caps = regexp::rexec(&pat, &b, limit, true);
    let _ = binary;
    match caps {
        None => { clear_match(vm); Ok(Value::Nil) }
        Some(caps) => Ok(create_matchdata(vm, re, str, caps)),
    }
}

/// The search of `match?`, which allocates no MatchData and leaves `$~` alone (`exec_match_p`).
fn exec_match_p(vm: &mut Vm, re: Value, str: Value, pos: i64) -> VmResult<Value> {
    if str.is_nil() { return Ok(Value::False); }
    let str = match_operand(vm, str)?;
    let Some(pos) = char_to_byte(vm, str, pos) else { return Ok(Value::False) };
    let binary = subject_binary(vm, str, false)?;
    let pat = pattern_of(vm, re)?;
    let b = bytes(vm, str);
    let caps = regexp::exec(&pat, &b, pos, false);
    let _ = binary;
    Ok(Value::bool(caps.is_some()))
}

// ------------------------------------------------------------------ Regexp

/// Compile `pattern` under `flags` into `self` and publish everything a Regexp answers from
/// (`re_initialize`).
fn re_initialize(vm: &mut Vm, self_: Value, pattern: Value, flags: u32) -> VmResult<Value> {
    let o = match self_.obj() { Some(o) => o, None => return Err(vm.raise_type("not a Regexp")) };
    // an object holds one pattern and owns it, so a second initialize cannot compile over it
    if matches!(vm.heap.get(o).kind, ObjKind::Regexp(_)) {
        return Err(vm.raise_type("already initialized regexp"));
    }
    vm.heap.ivar_set(o, vm.s.source, pattern);
    vm.heap.ivar_set(o, vm.s.rflags, Value::Int(flags as i64));
    // `@source` and `@flags` are set before the compile, so a Regexp that survives a
    // compile-time exception still answers `hash`, `eql?` and `inspect` from what it has
    // (`re_initialize`). The pattern slot is opened before the compile too, since what says the
    // object was initialized is the slot and not the source it may have inherited from a copy.
    vm.heap.get_mut(o).kind = ObjKind::Regexp(None);
    let src = bytes(vm, pattern);
    let binary = pattern_binary(vm, pattern);
    let pat = match regexp::compile(&src, flags, binary) {
        Ok(p) => p,
        Err(e) => return Err(vm.raise(vm.core.regexp_error, &compile_error_message(&e, &src, flags))),
    };
    // the named captures, as a hash of name -> [group, ...]: a name may be given to several
    // groups, and each keeps its own number
    let named = vm.intern("@named_captures");
    if !pat.named_captures.is_empty() {
        let h = vm.hash_new();
        for nc in &pat.named_captures {
            let name = vm.str_new(&nc.name);
            if let Some(o) = name.obj() { vm.heap.get_mut(o).frozen = true; }
            let cur = vm.hash_get(h, name).unwrap_or(Value::Nil);
            let list = if cur.is_nil() {
                let a = vm.ary_new(vec![]);
                vm.hash_set(h, name, a)?;
                a
            } else { cur };
            if let Some(lo) = list.obj() {
                if let ObjKind::Array(v) = &mut vm.heap.get_mut(lo).kind {
                    v.push(crate::value::Slot::from(Value::Int(nc.group as i64)));
                }
            }
        }
        vm.heap.ivar_set(o, named, h);
    } else {
        // the table belongs to the pattern compiled just above, so one that names nothing has to
        // leave nothing behind (a copy arrives with the original's table already on it)
        vm.heap.get_mut(o).ivars.retain(|(k, _)| *k != named);
    }
    vm.heap.get_mut(o).kind = ObjKind::Regexp(Some(Arc::new(pat)));
    Ok(self_)
}

/// The bytes of `str` with everything a pattern reads as syntax escaped (`re_escape_str`).
fn escape_str(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len() + b.len() / 4);
    for &c in b {
        match c {
            // control characters become two-character escapes so the result stays printable
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0b => out.extend_from_slice(b"\\v"),
            // `#`, `-` and space are only special under `/x` or inside `[...]`, but escaping them
            // unconditionally keeps the result literal in every mode
            b'\\' | b'.' | b'*' | b'+' | b'?' | b'|' | b'(' | b')' | b'[' | b']' | b'{' | b'}'
            | b'^' | b'$' | b'#' | b'-' | b' ' => { out.push(b'\\'); out.push(c); }
            _ => out.push(c),
        }
    }
    out
}

/// The options one letter of an inline group names (`re_option_letter_bits`).
fn option_letter_bits(c: u8) -> u32 {
    match c {
        b'i' => regexp::FLAG_IGNORECASE,
        b'm' => regexp::FLAG_MULTILINE | regexp::FLAG_DOTALL,
        b'x' => regexp::FLAG_EXTENDED,
        _ => 0,
    }
}

/// What a refused compile raises: the complaint, then the pattern as written and the flags it
/// was compiled with, the way the reference quotes them (`compile_error_str`).
fn compile_error_message(msg: &str, src: &[u8], flags: u32) -> String {
    let mut out = String::from(msg);
    out.push_str(": /");
    out.push_str(&alloc::string::String::from_utf8_lossy(src));
    out.push('/');
    flags_cat(&mut out, flags);
    out
}

/// The set flags as letters, in the order `Regexp#to_s` and `#inspect` write them
/// (`mrb_re_flags_cat`).
fn flags_cat(out: &mut String, flags: u32) {
    for (bit, letter) in FLAG_LETTERS {
        if flags & bit != 0 { out.push(letter as char); }
    }
}

/// The flag letters of the displayed forms, in the order Ruby writes them (`re_flag_letters`).
const FLAG_LETTERS: [(u32, u8); 3] = [
    (regexp::FLAG_MULTILINE, b'm'),
    (regexp::FLAG_IGNORECASE, b'i'),
    (regexp::FLAG_EXTENDED, b'x'),
];

/// Fold a leading option group into the flags to print, which is what makes `/(?i)a/` and `/a/i`
/// print alike (`re_fold_leading_group`).
fn fold_leading_group(src: &[u8], flags: u32, binary: bool) -> (usize, usize, u32) {
    let orig_flags = flags;
    let (mut ptr, mut len, mut flags) = (0usize, src.len(), flags);
    while len >= 4 && src[ptr] == b'(' && src[ptr + 1] == b'?' {
        let mut p = ptr + 2;
        let mut n = len - 2;
        let mut on = flags;
        while n > 0 && option_letter_bits(src[p]) != 0 {
            on |= option_letter_bits(src[p]);
            p += 1; n -= 1;
        }
        // a `-` with nothing after it names no letter to turn off
        if n > 1 && src[p] == b'-' {
            p += 1; n -= 1;
            while n > 0 && option_letter_bits(src[p]) != 0 {
                on &= !option_letter_bits(src[p]);
                p += 1; n -= 1;
            }
        }
        if n > 0 && src[p] == b')' {
            flags = on;
            ptr = p + 1;
            len = n - 1;
            continue;
        }
        // a scoped group folds only when what it encloses is the whole source, which one trial
        // compile settles
        if n >= 2 && src[p] == b':' && src[p + n - 1] == b')'
            && regexp::compile(&src[p + 1..p + n - 1], on, binary).is_ok()
        {
            flags = on;
            ptr = p + 1;
            len = n - 2;
        } else {
            // a group that folds neither way is not the only thing left as written: the toggles
            // already peeled ahead of it go back too
            return (0, src.len(), orig_flags);
        }
        break;
    }
    (ptr, len, flags)
}

/// The group a name refers to in the match the captures stand for, or `None` for a name the
/// pattern gives to no group: the candidates are walked back to front and the first that
/// participated is the answer (`re_name_to_group`).
fn name_to_group(captures: &[i32], pat: Option<&Pattern>, name: &[u8]) -> Option<u16> {
    let pat = pat?;
    let mut fallback = None;
    for nc in pat.named_captures.iter().rev() {
        if nc.name == name {
            if fallback.is_none() { fallback = Some(nc.group); }
            if (nc.group as usize) * 2 < captures.len() && captures[nc.group as usize * 2] >= 0 {
                return Some(nc.group);
            }
        }
    }
    fallback
}

/// The pattern a MatchData was made with, where it has one.
fn md_pattern(vm: &Vm, re: Value) -> Option<Arc<Pattern>> {
    let o = re.obj()?;
    match &vm.heap.get(o).kind {
        ObjKind::Regexp(Some(p)) => Some(Arc::clone(p)),
        _ => None,
    }
}

/// Resolve a String or Symbol to the group it names, raising where the name reaches none
/// (`matchdata_name_to_group`).
fn md_name_to_group(vm: &mut Vm, md: Value, arg: Value) -> VmResult<usize> {
    let (_, re, caps) = md_check(vm, md)?;
    let name = match arg {
        Value::Sym(s) => vm.sym_name(s).as_bytes().to_vec(),
        v => bytes(vm, v),
    };
    let pat = md_pattern(vm, re);
    let group = pat.and_then(|p| name_to_group(&caps, Some(&p), &name));
    match group {
        Some(g) => Ok(g as usize),
        // a name that resolves to no group is a mistake at the point of the call
        None => {
            let shown = String::from_utf8_lossy(&name).into_owned();
            Err(vm.raise(vm.core.index_error, &format!("undefined group name reference: {shown}")))
        }
    }
}

/// The group at the absolute index, or nil when it names no group of the match (`md_nth`).
fn md_nth(vm: &mut Vm, md: Value, idx: i64) -> Value {
    let Some((src, _, caps)) = md_parts(vm, md) else { return Value::Nil };
    let n = caps.len() / 2;
    if idx < 0 || idx as usize >= n { return Value::Nil; }
    let (s, e) = (caps[idx as usize * 2], caps[idx as usize * 2 + 1]);
    if s < 0 { return Value::Nil; }
    byte_substr(vm, src, s, e - s)
}

/// The arguments `begin`, `end` and `offset` read: an index they cannot use is an error rather
/// than a missing result (`matchdata_group_arg`).
fn md_group_arg(vm: &mut Vm, md: Value, arg: Value) -> VmResult<usize> {
    if matches!(arg, Value::Sym(_)) || vm.str_bytes(arg).is_some() {
        return md_name_to_group(vm, md, arg);
    }
    let (_, _, caps) = md_check(vm, md)?;
    let idx = vm.expect_int(arg, "index")?;
    if idx < 0 || idx as usize >= caps.len() / 2 {
        return Err(vm.raise(vm.core.index_error, &format!("index {idx} out of matches")));
    }
    Ok(idx as usize)
}

/// Splice a replacement string, expanding the escapes that stand for a group
/// (`apply_replacement`).
#[allow(clippy::too_many_arguments)]
fn apply_replacement(vm: &mut Vm, out: &mut Vec<u8>, rep: &[u8], s: &[u8], captures: &[i32],
                     pat: Option<&Pattern>) -> VmResult<()> {
    let ncap = captures.len() / 2;
    let named = pat.is_some_and(|p| !p.named_captures.is_empty());
    let mut i = 0;
    while i < rep.len() {
        if rep[i] == b'\\' && i + 1 < rep.len() {
            let c = rep[i + 1];
            // the escapes that stand for a group settle on one here
            let mut g: i64 = -1;
            let mut is_ref = true;
            let mut next = i + 2;
            if c.is_ascii_digit() {
                g = (c - b'0') as i64;
                // a pattern that names a group turns `\1` through `\9` off
                if g > 0 && named { g = -1; }
            } else if c == b'&' {
                g = 0;
            } else if c == b'k' && i + 2 < rep.len() && rep[i + 2] == b'<' {
                // a group named where `\1` numbers one
                let from = i + 3;
                let Some(off) = rep[from..].iter().position(|&x| x == b'>') else {
                    return Err(vm.raise(vm.core.runtime_error, "invalid group name reference format"));
                };
                let name = &rep[from..from + off];
                let found = pat.and_then(|p| name_to_group(captures, Some(p), name));
                match found {
                    Some(x) => g = x as i64,
                    None => {
                        let shown = String::from_utf8_lossy(name).into_owned();
                        return Err(vm.raise(vm.core.index_error, &format!("undefined group name reference: {shown}")));
                    }
                }
                next = from + off + 1;
            } else if c == b'+' {
                // the last successful capture
                for j in (1..ncap).rev() {
                    if captures[j * 2] >= 0 { g = j as i64; break; }
                }
            } else {
                is_ref = false;
            }
            if is_ref {
                if g >= 0 && (g as usize) < ncap && captures[g as usize * 2] >= 0 {
                    let (a, b) = (captures[g as usize * 2] as usize, captures[g as usize * 2 + 1] as usize);
                    out.extend_from_slice(&s[a..b]);
                }
            } else if c == b'`' {
                if captures[0] >= 0 { out.extend_from_slice(&s[..captures[0] as usize]); }
            } else if c == b'\'' {
                if captures[1] >= 0 { out.extend_from_slice(&s[captures[1] as usize..]); }
            } else if c == b'\\' {
                out.push(b'\\');
            } else {
                out.extend_from_slice(&rep[i..i + 2]); // `\x` as it stands
            }
            i = next;
        } else {
            let mut j = i + 1;
            while j < rep.len() && rep[j] != b'\\' { j += 1; }
            out.extend_from_slice(&rep[i..j]);
            i = j;
        }
    }
    Ok(())
}

/// What `sub` and `gsub` build is the subject's bytes with the replacement spliced in, so it is
/// read the way the subject was; a replacement that was read as bytes and goes above ASCII hands
/// its reading over (`re_mark_spliced`).
fn mark_spliced(vm: &mut Vm, result: Value, subject: Value, replacement: Value, spliced: bool) {
    if !vm.str_binary(subject) {
        if !spliced || !vm.str_binary(replacement) { return; }
        let r = bytes(vm, replacement);
        if r.iter().all(|c| *c < 0x80) { return; }
    }
    vm.str_set_binary(result, true);
}

/// `gsub` with a String replacement (`re_gsub_str`).
fn gsub_str(vm: &mut Vm, re: Value, str: Value, replacement: Value) -> VmResult<Value> {
    let binary = subject_binary(vm, str, false)?;
    let pat = pattern_of(vm, re)?;
    let s = bytes(vm, str);
    let rep = bytes(vm, replacement);
    let need_expand = rep.contains(&b'\\');
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut pos = 0usize;
    let mut last: Option<Vec<i32>> = None;
    while pos <= s.len() {
        let caps = regexp::exec(&pat, &s, pos, true);
        let Some(caps) = caps else { break };
        last = Some(caps.clone());
        if caps[0] as usize > pos { out.extend_from_slice(&s[pos..caps[0] as usize]); }
        if need_expand {
            apply_replacement(vm, &mut out, &rep, &s, &caps, Some(&pat))?;
        } else {
            out.extend_from_slice(&rep);
        }
        // an empty match steps one character on, copying it, so the next search does not find the
        // same one again
        if caps[1] == caps[0] {
            let at = caps[1] as usize;
            if at < s.len() {
                let clen = regexp::charlen(&s, at, binary);
                out.extend_from_slice(&s[at..at + clen]);
                pos = at + clen;
            } else {
                pos = at + 1;
            }
        } else {
            pos = caps[1] as usize;
        }
    }
    if pos <= s.len() { out.extend_from_slice(&s[pos.min(s.len())..]); }
    let result = vm.str_new(&out);
    let spliced = last.is_some();
    match last {
        Some(caps) => { create_matchdata(vm, re, str, caps); }
        None => clear_match(vm),
    }
    mark_spliced(vm, result, str, replacement, spliced);
    Ok(result)
}

/// `sub`/`gsub` with a String replacement over a quoted literal pattern (`re_gsub_lit`,
/// `re_sub_lit`): a literal search reads the subject byte by byte and records a match with group
/// 0 alone.
fn lit_search(s: &[u8], p: &[u8], pos: usize) -> Option<usize> {
    if p.is_empty() { return if pos <= s.len() { Some(pos) } else { None }; }
    if s.len() < p.len() { return None; }
    (pos..=s.len() - p.len()).find(|&i| &s[i..i + p.len()] == p)
}

/// The MatchData a literal search leaves: group 0 is the whole of it, and no Regexp goes with it
/// (`re_lit_matchdata`).
fn lit_matchdata(vm: &mut Vm, str: Value, beg: usize, end: usize) {
    create_matchdata(vm, Value::Nil, str, vec![beg as i32, end as i32]);
}

/// The piece a block or a Hash answers for one match (`sub_piece`).
fn sub_piece(vm: &mut Vm, block: Value, hash: Value, matched: Value) -> VmResult<Vec<u8>> {
    let piece = if hash.is_nil() {
        vm.call_block(block, &[matched])?
    } else {
        // the lookup is the Hash's own, so a default, or a default proc reaching the receiver,
        // answers where the key is missing
        let aref = vm.intern("[]");
        vm.funcall(hash, aref, &[matched], Value::Nil)?
    };
    vm.as_string(piece)
}

/// Whether the receiver still reads as it did when the match was made (`re_subject_reads_as`).
fn subject_reads_as(vm: &mut Vm, str: Value, md: Value) -> bool {
    let Some((src, _, _)) = md_parts(vm, md) else { return false };
    if vm.str_binary(str) != vm.str_binary(src) { return false; }
    bytes(vm, str) == bytes(vm, src)
}

/// The walk `gsub` with a block or a Hash makes (`re_gsub_walk`). The subject is read afresh
/// after every block call: what the block did to the receiver while it had it is what the bytes
/// before the next match are taken from, and a change of length stops the walk.
fn gsub_walk(vm: &mut Vm, re: Value, str: Value, literal: bool, block: Value, hash: Value) -> VmResult<Value> {
    let mut binary = subject_binary(vm, str, literal)?;
    let pat = pattern_of(vm, re)?;
    let mut out: Vec<u8> = Vec::new();
    let (mut pos, mut last) = (0usize, 0usize);
    let mut last_md = Value::Nil;
    let mut s = bytes(vm, str);
    let slen = s.len();
    // the result is read the way the bytes put into it are (`re_cat_bytes`)
    let mut out_binary = false;
    while pos <= slen {
        let caps = regexp::exec(&pat, &s, pos, true);
        let Some(caps) = caps else { break };
        let (beg, end) = (caps[0] as usize, caps[1] as usize);
        let matched = byte_substr(vm, str, caps[0], caps[1] - caps[0]);
        last_md = create_matchdata(vm, if literal { Value::Nil } else { re }, str, caps.clone());
        last = pos;
        let piece = sub_piece(vm, block, hash, matched)?;
        // a change of length moved every offset the walk holds, and the walk stops there
        if bytes(vm, str).len() != slen {
            return Err(vm.raise(vm.core.runtime_error, "string modified"));
        }
        s = bytes(vm, str);
        binary = subject_binary(vm, str, literal)?;
        // after the block and not before it: the bytes before the match are taken from the
        // receiver as the block left it
        if beg > pos {
            if binary && s[pos..beg].iter().any(|c| *c >= 0x80) { out_binary = true; }
            out.extend_from_slice(&s[pos..beg]);
        }
        out.extend_from_slice(&piece);
        // a zero-width match carries the character it stood before
        if beg == end {
            if end < slen {
                let clen = regexp::charlen(&s, end, binary);
                if binary && s[end..end + clen].iter().any(|c| *c >= 0x80) { out_binary = true; }
                out.extend_from_slice(&s[end..end + clen]);
                pos = end + clen;
            } else {
                pos = end + 1;
            }
        } else {
            pos = end;
        }
    }
    if pos < slen {
        if binary && s[pos..].iter().any(|c| *c >= 0x80) { out_binary = true; }
        out.extend_from_slice(&s[pos..]);
    }
    let result = vm.str_new(&out);
    if out_binary { vm.str_set_binary(result, true); }
    if last_md.is_nil() {
        // a walk that matched nothing keeps the cleared state, as CRuby does
        clear_match(vm);
    } else if subject_reads_as(vm, str, last_md) {
        vm.svar_set(last_md);
    } else {
        // the receiver changed under the walk: search it again from where the last match was
        exec_match(vm, re, str, last, literal, literal)?;
    }
    Ok(result)
}

/// `scan` (`re_scan_ary`).
fn scan_ary(vm: &mut Vm, re: Value, str: Value, literal: bool) -> VmResult<Value> {
    subject_binary(vm, str, false)?;
    let pat = pattern_of(vm, re)?;
    let s = bytes(vm, str);
    let ncap = pat.num_captures as usize;
    let mut items: Vec<Value> = Vec::new();
    let mut pos = 0usize;
    let mut last: Option<Vec<i32>> = None;
    while pos <= s.len() {
        let caps = regexp::exec(&pat, &s, pos, true);
        let Some(caps) = caps else { break };
        last = Some(caps.clone());
        if ncap <= 1 {
            let v = byte_substr(vm, str, caps[0], caps[1] - caps[0]);
            items.push(v);
        } else {
            // CRuby returns an array per match whenever the pattern has any group
            let mut sub: Vec<Value> = Vec::with_capacity(ncap - 1);
            for i in 1..ncap {
                if caps[i * 2] >= 0 {
                    let v = byte_substr(vm, str, caps[i * 2], caps[i * 2 + 1] - caps[i * 2]);
                    sub.push(v);
                } else {
                    sub.push(Value::Nil);
                }
            }
            let a = vm.ary_new(sub);
            items.push(a);
        }
        // an empty match steps one byte forward
        pos = if caps[1] == caps[0] { caps[1] as usize + 1 } else { caps[1] as usize };
    }
    match last {
        Some(caps) => { create_matchdata(vm, if literal { Value::Nil } else { re }, str, caps); }
        None => clear_match(vm),
    }
    Ok(vm.ary_new(items))
}

/// The argument `match`, `sub`, `gsub`, `scan` and `split` take: a Regexp or a String passes
/// through, everything else raises (`check_pattern`).
fn check_pattern(vm: &mut Vm, re: Value) -> VmResult<Value> {
    if is_regexp(vm, re) || vm.str_bytes(re).is_some() { return Ok(re); }
    let name = match re {
        Value::Nil => String::from("nil"),
        Value::True => String::from("true"),
        Value::False => String::from("false"),
        v => { let c = vm.class_of(v); vm.class_name(c) }
    };
    Err(vm.raise_type(&format!("wrong argument type {name} (expected Regexp)")))
}

/// Whether an argument is a Regexp, which is what decides which implementation answers: the
/// object's own type, not `is_a?`, and an uninitialized one is one too (`regexp_arg_p`).
fn is_regexp(vm: &Vm, v: Value) -> bool {
    vm.obj_is_kind_of(v, vm.core.regexp)
}

/// The Regexp a match against a literal String pattern reports itself as, with the one entry
/// CRuby's `rb_reg_regcomp` keeps: the same literal asked for twice running answers the same
/// object, and one asked for in between drops it. The pair hangs off the `Regexp` class, under
/// names `instance_variables` does not report (`re_quoted_regexp`).
fn quoted_regexp(vm: &mut Vm, lit: Value) -> VmResult<Value> {
    let klass = Value::Obj(vm.core.regexp);
    let (kn, rn) = (vm.intern("__quoted_literal"), vm.intern("__quoted_regexp"));
    let key = vm.heap.ivar_get(vm.core.regexp, kn);
    let hit = vm.heap.ivar_get(vm.core.regexp, rn);
    if vm.str_bytes(key).is_some() && !hit.is_nil() && bytes(vm, key) == bytes(vm, lit) {
        return Ok(hit);
    }
    // everything that can raise happens before either half of the pair is stored
    let b = bytes(vm, lit);
    let frozen = vm.str_new_like(&b, lit);
    if let Some(o) = frozen.obj() { vm.heap.get_mut(o).frozen = true; }
    let re = quote_to_regexp(vm, lit)?;
    let _ = klass;
    vm.heap.ivar_set(vm.core.regexp, rn, re);
    vm.heap.ivar_set(vm.core.regexp, kn, frozen);
    Ok(re)
}

/// A String pattern quoted into a Regexp, the way CRuby's `get_pat_quoted` does
/// (`quote_to_regexp`).
fn quote_to_regexp(vm: &mut Vm, lit: Value) -> VmResult<Value> {
    let src = escape_str(&bytes(vm, lit));
    let src = vm.str_new_like(&src, lit);
    let re = vm.heap.alloc(vm.core.regexp, ObjKind::Object);
    let re = Value::Obj(re);
    re_initialize(vm, re, src, 0)?;
    Ok(re)
}

// ------------------------------------------------------------------ the classes

/// A fresh Regexp holding `src` under `flags` (`mrb_obj_new` on the class).
fn new_regexp(vm: &mut Vm, src: Value, flags: u32) -> VmResult<Value> {
    let o = vm.heap.alloc(vm.core.regexp, ObjKind::Object);
    let re = Value::Obj(o);
    re_initialize(vm, re, src, flags)?;
    Ok(re)
}

pub fn init(vm: &mut Vm) {
    let object = vm.core.object;
    let regexp = vm.define_class("Regexp", object);
    let match_data = vm.define_class("MatchData", object);
    vm.core.regexp = regexp;
    vm.core.match_data = match_data;
    vm.s.backref = Some(vm.intern("$~"));

    // `Regexp::IGNORECASE` and friends, and the limits the engine carries
    for (name, v) in [("IGNORECASE", 1i64), ("EXTENDED", 2), ("MULTILINE", 4),
                      ("STEP_LIMIT", regexp::STEP_LIMIT as i64),
                      ("STACK_LIMIT", regexp::STACK_LIMIT as i64),
                      ("PARSE_DEPTH_LIMIT", regexp::PARSE_DEPTH_LIMIT as i64)] {
        let n = vm.intern(name);
        vm.heap.class_mut(regexp).consts.insert(n, crate::value::Slot::from(Value::Int(v)));
    }

    vm.define_methods(regexp, &[
        ("initialize", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let mut pattern = a[0];
            let flags;
            // a Regexp argument hands over its source and its flags
            if is_regexp(vm, pattern) {
                flags = iflags(vm, pattern);
                let src = check_initialized(vm, pattern)?;
                pattern = vm.str_new(&src);
            } else {
                if vm.str_bytes(pattern).is_none() {
                    return Err(vm.raise_type("wrong argument type (expected String or Regexp)"));
                }
                flags = parse_flags(vm, a.get(1).copied().unwrap_or(Value::Nil));
            }
            re_initialize(vm, s, pattern, flags)
        }),
        ("initialize_copy", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let orig = a[0];
            if s == orig { return Ok(s); }
            if vm.class_of(s) != vm.class_of(orig) {
                return Err(vm.raise_type("initialize_copy should take same class object"));
            }
            let src = check_initialized(vm, orig)?;
            let src = vm.str_new(&src);
            let f = iflags(vm, orig);
            re_initialize(vm, s, src, f)
        }),
        ("match", |vm, s, a, b| {
            argc!(vm, a, 1, 2);
            let str = a[0];
            if str.is_nil() { clear_match(vm); return Ok(Value::Nil); }
            let str = match_operand(vm, str)?;
            let pos = if a.len() == 2 { vm.expect_int(a[1], "pos")? } else { 0 };
            let Some(pos) = char_to_byte(vm, str, pos) else { clear_match(vm); return Ok(Value::Nil) };
            let md = exec_match(vm, s, str, pos, false, false)?;
            // called by a SEND the block runs in a frame of its own
            if !md.is_nil() && !b.is_nil() { return vm.exec_block(b, &[md]); }
            Ok(md)
        }),
        ("match?", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let pos = if a.len() == 2 { vm.expect_int(a[1], "pos")? } else { 0 };
            exec_match_p(vm, s, a[0], pos)
        }),
        ("=~", |vm, s, a, _b| {
            argc!(vm, a, 1);
            if a[0].is_nil() { clear_match(vm); return Ok(Value::Nil); }
            let str = match_operand(vm, a[0])?;
            let md = exec_match(vm, s, str, 0, false, false)?;
            if md.is_nil() { return Ok(Value::Nil); }
            let (src, _, caps) = md_check(vm, md)?;
            Ok(Value::Int(byte_to_char(vm, src, caps[0]) as i64))
        }),
        ("===", |vm, s, a, _b| {
            argc!(vm, a, 1);
            if vm.str_bytes(a[0]).is_none() && !matches!(a[0], Value::Sym(_)) { return Ok(Value::False); }
            let str = match_operand(vm, a[0])?;
            let md = exec_match(vm, s, str, 0, false, false)?;
            Ok(Value::bool(!md.is_nil()))
        }),
        ("source", |vm, s, _a, _b| { let b = check_initialized(vm, s)?; Ok(vm.str_new(&b)) }),
        ("options", |vm, s, _a, _b| {
            check_initialized(vm, s)?;
            let f = iflags(vm, s);
            let mut opts = 0i64;
            if f & regexp::FLAG_IGNORECASE != 0 { opts |= 1; }
            if f & regexp::FLAG_EXTENDED != 0 { opts |= 2; }
            if f & regexp::FLAG_MULTILINE != 0 { opts |= 4; }
            Ok(Value::Int(opts))
        }),
        ("casefold?", |vm, s, _a, _b| {
            check_initialized(vm, s)?;
            Ok(Value::bool(iflags(vm, s) & regexp::FLAG_IGNORECASE != 0))
        }),
        ("to_s", |vm, s, _a, _b| {
            // `(?on-off:source)`, with the flags that are off named after a `-` so the result
            // stays meaningful once it is interpolated into another pattern
            let src = check_initialized(vm, s)?;
            let flags = iflags(vm, s);
            let binary = { let iv = s.obj().map(|o| vm.heap.ivar_get(o, vm.s.source)).unwrap_or(Value::Nil); vm.str_binary(iv) };
            let (ptr, len, flags) = fold_leading_group(&src, flags, binary);
            let mut out = b"(?".to_vec();
            let mut off = Vec::new();
            for (bit, letter) in FLAG_LETTERS {
                if flags & bit != 0 { out.push(letter); } else { off.push(letter); }
            }
            if !off.is_empty() { out.push(b'-'); out.extend_from_slice(&off); }
            out.push(b':');
            out.extend_from_slice(&src[ptr..ptr + len]);
            out.push(b')');
            Ok(vm.str_new(&out))
        }),
        ("inspect", |vm, s, _a, _b| {
            // the one reader that answers for an uninitialized Regexp instead of raising
            let src = s.obj().map(|o| vm.heap.ivar_get(o, vm.s.source)).unwrap_or(Value::Nil);
            let has_pat = s.obj().map(|o| matches!(vm.heap.get(o).kind, ObjKind::Regexp(_))).unwrap_or(false);
            if !has_pat || vm.str_bytes(src).is_none() { let d = super::object::any_to_s(vm, s); return Ok(vm.str_new(d.as_bytes())); }
            let mut out = b"/".to_vec();
            out.extend_from_slice(&bytes(vm, src));
            out.push(b'/');
            let mut letters = String::new();
            flags_cat(&mut letters, iflags(vm, s));
            out.extend_from_slice(letters.as_bytes());
            Ok(vm.str_new(&out))
        }),
        ("==", regexp_eql),
        ("eql?", regexp_eql),
        ("hash", |vm, s, _a, _b| {
            let src = check_initialized(vm, s)?;
            let sv = vm.str_new(&src);
            let h = vm.value_hash(sv) as u32 ^ iflags(vm, s).wrapping_mul(0x9e37_79b9);
            Ok(Value::Int(h as i64))
        }),
    ]);
    // `mrb_define_method` puts both through `mrb_define_method_raw`, which makes every
    // `initialize` and `initialize_copy` private (regexp.c)
    vm.mark_private(regexp, &["initialize", "initialize_copy"]);
    // `mrb_define_private_method(mrb, re, "__check_initialized", ...)` (regexp.c)
    vm.define_private_methods(regexp, &[
        ("__check_initialized", |vm, s, _a, _b| { check_initialized(vm, s)?; Ok(s) }),
    ]);
    let rsc = vm.singleton_class(Value::Obj(regexp)).expect("Regexp singleton");
    vm.define_methods(rsc, &[
        ("escape", |vm, _s, a, _b| { argc!(vm, a, 1); let b = vm.expect_str(a[0], "string")?; let e = escape_str(&b); Ok(vm.str_new(&e)) }),
        ("quote", |vm, _s, a, _b| { argc!(vm, a, 1); let b = vm.expect_str(a[0], "string")?; let e = escape_str(&b); Ok(vm.str_new(&e)) }),
        ("last_match", |vm, _s, a, _b| {
            argc!(vm, a, 0, 1);
            let md = vm.svar_get();
            match a.first() {
                None => Ok(md),
                Some(n) => {
                    if md.is_nil() { return Ok(Value::Nil); }
                    // the argument goes to the single-index read, so a Range raises TypeError
                    // here where `$~[0..1]` answers an array
                    let n = *n;
                    md_index(vm, md, n)
                }
            }
        }),
        ("union", |vm, _s, a, _b| {
            // a single Array argument stands for its elements, and a lone Regexp is answered as
            // itself rather than recompiled
            let mut args: Vec<Value> = a.to_vec();
            if args.len() == 1 {
                if let Some(ary) = vm.ary_vals(args[0]) { args = ary; }
            }
            if args.is_empty() {
                let src = vm.str_new(b"(?!)");
                return new_regexp(vm, src, 0);
            }
            if args.len() == 1 && is_regexp(vm, args[0]) { return Ok(args[0]); }
            let mut src: Vec<u8> = Vec::new();
            for (i, e) in args.iter().enumerate() {
                if i > 0 { src.push(b'|'); }
                if is_regexp(vm, *e) {
                    let to_s = vm.intern("to_s");
                    let t = vm.funcall(*e, to_s, &[], Value::Nil)?;
                    src.extend_from_slice(&bytes(vm, t));
                } else {
                    let b = vm.expect_str(*e, "pattern")?;
                    src.extend_from_slice(&escape_str(&b));
                }
            }
            let src = vm.str_new(&src);
            new_regexp(vm, src, 0)
        }),
    ]);

    vm.define_methods(match_data, &[
        ("[]", |vm, s, a, _b| { argc!(vm, a, 1, 2); md_aref(vm, s, a[0], a.get(1).copied()) }),
        ("captures", |vm, s, _a, _b| md_to_ary(vm, s, 1)),
        ("to_a", |vm, s, _a, _b| md_to_ary(vm, s, 0)),
        ("length", |vm, s, _a, _b| { let (_, _, c) = md_check(vm, s)?; Ok(Value::Int((c.len() / 2) as i64)) }),
        ("size", |vm, s, _a, _b| { let (_, _, c) = md_check(vm, s)?; Ok(Value::Int((c.len() / 2) as i64)) }),
        ("begin", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let idx = md_group_arg(vm, s, a[0])?;
            let (src, _, caps) = md_check(vm, s)?;
            let pos = caps[idx * 2];
            if pos < 0 { return Ok(Value::Nil); }
            Ok(Value::Int(byte_to_char(vm, src, pos) as i64))
        }),
        ("end", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let idx = md_group_arg(vm, s, a[0])?;
            let (src, _, caps) = md_check(vm, s)?;
            let pos = caps[idx * 2 + 1];
            if pos < 0 { return Ok(Value::Nil); }
            Ok(Value::Int(byte_to_char(vm, src, pos) as i64))
        }),
        ("offset", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let idx = md_group_arg(vm, s, a[0])?;
            let (src, _, caps) = md_check(vm, s)?;
            let beg = caps[idx * 2];
            if beg < 0 { return Ok(vm.ary_new(vec![Value::Nil, Value::Nil])); }
            let end = caps[idx * 2 + 1];
            let (b, e) = (byte_to_char(vm, src, beg) as i64, byte_to_char(vm, src, end) as i64);
            Ok(vm.ary_new(vec![Value::Int(b), Value::Int(e)]))
        }),
        ("pre_match", md_pre),
        ("__pre_match", md_pre),
        ("post_match", md_post),
        ("__post_match", md_post),
        // the compiler derives `$1` and `$&` from `$~` with these, so a program redefining `[]`
        // moves `$~[n]` and leaves the names alone
        ("__group", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let n = vm.expect_int(a[0], "index")?;
            let (_, _, caps) = md_check(vm, s)?;
            if n < 0 || n as usize >= caps.len() / 2 { return Ok(Value::Nil); }
            Ok(md_nth(vm, s, n))
        }),
        ("__last_group", |vm, s, _a, _b| {
            let (_, _, caps) = md_check(vm, s)?;
            for g in (1..caps.len() / 2).rev() {
                if caps[g * 2] >= 0 { return Ok(md_nth(vm, s, g as i64)); }
            }
            Ok(Value::Nil)
        }),
        ("values_at", |vm, s, a, _b| {
            let (_, _, caps) = md_check(vm, s)?;
            let n = (caps.len() / 2) as i64;
            let mut out: Vec<Value> = Vec::with_capacity(a.len());
            for v in a {
                if matches!(v, Value::Sym(_)) || vm.str_bytes(*v).is_some() {
                    let g = md_name_to_group(vm, s, *v)?;
                    out.push(md_nth(vm, s, g as i64));
                } else if matches!(v.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Range { .. })) {
                    match crate::builtins::ext_array::range_beg_len(vm, *v, n, false)? {
                        Some((beg, len)) => { for j in 0..len { out.push(md_nth(vm, s, beg + j)); } }
                        None => {
                            let d = vm.inspect_str(*v)?;
                            return Err(vm.raise(vm.core.range_error, &format!("{d} out of range")));
                        }
                    }
                } else {
                    let mut idx = vm.expect_int(*v, "index")?;
                    if idx < 0 {
                        idx += n;
                        if idx <= 0 { out.push(Value::Nil); continue; }
                    }
                    out.push(md_nth(vm, s, idx));
                }
            }
            Ok(vm.ary_new(out))
        }),
        ("named_captures", |vm, s, _a, _b| {
            let (_, re, caps) = md_check(vm, s)?;
            let h = vm.hash_new();
            let Some(pat) = md_pattern(vm, re) else { return Ok(h) };
            let names: Vec<(Vec<u8>, u16)> = pat.named_captures.iter().map(|n| (n.name.clone(), n.group)).collect();
            for (name, _) in &names {
                let g = name_to_group(&caps, Some(&pat), name);
                let k = vm.str_new(name);
                let v = match g { Some(g) => md_nth(vm, s, g as i64), None => Value::Nil };
                vm.hash_set(h, k, v)?;
            }
            Ok(h)
        }),
        ("string", |vm, s, _a, _b| { let (src, _, _) = md_check(vm, s)?; Ok(src) }),
        ("regexp", |vm, s, _a, _b| {
            let (src, re, caps) = md_check(vm, s)?;
            if !re.is_nil() { return Ok(re); }
            // a literal String pattern is the only thing that arrives without a Regexp
            let lit = byte_substr(vm, src, caps[0], caps[1] - caps[0]);
            let q = quoted_regexp(vm, lit)?;
            if let Some(o) = s.obj() {
                if let ObjKind::MatchData { regexp, .. } = &mut vm.heap.get_mut(o).kind {
                    *regexp = crate::value::Slot::from(q);
                }
            }
            Ok(q)
        }),
        ("to_s", |vm, s, _a, _b| {
            let (src, _, caps) = md_check(vm, s)?;
            if caps[0] < 0 { return Ok(Value::Nil); }
            Ok(byte_substr(vm, src, caps[0], caps[1] - caps[0]))
        }),
        ("inspect", |vm, s, _a, _b| {
            // the one reader that answers rather than raising, as on an uninitialized Regexp
            let Some((src, re, caps)) = md_parts(vm, s) else {
                let d = super::object::any_to_s(vm, s);
                return Ok(vm.str_new(d.as_bytes()));
            };
            if re.is_nil() {
                // a match made with a quoted String pattern carries no Regexp until
                // `MatchData#regexp` builds one, and CRuby renders such a match whole
                let mut out = b"#<MatchData: ".to_vec();
                let piece = byte_substr(vm, src, caps[0], caps[1] - caps[0]);
                out.extend_from_slice(&bytes(vm, piece));
                out.push(b'>');
                return Ok(vm.str_new(&out));
            }
            let pat = md_pattern(vm, re);
            let named: Vec<(Vec<u8>, u16)> = pat.map(|p| p.named_captures.iter().map(|n| (n.name.clone(), n.group)).collect()).unwrap_or_default();
            let mut out = b"#<MatchData".to_vec();
            for i in 0..caps.len() / 2 {
                out.push(b' ');
                if i > 0 {
                    match named.iter().find(|(_, g)| *g as usize == i) {
                        Some((name, _)) => out.extend_from_slice(name),
                        None => out.extend_from_slice(format!("{i}").as_bytes()),
                    }
                    out.push(b':');
                }
                if caps[i * 2] < 0 {
                    out.extend_from_slice(b"nil");
                } else {
                    let g = byte_substr(vm, src, caps[i * 2], caps[i * 2 + 1] - caps[i * 2]);
                    let d = vm.inspect_str(g)?;
                    out.extend_from_slice(d.as_bytes());
                }
            }
            out.push(b'>');
            Ok(vm.str_new(&out))
        }),
    ]);
    // The String methods whose regexp form this gem answers. Every core or string-ext method a
    // non-Regexp argument goes back to is captured under a private name first, before the
    // override takes the name (`mrb_alias_method` in the gem's init).
    let string = vm.core.string;
    for (new, old) in [("__split", "split"), ("__slice_bang", "slice!"), ("__index", "index"),
                       ("__rindex", "rindex"), ("__byteindex", "byteindex"),
                       ("__byterindex", "byterindex"), ("__partition", "partition"),
                       ("__rpartition", "rpartition"), ("__start_with?", "start_with?"),
                       ("__aref", "[]"), ("__aset", "[]=")] {
        let (n, o) = (vm.intern(new), vm.intern(old));
        let _ = vm.alias_method(string, n, o);
    }
    init_string(vm);

    // a match is the only thing that builds one (`create_matchdata`), so neither `new` nor
    // `allocate` answers, as CRuby undefines both
    let msc = vm.singleton_class(Value::Obj(match_data)).expect("MatchData singleton");
    for name in ["new", "allocate"] {
        let n = vm.intern(name);
        vm.heap.class_mut(msc).methods.insert(n, crate::object::Method::Undef);
    }
}

fn regexp_eql(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1);
    // the one object equal to an uninitialized Regexp is itself
    if s == a[0] { return Ok(Value::True); }
    if !is_regexp(vm, a[0]) { return Ok(Value::False); }
    let (s1, s2) = (check_initialized(vm, s)?, check_initialized(vm, a[0])?);
    if s1 != s2 { return Ok(Value::False); }
    Ok(Value::bool(iflags(vm, s) == iflags(vm, a[0])))
}

fn md_pre(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let (src, _, caps) = md_check(vm, s)?;
    if caps[0] < 0 { return Ok(Value::Nil); }
    Ok(byte_substr(vm, src, 0, caps[0]))
}

fn md_post(vm: &mut Vm, s: Value, _a: &[Value], _b: Value) -> VmResult<Value> {
    let (src, _, caps) = md_check(vm, s)?;
    if caps[1] < 0 { return Ok(Value::Nil); }
    let len = bytes(vm, src).len() as i32;
    Ok(byte_substr(vm, src, caps[1], len - caps[1]))
}

/// The groups from `from` on, as strings (`matchdata_to_ary`).
fn md_to_ary(vm: &mut Vm, s: Value, from: usize) -> VmResult<Value> {
    let (_, _, caps) = md_check(vm, s)?;
    let mut out: Vec<Value> = Vec::new();
    for i in from..caps.len() / 2 { out.push(md_nth(vm, s, i as i64)); }
    Ok(vm.ary_new(out))
}

/// The group one index or name reads, which is what `MatchData#[]` keeps behind its slice forms
/// (`md_aref`).
fn md_index(vm: &mut Vm, s: Value, arg: Value) -> VmResult<Value> {
    let (_, _, caps) = md_check(vm, s)?;
    let n = (caps.len() / 2) as i64;
    let idx = if matches!(arg, Value::Sym(_)) || vm.str_bytes(arg).is_some() {
        md_name_to_group(vm, s, arg)? as i64
    } else {
        let mut idx = vm.expect_int(arg, "index")?;
        if idx < 0 {
            // a negative index counts back from the last group, and the lowest it reaches is
            // group 1, never the whole match
            idx += n;
            if idx <= 0 { return Ok(Value::Nil); }
        }
        idx
    };
    Ok(md_nth(vm, s, idx))
}

/// `MatchData#[]` (`matchdata_aref`), which `String#[]` with a Regexp reaches too.
fn md_aref(vm: &mut Vm, s: Value, arg: Value, len_v: Option<Value>) -> VmResult<Value> {
    let is_range = matches!(arg.obj().map(|o| &vm.heap.get(o).kind), Some(ObjKind::Range { .. }));
    if (len_v.is_none() || len_v == Some(Value::Nil)) && !is_range {
        return md_index(vm, s, arg);
    }
    let (_, _, caps) = md_check(vm, s)?;
    let n = (caps.len() / 2) as i64;
    let (beg, len) = match len_v {
        Some(l) if !l.is_nil() => {
            let mut beg = vm.expect_int(arg, "index")?;
            let mut len = vm.expect_int(l, "length")?;
            if len < 0 { return Ok(Value::Nil); }
            if beg < 0 { beg += n; if beg < 0 { return Ok(Value::Nil); } }
            else if beg > n { return Ok(Value::Nil); }
            if len > n - beg { len = n - beg; }
            (beg, len)
        }
        _ => match crate::builtins::ext_array::range_beg_len(vm, arg, n, true)? {
            Some(x) => x,
            None => return Ok(Value::Nil),
        },
    };
    let mut out: Vec<Value> = Vec::with_capacity(len as usize);
    for i in 0..len { out.push(md_nth(vm, s, beg + i)); }
    Ok(vm.ary_new(out))
}

// ------------------------------------------------------------------ the String methods

/// `String#replace` without the dispatch: the write path of `sub!`, `gsub!` and `slice!`
/// (`str_assign`).
fn str_assign(vm: &mut Vm, str: Value, newstr: Value) -> VmResult<()> {
    if let Some(o) = str.obj() {
        if vm.heap.get(o).frozen { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen String")); }
        let b = bytes(vm, newstr);
        let binary = vm.str_binary(newstr);
        if let ObjKind::String(s) = &mut vm.heap.get_mut(o).kind { *s = b; }
        vm.str_set_binary(str, binary);
    }
    Ok(())
}

fn check_frozen(vm: &mut Vm, v: Value) -> VmResult<()> {
    if let Some(o) = v.obj() {
        if vm.heap.get(o).frozen { return Err(vm.raise(vm.core.frozen_error, "can't modify frozen String")); }
    }
    Ok(())
}

/// The pattern a String method searches with: a String argument is quoted (`get_pat_quoted`).
fn pattern_arg(vm: &mut Vm, arg: Value) -> VmResult<(Value, bool)> {
    let p = check_pattern(vm, arg)?;
    let literal = vm.str_bytes(p).is_some();
    Ok((p, literal))
}

/// `sub`/`gsub` with the arity CRuby gives them: one or two arguments with a block, exactly two
/// without one (`sub_argnum_check`).
fn sub_argnum_check(vm: &mut Vm, argc: usize, block: Value) -> VmResult<()> {
    if !block.is_nil() {
        if argc != 1 && argc != 2 { return Err(vm.argnum_error(argc, "1..2")); }
    } else if argc != 2 {
        return Err(vm.argnum_error(argc, "2"));
    }
    Ok(())
}

/// The literal-pattern form of `sub` (`re_sub_lit`).
fn sub_lit(vm: &mut Vm, lit: Value, str: Value, replacement: Value, bang: bool) -> VmResult<Value> {
    let s = bytes(vm, str);
    let p = bytes(vm, lit);
    let rep = bytes(vm, replacement);
    let Some(beg) = lit_search(&s, &p, 0) else {
        clear_match(vm);
        return Ok(if bang { Value::Nil } else { vm.str_new_like(&s, str) });
    };
    let end = beg + p.len();
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    out.extend_from_slice(&s[..beg]);
    if rep.contains(&b'\\') {
        let caps = vec![beg as i32, end as i32];
        apply_replacement(vm, &mut out, &rep, &s, &caps, None)?;
    } else {
        out.extend_from_slice(&rep);
    }
    out.extend_from_slice(&s[end..]);
    let result = vm.str_new(&out);
    lit_matchdata(vm, str, beg, end);
    mark_spliced(vm, result, str, replacement, true);
    vm.str_set_binary(result, vm.str_binary(str) || vm.str_binary(result));
    Ok(result)
}

/// The literal-pattern form of `gsub` (`re_gsub_lit`).
fn gsub_lit(vm: &mut Vm, lit: Value, str: Value, replacement: Value, bang: bool) -> VmResult<Value> {
    let s = bytes(vm, str);
    let p = bytes(vm, lit);
    let rep = bytes(vm, replacement);
    let need_expand = rep.contains(&b'\\');
    let binary = vm.str_binary(str);
    let Some(mut beg) = lit_search(&s, &p, 0) else {
        clear_match(vm);
        return Ok(if bang { Value::Nil } else { vm.str_new_like(&s, str) });
    };
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut pos = 0usize;
    let mut caps = vec![0i32, 0];
    loop {
        caps[0] = beg as i32;
        caps[1] = (beg + p.len()) as i32;
        if beg > pos { out.extend_from_slice(&s[pos..beg]); }
        if need_expand {
            apply_replacement(vm, &mut out, &rep, &s, &caps, None)?;
        } else {
            out.extend_from_slice(&rep);
        }
        // an empty pattern matches before every character, so the step past it carries the
        // character it stood before
        if p.is_empty() {
            if beg < s.len() {
                let clen = regexp::charlen(&s, beg, binary);
                out.extend_from_slice(&s[beg..beg + clen]);
                pos = beg + clen;
            } else {
                pos = beg + 1;
            }
        } else {
            pos = beg + p.len();
        }
        match lit_search(&s, &p, pos) { Some(b) => beg = b, None => break }
    }
    if pos < s.len() { out.extend_from_slice(&s[pos..]); }
    let result = vm.str_new(&out);
    lit_matchdata(vm, str, caps[0] as usize, caps[1] as usize);
    mark_spliced(vm, result, str, replacement, true);
    vm.str_set_binary(result, vm.str_binary(str) || vm.str_binary(result));
    Ok(result)
}

/// `sub` with a compiled pattern and a String replacement (`re_sub_str`).
fn sub_str(vm: &mut Vm, re: Value, str: Value, replacement: Value) -> VmResult<Value> {
    subject_binary(vm, str, false)?;
    let pat = pattern_of(vm, re)?;
    let s = bytes(vm, str);
    let rep = bytes(vm, replacement);
    let caps = regexp::exec(&pat, &s, 0, true);
    let Some(caps) = caps else {
        clear_match(vm);
        return Ok(vm.str_new_like(&s, str));
    };
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    out.extend_from_slice(&s[..caps[0] as usize]);
    if rep.contains(&b'\\') {
        apply_replacement(vm, &mut out, &rep, &s, &caps, Some(&pat))?;
    } else {
        out.extend_from_slice(&rep);
    }
    out.extend_from_slice(&s[caps[1] as usize..]);
    let result = vm.str_new(&out);
    create_matchdata(vm, re, str, caps);
    mark_spliced(vm, result, str, replacement, true);
    vm.str_set_binary(result, vm.str_binary(str) || vm.str_binary(result));
    Ok(result)
}

/// The String and Symbol methods the gem takes the names of; each reaches the method it replaced
/// under the private name captured in [`init`] where the argument is not a Regexp.
fn init_string(vm: &mut Vm) {
    let string = vm.core.string;
    vm.define_methods(string, &[
        ("match", str_match),
        ("match?", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let (re, literal) = pattern_arg(vm, a[0])?;
            let re = if literal { new_regexp(vm, re, 0)? } else { re };
            let pos = if a.len() == 2 { vm.expect_int(a[1], "pos")? } else { 0 };
            exec_match_p(vm, re, s, pos)
        }),
        ("=~", |vm, s, a, _b| {
            argc!(vm, a, 1);
            // a String on the right is refused: two strings have no match between them
            if vm.str_bytes(a[0]).is_some() { return Err(vm.raise_type("type mismatch: String given")); }
            if is_regexp(vm, a[0]) {
                let md = re_search(vm, a[0], s, 0, false)?;
                if md.is_nil() { return Ok(Value::Nil); }
                let (src, _, caps) = md_check(vm, md)?;
                return Ok(Value::Int(byte_to_char(vm, src, caps[0]) as i64));
            }
            let m = vm.intern("=~");
            vm.funcall(a[0], m, &[s], Value::Nil)
        }),
        ("sub", |vm, s, a, b| str_sub(vm, s, a, b, false)),
        ("sub!", |vm, s, a, b| str_sub(vm, s, a, b, true)),
        ("gsub", |vm, s, a, b| str_gsub(vm, s, a, b, false)),
        ("gsub!", |vm, s, a, b| str_gsub(vm, s, a, b, true)),
        ("scan", |vm, s, a, b| {
            argc!(vm, a, 1);
            let (pattern, literal) = pattern_arg(vm, a[0])?;
            let pattern = if literal { quote_to_regexp(vm, pattern)? } else { pattern };
            if b.is_nil() { return scan_ary(vm, pattern, s, literal); }
            // the block form walks the subject itself and lets each search publish as it goes
            let len = bytes(vm, s).len();
            let (mut pos, mut last) = (0usize, 0usize);
            let mut last_md = Value::Nil;
            while pos <= len {
                if bytes(vm, s).len() != len {
                    return Err(vm.raise(vm.core.runtime_error, "string modified"));
                }
                let md = exec_match(vm, pattern, s, pos, false, literal)?;
                if md.is_nil() { break; }
                last = pos;
                last_md = md;
                let (src, _, caps) = md_check(vm, md)?;
                let (beg, end) = (caps[0], caps[1]);
                let yv = if caps.len() == 2 { byte_substr(vm, src, beg, end - beg) } else { md_to_ary(vm, md, 1)? };
                vm.call_block(b, &[yv])?;
                pos = if beg == end { end as usize + 1 } else { end as usize };
            }
            if !last_md.is_nil() {
                if subject_reads_as(vm, s, last_md) {
                    vm.svar_set(last_md);
                } else {
                    if bytes(vm, s).len() != len {
                        return Err(vm.raise(vm.core.runtime_error, "string modified"));
                    }
                    exec_match(vm, pattern, s, last, false, literal)?;
                }
            }
            Ok(s)
        }),
        ("split", str_split),
        ("slice!", str_slice_bang),
        ("index", str_index),
        ("rindex", str_rindex),
        ("byteindex", str_byteindex),
        ("byterindex", str_byterindex),
        ("partition", |vm, s, a, _b| {
            argc!(vm, a, 1);
            if !is_regexp(vm, a[0]) {
                let m = vm.intern("__partition");
                return vm.funcall(s, m, a, Value::Nil);
            }
            let md = re_search(vm, a[0], s, 0, false)?;
            if md.is_nil() {
                let whole = bytes(vm, s);
                let (x, y, z) = (vm.str_new(&whole), vm.str_new(b""), vm.str_new(b""));
                return Ok(vm.ary_new(vec![x, y, z]));
            }
            let (src, _, caps) = md_check(vm, md)?;
            let slen = bytes(vm, src).len() as i32;
            let x = byte_substr(vm, src, 0, caps[0]);
            let y = byte_substr(vm, src, caps[0], caps[1] - caps[0]);
            let z = byte_substr(vm, src, caps[1], slen - caps[1]);
            Ok(vm.ary_new(vec![x, y, z]))
        }),
        ("rpartition", |vm, s, a, _b| {
            argc!(vm, a, 1);
            if !is_regexp(vm, a[0]) {
                let m = vm.intern("__rpartition");
                return vm.funcall(s, m, a, Value::Nil);
            }
            let len = bytes(vm, s).len();
            let md = re_byte_rsearch(vm, a[0], s, len)?;
            if md.is_nil() {
                let whole = bytes(vm, s);
                let (x, y, z) = (vm.str_new(b""), vm.str_new(b""), vm.str_new(&whole));
                return Ok(vm.ary_new(vec![x, y, z]));
            }
            let (src, _, caps) = md_check(vm, md)?;
            let slen = bytes(vm, src).len() as i32;
            let x = byte_substr(vm, src, 0, caps[0]);
            let y = byte_substr(vm, src, caps[0], caps[1] - caps[0]);
            let z = byte_substr(vm, src, caps[1], slen - caps[1]);
            Ok(vm.ary_new(vec![x, y, z]))
        }),
        ("start_with?", |vm, s, a, _b| {
            if !a.iter().any(|v| is_regexp(vm, *v)) {
                let m = vm.intern("__start_with?");
                return vm.funcall(s, m, a, Value::Nil);
            }
            for arg in a {
                if is_regexp(vm, *arg) {
                    // the engine matches leftmost, so a match starting at 0 is the anchored answer
                    let md = re_search(vm, *arg, s, 0, false)?;
                    if !md.is_nil() {
                        let (_, _, caps) = md_check(vm, md)?;
                        if caps[0] == 0 { return Ok(Value::True); }
                        clear_match(vm);
                    }
                } else {
                    let m = vm.intern("__start_with?");
                    if vm.funcall(s, m, &[*arg], Value::Nil)?.truthy() { return Ok(Value::True); }
                }
            }
            Ok(Value::False)
        }),
        ("[]", str_aref),
        ("slice", str_aref),
        ("[]=", str_aset),
    ]);
    // Taking the two names disarmed the String branch of the index opcodes, which answer
    // `str[Integer]`, `str[String]` and `str[Range]` only while `[]` is the implementation they
    // stand in for. For those three the pair above calls the same core body the opcode calls,
    // with the same arguments, so the promise `Vm::idx_op_rearm` asks for holds. A Regexp is
    // none of the three: the opcode sends it, and it arrives here.
    vm.idx_op_rearm(crate::vm::IDX_STR_AREF);
    vm.idx_op_rearm(crate::vm::IDX_STR_ASET);
    let symbol = vm.core.symbol;
    vm.define_methods(symbol, &[
        ("match", |vm, s, a, b| { let str = match_operand(vm, s)?; str_match(vm, str, a, b) }),
        ("match?", |vm, s, a, _b| {
            argc!(vm, a, 1, 2);
            let str = match_operand(vm, s)?;
            let (re, literal) = pattern_arg(vm, a[0])?;
            let re = if literal { new_regexp(vm, re, 0)? } else { re };
            let pos = if a.len() == 2 { vm.expect_int(a[1], "pos")? } else { 0 };
            exec_match_p(vm, re, str, pos)
        }),
        ("=~", |vm, s, a, _b| {
            argc!(vm, a, 1);
            let str = match_operand(vm, s)?;
            let m = vm.intern("=~");
            vm.funcall(str, m, a, Value::Nil)
        }),
    ]);
}

fn str_match(vm: &mut Vm, s: Value, a: &[Value], b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    // a String pattern is compiled here, not quoted: `match` reads it as a pattern
    let (re, literal) = pattern_arg(vm, a[0])?;
    let re = if literal { new_regexp(vm, re, 0)? } else { re };
    let m = vm.intern("match");
    let pos = a.get(1).copied().unwrap_or(Value::Int(0));
    // `Regexp#match` in the same frame, so its block is no native boundary either
    vm.send_in_frame(re, m, &[s, pos], None, b)
}

fn str_sub(vm: &mut Vm, s: Value, a: &[Value], b: Value, bang: bool) -> VmResult<Value> {
    sub_argnum_check(vm, a.len(), b)?;
    let a1 = a.get(1).copied().unwrap_or(Value::Nil);
    let (mut pattern, literal) = pattern_arg(vm, a[0])?;
    let hash = if a.len() == 2 && is_hash(vm, a1) { a1 } else { Value::Nil };
    if bang { check_frozen(vm, s)?; }
    if literal && a.len() == 2 && hash.is_nil() {
        let rep = as_string_value(vm, a1)?;
        let r = sub_lit(vm, pattern, s, rep, bang)?;
        if !bang { return Ok(r); }
        if r.is_nil() { return Ok(Value::Nil); }
        str_assign(vm, s, r)?;
        return Ok(s);
    }
    if literal { pattern = quote_to_regexp(vm, pattern)?; }
    if !bang && a.len() == 2 && hash.is_nil() {
        let rep = as_string_value(vm, a1)?;
        return sub_str(vm, pattern, s, rep);
    }
    let md = re_search(vm, pattern, s, 0, literal)?;
    if md.is_nil() {
        if bang { return Ok(Value::Nil); }
        let b = bytes(vm, s);
        return Ok(vm.str_new_like(&b, s));
    }
    if bang && a.len() == 2 && hash.is_nil() {
        let rep = as_string_value(vm, a1)?;
        let r = sub_str(vm, pattern, s, rep)?;
        str_assign(vm, s, r)?;
        return Ok(s);
    }
    // the block or Hash form: `sub` splices into the subject the match was made on, and `sub!`
    // into the receiver as the block left it, which is what CRuby's `rb_str_sub_bang` does
    let (src, _, caps) = md_check(vm, md)?;
    let (beg, end) = (caps[0], caps[1]);
    let matched = byte_substr(vm, src, beg, end - beg);
    let len = bytes(vm, s).len();
    let piece = sub_piece(vm, b, hash, matched)?;
    if bang && bytes(vm, s).len() != len {
        return Err(vm.raise(vm.core.runtime_error, "string modified"));
    }
    let from = if bang { s } else { src };
    let s_bytes = bytes(vm, from);
    let mut out: Vec<u8> = Vec::with_capacity(s_bytes.len());
    out.extend_from_slice(&s_bytes[..beg as usize]);
    out.extend_from_slice(&piece);
    out.extend_from_slice(&s_bytes[end as usize..]);
    let result = vm.str_new_like(&out, from);
    if bang { str_assign(vm, s, result)?; return Ok(s); }
    Ok(result)
}

fn str_gsub(vm: &mut Vm, s: Value, a: &[Value], b: Value, bang: bool) -> VmResult<Value> {
    // the frozen check comes before the arity check and before the enumerator, as in CRuby
    if bang { check_frozen(vm, s)?; }
    if a.len() != 1 && a.len() != 2 { return Err(vm.argnum_error(a.len(), "1..2")); }
    if a.len() == 1 && b.is_nil() {
        // before the pattern check, so that `"abc".gsub(:b)` yields an Enumerator and raises on
        // the first iteration
        let to_enum = vm.intern("to_enum");
        let name = vm.intern(if bang { "gsub!" } else { "gsub" });
        return vm.funcall(s, to_enum, &[Value::Sym(name), a[0]], Value::Nil);
    }
    let a1 = a.get(1).copied().unwrap_or(Value::Nil);
    let (mut pattern, literal) = pattern_arg(vm, a[0])?;
    let hash = if a.len() == 2 && is_hash(vm, a1) { a1 } else { Value::Nil };
    if literal && a.len() == 2 && hash.is_nil() {
        let rep = as_string_value(vm, a1)?;
        let r = gsub_lit(vm, pattern, s, rep, bang)?;
        if !bang { return Ok(r); }
        if r.is_nil() { return Ok(Value::Nil); }
        str_assign(vm, s, r)?;
        return Ok(s);
    }
    if literal { pattern = quote_to_regexp(vm, pattern)?; }
    if bang && re_search(vm, pattern, s, 0, literal)?.is_nil() { return Ok(Value::Nil); }
    let r = if a.len() == 2 && hash.is_nil() {
        let rep = as_string_value(vm, a1)?;
        gsub_str(vm, pattern, s, rep)?
    } else if !hash.is_nil() {
        gsub_walk(vm, pattern, s, literal, Value::Nil, hash)?
    } else {
        gsub_walk(vm, pattern, s, literal, b, Value::Nil)?
    };
    if bang { str_assign(vm, s, r)?; return Ok(s); }
    Ok(r)
}

/// Whether a value is a Hash, which decides whether a second argument is a replacement or a
/// lookup table.
fn is_hash(vm: &Vm, v: Value) -> bool {
    v.obj().map(|o| matches!(vm.heap.get(o).kind, ObjKind::Hash(_))).unwrap_or(false)
}

/// `mrb_obj_as_string`: the replacement as a String value.
fn as_string_value(vm: &mut Vm, v: Value) -> VmResult<Value> {
    if vm.str_bytes(v).is_some() { return Ok(v); }
    let b = vm.as_string(v)?;
    Ok(vm.str_new(&b))
}

fn str_split(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 0, 2);
    let pattern = a.first().copied().unwrap_or(Value::Nil);
    let limit_given = a.len() > 1;
    let limit = if limit_given { vm.expect_int(a[1], "limit")? } else { 0 };
    // nil or a String separator is the core method's, which the gem captured
    if pattern.is_nil() || vm.str_bytes(pattern).is_some() {
        if limit != 1 { subject_binary(vm, s, false)?; }
        let m = vm.intern("__split");
        let args: Vec<Value> = if limit_given { vec![pattern, Value::Int(limit)] } else { vec![pattern] };
        return vm.funcall(s, m, &args, Value::Nil);
    }
    if limit == 1 {
        let b = bytes(vm, s);
        if b.is_empty() { return Ok(vm.ary_new(vec![])); }
        return Ok(vm.ary_new(vec![s]));
    }
    let pattern = check_pattern(vm, pattern)?;
    // `split` is the one search where CRuby reads the pattern first, to see whether it can be
    // split on as a literal, and the order is kept here
    pattern_of(vm, pattern)?;
    let binary = subject_binary(vm, s, false)?;
    let mut result: Vec<Value> = Vec::new();
    let (mut field_start, mut search_pos, mut count) = (0i64, 0i64, 0i64);
    let sb = bytes(vm, s);
    let len = sb.len() as i64;
    while search_pos <= len {
        if limit > 0 && count >= limit - 1 {
            let tail = byte_substr(vm, s, field_start as i32, (len - field_start) as i32);
            let tail = if tail.is_nil() { vm.str_new(b"") } else { tail };
            result.push(tail);
            return Ok(vm.ary_new(result));
        }
        let md = exec_match(vm, pattern, s, search_pos as usize, false, false)?;
        if md.is_nil() { break; }
        let (src, _, caps) = md_check(vm, md)?;
        let (ms, me) = (caps[0] as i64, caps[1] as i64);
        if ms == me {
            if binary || me >= len {
                search_pos = me + 1;
            } else {
                search_pos = me + regexp::charlen(&sb, me as usize, binary) as i64;
            }
            if ms == field_start { continue; }
        }
        let piece = byte_substr(vm, s, field_start as i32, (ms - field_start) as i32);
        result.push(piece);
        count += 1;
        field_start = me;
        if ms != me { search_pos = me; }
        for i in 1..caps.len() / 2 {
            let cs = caps[i * 2];
            if cs >= 0 {
                let g = byte_substr(vm, src, cs, caps[i * 2 + 1] - cs);
                result.push(g);
            }
        }
    }
    if len > 0 && field_start <= len && (field_start < len || limit != 0) {
        let tail = byte_substr(vm, s, field_start as i32, (len - field_start) as i32);
        result.push(tail);
    }
    if limit == 0 {
        while let Some(last) = result.last() {
            if bytes(vm, *last).is_empty() { result.pop(); } else { break; }
        }
    }
    Ok(vm.ary_new(result))
}

fn str_slice_bang(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.is_empty() || !is_regexp(vm, a[0]) {
        let m = vm.intern("__slice_bang");
        return vm.funcall(s, m, a, Value::Nil);
    }
    if a.len() > 2 { return Err(vm.argnum_error(a.len(), "1..2")); }
    let group = a.get(1).copied().unwrap_or(Value::Int(0));
    // a frozen receiver raises even for a pattern that would not have matched
    check_frozen(vm, s)?;
    let md = re_search(vm, a[0], s, 0, false)?;
    if md.is_nil() { return Ok(Value::Nil); }
    let (src, _, caps) = md_check(vm, md)?;
    let n = (caps.len() / 2) as i64;
    let idx = if matches!(group, Value::Int(_)) {
        // where `[]=` raises, `slice!` answers nil: an index that reaches no group removed
        // nothing, and group 0 stays out of the negative end's reach
        let g = vm.expect_int(group, "group")?;
        if g >= n || g <= -n { return Ok(Value::Nil); }
        if g < 0 { g + n } else { g }
    } else {
        // a String or Symbol resolves to the group it names, and everything else is read as an
        // index, both the way `MatchData#begin` reads its argument
        md_group_arg(vm, md, group)? as i64
    };
    let (beg, end) = (caps[idx as usize * 2], caps[idx as usize * 2 + 1]);
    // CRuby answers "" for a group that exists but did not take part in the match
    if beg < 0 { return Ok(vm.str_new(b"")); }
    let piece = byte_substr(vm, src, beg, end - beg);
    let b = bytes(vm, s);
    let mut rest = b[..beg as usize].to_vec();
    rest.extend_from_slice(&b[end as usize..]);
    let rest = vm.str_new_like(&rest, s);
    str_assign(vm, s, rest)?;
    Ok(piece)
}

fn str_index(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.is_empty() || !is_regexp(vm, a[0]) {
        let m = vm.intern("__index");
        return vm.funcall(s, m, a, Value::Nil);
    }
    if a.len() > 2 { return Err(vm.argnum_error(a.len(), "1..2")); }
    let b = bytes(vm, s);
    let chars = crate::builtins::string::char_mode(vm, s);
    let clen = crate::builtins::string::char_len(&b, chars) as i64;
    let mut pos = if a.len() > 1 { vm.expect_int(a[1], "pos")? } else { 0 };
    if pos < 0 { pos += clen; }
    if pos < 0 || pos > clen { clear_match(vm); return Ok(Value::Nil); }
    let byte_pos = crate::builtins::string::char_to_byte(&b, pos as usize, chars);
    let md = exec_match(vm, a[0], s, byte_pos, false, false)?;
    if md.is_nil() { return Ok(Value::Nil); }
    let (src, _, caps) = md_check(vm, md)?;
    Ok(Value::Int(byte_to_char(vm, src, caps[0]) as i64))
}

fn str_rindex(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.is_empty() || !is_regexp(vm, a[0]) {
        let m = vm.intern("__rindex");
        return vm.funcall(s, m, a, Value::Nil);
    }
    if a.len() > 2 { return Err(vm.argnum_error(a.len(), "1..2")); }
    let b = bytes(vm, s);
    let chars = crate::builtins::string::char_mode(vm, s);
    let clen = crate::builtins::string::char_len(&b, chars) as i64;
    let mut pos = clen;
    if a.len() > 1 {
        pos = vm.expect_int(a[1], "pos")?;
        if pos < 0 {
            pos += clen;
            if pos < 0 { clear_match(vm); return Ok(Value::Nil); }
        } else if pos > clen {
            pos = clen;
        }
    }
    let byte_pos = if pos == clen { b.len() } else { crate::builtins::string::char_to_byte(&b, pos as usize, chars) };
    let md = re_byte_rsearch(vm, a[0], s, byte_pos)?;
    if md.is_nil() { return Ok(Value::Nil); }
    let (src, _, caps) = md_check(vm, md)?;
    Ok(Value::Int(byte_to_char(vm, src, caps[0]) as i64))
}

fn str_byteindex(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.is_empty() || !is_regexp(vm, a[0]) {
        let m = vm.intern("__byteindex");
        return vm.funcall(s, m, a, Value::Nil);
    }
    if a.len() > 2 { return Err(vm.argnum_error(a.len(), "1..2")); }
    let b = bytes(vm, s);
    let len = b.len() as i64;
    let mut pos = 0i64;
    if a.len() > 1 {
        pos = vm.expect_int(a[1], "pos")?;
        if pos < 0 { pos += len; }
    }
    if pos < 0 || pos > len { clear_match(vm); return Ok(Value::Nil); }
    crate::builtins::string::check_byte_pos(vm, &b, pos as usize, crate::builtins::string::char_mode(vm, s))?;
    let md = exec_match(vm, a[0], s, pos as usize, false, false)?;
    if md.is_nil() { return Ok(Value::Nil); }
    let (_, _, caps) = md_check(vm, md)?;
    Ok(Value::Int(caps[0] as i64))
}

fn str_byterindex(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.is_empty() || !is_regexp(vm, a[0]) {
        let m = vm.intern("__byterindex");
        return vm.funcall(s, m, a, Value::Nil);
    }
    if a.len() > 2 { return Err(vm.argnum_error(a.len(), "1..2")); }
    let b = bytes(vm, s);
    let len = b.len() as i64;
    let mut pos = len;
    if a.len() > 1 {
        pos = vm.expect_int(a[1], "pos")?;
        if pos < 0 {
            pos += len;
            if pos < 0 { clear_match(vm); return Ok(Value::Nil); }
        } else if pos > len {
            pos = len;
        }
    }
    crate::builtins::string::check_byte_pos(vm, &b, pos as usize, crate::builtins::string::char_mode(vm, s))?;
    let md = re_byte_rsearch(vm, a[0], s, pos as usize)?;
    if md.is_nil() { return Ok(Value::Nil); }
    let (_, _, caps) = md_check(vm, md)?;
    Ok(Value::Int(caps[0] as i64))
}

fn str_aref(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    if !is_regexp(vm, a[0]) {
        let m = vm.intern("__aref");
        return vm.funcall(s, m, a, Value::Nil);
    }
    let md = re_search(vm, a[0], s, 0, false)?;
    if md.is_nil() { return Ok(Value::Nil); }
    let arg = a.get(1).copied().unwrap_or(Value::Int(0));
    // the capture argument selects one group and never a slice, so a Range is a mistake here
    // where `MatchData#[]` would build an array (`re_str_aref`)
    md_index(vm, md, arg)
}

fn str_aset(vm: &mut Vm, s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    if a.is_empty() || !is_regexp(vm, a[0]) {
        let m = vm.intern("__aset");
        return vm.funcall(s, m, a, Value::Nil);
    }
    if a.len() < 2 || a.len() > 3 { return Err(vm.argnum_error(a.len(), "2..3")); }
    let pattern = a[0];
    let group = if a.len() > 2 { a[1] } else { Value::Int(0) };
    let replace = a[a.len() - 1];
    // CRuby searches before it checks the receiver for modification, which makes the order
    // observable: a frozen receiver still leaves the match behind
    let md = re_search(vm, pattern, s, 0, false)?;
    if md.is_nil() { return Err(vm.raise(vm.core.index_error, "regexp not matched")); }
    let (_, _, caps) = md_check(vm, md)?;
    let n = (caps.len() / 2) as i64;
    let idx = if matches!(group, Value::Sym(_)) || vm.str_bytes(group).is_some() {
        md_name_to_group(vm, md, group)? as i64
    } else {
        let g = vm.expect_int(group, "group")?;
        if g >= n || g <= -n {
            let d = vm.inspect_str(group)?;
            return Err(vm.raise(vm.core.index_error, &format!("index {d} out of regexp")));
        }
        if g < 0 { g + n } else { g }
    };
    let (beg, end) = (caps[idx as usize * 2], caps[idx as usize * 2 + 1]);
    if beg < 0 { return Err(vm.raise(vm.core.index_error, "regexp group not matched")); }
    let rep = vm.expect_str(replace, "replacement")?;
    check_frozen(vm, s)?;
    let b = bytes(vm, s);
    let mut out = b[..beg as usize].to_vec();
    out.extend_from_slice(&rep);
    out.extend_from_slice(&b[end as usize..]);
    let out = vm.str_new_like(&out, s);
    str_assign(vm, s, out)?;
    Ok(replace)
}
