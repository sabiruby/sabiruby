//! mruby-complex (`mrbgems/mruby-complex/src/complex.c`): `Complex`. Its Ruby part
//! (`inspect`, `to_s`, `abs`, `arg`, `polar`, `conjugate`, `Numeric#to_c`, …) is
//! `src/mrblib/complex.mrb`.
//!
//! A Complex keeps its two parts in hidden instance variables (`__real`, `__imag`), each of
//! them whatever member of the numeric tower it was given (Integer of any width, Rational or
//! Float) — the reference's `COMP_VALUE` form. It has no separate two-Float form: the parts
//! are Values here either way, so `Complex(1, 2)` keeps its Integers and
//! `Complex(1.0, 2.0)` its Floats, which is what the reference answers as well.

use alloc::{format, vec, vec::Vec};

use crate::argc;
use crate::error::{VmError, VmResult};
use crate::object::ObjKind;
use crate::value::Value;
use crate::vm::Vm;

use super::ext_rational as rat;
use super::numeric::num_f64;

/// True for a Complex (`mrb_type(v) == MRB_TT_COMPLEX`).
pub(crate) fn is_complex(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Obj(o) if vm.heap.get(o).class == vm.core.complex)
}

/// The real and the imaginary part, as they are held.
pub(crate) fn parts(vm: &Vm, v: Value) -> Option<(Value, Value)> {
    if !is_complex(vm, v) { return None; }
    let o = v.obj()?;
    Some((vm.heap.ivar_get(o, vm.s.real), vm.heap.ivar_get(o, vm.s.imag)))
}

/// `part_exact_type_p`: the members of the tower a part keeps as itself.
fn exact_part(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Int(_)) || vm.is_bigint(v) || rat::is_rational(vm, v)
}

/// `part_coerce`: a member of the tower passes through; anything else becomes a Float.
fn coerce_part(vm: &mut Vm, v: Value) -> VmResult<Value> {
    if exact_part(vm, v) || matches!(v, Value::Float(_)) { return Ok(v); }
    match part_f64(vm, v) {
        Some(f) => Ok(Value::Float(f)),
        None => { let d = vm.describe_for_type_error(v); Err(vm.raise_type(&format!("{d} cannot be converted to Float"))) }
    }
}

/// A part (or any numeric) as an `f64`.
fn part_f64(vm: &Vm, v: Value) -> Option<f64> {
    if rat::is_rational(vm, v) { return Some(rat::to_f(vm, v)); }
    num_f64(vm, v)
}

pub(crate) fn new_complex(vm: &mut Vm, re: Value, im: Value) -> VmResult<Value> {
    let o = vm.heap.alloc(vm.core.complex, ObjKind::Object);
    let (rs, is) = (vm.s.real, vm.s.imag);
    vm.heap.ivar_set(o, rs, re);
    vm.heap.ivar_set(o, is, im);
    vm.heap.get_mut(o).frozen = true;
    Ok(Value::Obj(o))
}

fn zero_div(vm: &mut Vm) -> VmError {
    vm.raise(vm.core.zero_division_error, "divided by 0")
}

/// `part_eq`: equality inside the closed set a part can be.
fn part_eq(vm: &mut Vm, a: Value, b: Value) -> VmResult<bool> {
    if rat::is_rational(vm, a) { return rat::eq(vm, a, b); }
    if rat::is_rational(vm, b) { return rat::eq(vm, b, a); }
    vm.equal(a, b)
}

fn part_zero(vm: &mut Vm, v: Value) -> VmResult<bool> {
    part_eq(vm, v, Value::Int(0))
}

/// One part's arithmetic, through the tower's own dispatch (`mrb_num_add` and friends).
fn part_op(vm: &mut Vm, a: Value, b: Value, op: char) -> VmResult<Value> {
    let mid = match op { '+' => vm.s.plus, '-' => vm.s.minus, '*' => vm.s.mul, _ => vm.s.div };
    vm.funcall(a, mid, &[b], Value::Nil)
}

/// `complex_op`: `+`, `-` and `*` work part by part; a real operand touches the real part
/// only for `+`/`-` and both parts for `*`.
pub(crate) fn arith(vm: &mut Vm, x: Value, y: Value, op: char) -> VmResult<Value> {
    if op == '/' { return div(vm, x, y); }
    let (ar, ai) = parts(vm, x).expect("receiver is a Complex");
    if !is_complex(vm, y) {
        let s = coerce_part(vm, y)?;
        return match op {
            '+' => { let r = part_op(vm, ar, s, '+')?; new_complex(vm, r, ai) }
            '-' => { let r = part_op(vm, ar, s, '-')?; new_complex(vm, r, ai) }
            _ => { let r = part_op(vm, ar, s, '*')?; let i = part_op(vm, ai, s, '*')?; new_complex(vm, r, i) }
        };
    }
    let (br, bi) = parts(vm, y).expect("Complex operand");
    match op {
        '+' => { let r = part_op(vm, ar, br, '+')?; let i = part_op(vm, ai, bi, '+')?; new_complex(vm, r, i) }
        '-' => { let r = part_op(vm, ar, br, '-')?; let i = part_op(vm, ai, bi, '-')?; new_complex(vm, r, i) }
        _ => {
            let (p, q) = (part_op(vm, ar, br, '*')?, part_op(vm, ai, bi, '*')?);
            let r = part_op(vm, p, q, '-')?;
            let (p, q) = (part_op(vm, ar, bi, '*')?, part_op(vm, ai, br, '*')?);
            let i = part_op(vm, p, q, '+')?;
            new_complex(vm, r, i)
        }
    }
}

/// `part_quo`: the exact quotient of two parts, folded back to an Integer when it comes out
/// whole (`Integer#quo` and `mrb_rational_canonicalize`).
fn part_quo(vm: &mut Vm, a: Value, b: Value) -> VmResult<Value> {
    if matches!(a, Value::Float(_)) {
        let (x, y) = (part_f64(vm, a).unwrap_or(0.0), part_f64(vm, b).unwrap_or(0.0));
        return Ok(Value::Float(x / y));
    }
    let r = rat::as_rational(vm, a)?;
    let q = rat::div(vm, r, b)?;
    Ok(rat::canonicalize(vm, q))
}

fn no_float_parts(vm: &Vm, v: Value) -> bool {
    match parts(vm, v) {
        Some((r, i)) => !matches!(r, Value::Float(_)) && !matches!(i, Value::Float(_)),
        None => false,
    }
}

/// `mrb_complex_div`: exact while no Float is anywhere in it, the standard float algorithm
/// otherwise (the reference's Smith form).
pub(crate) fn div(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> {
    if !is_complex(vm, y) {
        // an exact scalar divides each part on its own: a Float part as a float, an exact
        // part exactly (the reference asks only about the divisor here)
        if exact_part(vm, y) {
            if part_zero(vm, y)? { return Err(zero_div(vm)); }
            let (ar, ai) = parts(vm, x).expect("receiver is a Complex");
            let (r, i) = (part_quo(vm, ar, y)?, part_quo(vm, ai, y)?);
            return new_complex(vm, r, i);
        }
    } else if no_float_parts(vm, x) && no_float_parts(vm, y) {
        return div_exact(vm, x, y);
    }
    let (ar, ai) = float_parts(vm, x);
    if !is_complex(vm, y) {
        if matches!(y, Value::Int(0)) { return Err(zero_div(vm)); }
        let f = match part_f64(vm, y) { Some(f) => f, None => { let d = vm.describe_for_type_error(y); return Err(vm.raise_type(&format!("{d} cannot be converted to Float"))); } };
        if f == 0.0 { return Err(zero_div(vm)); }
        return new_complex(vm, Value::Float(ar / f), Value::Float(ai / f));
    }
    let (br, bi) = float_parts(vm, y);
    if br == 0.0 && bi == 0.0 { return Err(zero_div(vm)); }
    let (r, den, zr, zi);
    if libm::fabs(br) > libm::fabs(bi) {
        r = bi / br;
        den = br + r * bi;
        zr = (ar + ai * r) / den;
        zi = (ai - ar * r) / den;
    } else {
        r = br / bi;
        den = bi + r * br;
        zr = (ar * r + ai) / den;
        zi = (ai * r - ar) / den;
    }
    new_complex(vm, Value::Float(zr), Value::Float(zi))
}

/// `complex_div_exact`: multiply through by the conjugate, then divide each part exactly.
fn div_exact(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> {
    let (ar, ai) = parts(vm, x).expect("receiver is a Complex");
    let (br, bi) = parts(vm, y).expect("Complex operand");
    let (p, q) = (part_op(vm, br, br, '*')?, part_op(vm, bi, bi, '*')?);
    let n = part_op(vm, p, q, '+')?;
    if part_zero(vm, n)? { return Err(zero_div(vm)); }
    let (p, q) = (part_op(vm, ar, br, '*')?, part_op(vm, ai, bi, '*')?);
    let zr = part_op(vm, p, q, '+')?;
    let (p, q) = (part_op(vm, ai, br, '*')?, part_op(vm, ar, bi, '*')?);
    let zi = part_op(vm, p, q, '-')?;
    let (r, i) = (part_quo(vm, zr, n)?, part_quo(vm, zi, n)?);
    new_complex(vm, r, i)
}

/// Both parts as `f64` (`mrb_complex_get`).
pub(crate) fn float_parts(vm: &Vm, v: Value) -> (f64, f64) {
    match parts(vm, v) {
        Some((r, i)) => (part_f64(vm, r).unwrap_or(0.0), part_f64(vm, i).unwrap_or(0.0)),
        None => (0.0, 0.0),
    }
}

/// `mrb_complex_eq`.
pub(crate) fn eq(vm: &mut Vm, x: Value, y: Value) -> VmResult<bool> {
    let (ar, ai) = match parts(vm, x) { Some(p) => p, None => return Ok(false) };
    if is_complex(vm, y) {
        let (br, bi) = parts(vm, y).expect("Complex");
        return Ok(part_eq(vm, ar, br)? && part_eq(vm, ai, bi)?);
    }
    if matches!(y, Value::Int(_) | Value::Float(_)) || vm.is_bigint(y) || rat::is_rational(vm, y) {
        return Ok(part_zero(vm, ai)? && part_eq(vm, ar, y)?);
    }
    vm.equal(y, x)
}

/// `mrb_complex_to_f` / `to_i`: only a Complex whose imaginary part is zero converts.
fn to_real(vm: &mut Vm, x: Value, what: &str) -> VmResult<Value> {
    let (r, i) = parts(vm, x).expect("receiver is a Complex");
    if !part_zero(vm, i)? {
        let d = vm.inspect_str(x).unwrap_or_default();
        return Err(vm.raise(vm.core.range_error, &format!("can't convert {d} into {what}")));
    }
    Ok(r)
}

pub(crate) fn to_f64(vm: &mut Vm, x: Value) -> VmResult<f64> {
    let r = to_real(vm, x, "Float")?;
    Ok(part_f64(vm, r).unwrap_or(0.0))
}

fn complex_to_i(vm: &mut Vm, x: Value) -> VmResult<Value> {
    let r = to_real(vm, x, "Integer")?;
    if matches!(r, Value::Int(_)) || vm.is_bigint(r) { return Ok(r); }
    if rat::is_rational(vm, r) {
        let (n, d) = rat::parts(vm, r).expect("Rational");
        let (n, d) = (vm.as_bigint(n).unwrap(), vm.as_bigint(d).unwrap());
        let q = n.divmod_trunc(&d).0;
        return Ok(vm.bint_value(q));
    }
    let f = part_f64(vm, r).unwrap_or(0.0);
    if !(-9223372036854775808.0..9223372036854775808.0).contains(&f) {
        return Ok(vm.bint_value(crate::bigint::BigInt::from_f64(libm::trunc(f))));
    }
    Ok(Value::Int(libm::trunc(f) as i64))
}

/// `complex_pow`: a whole exponent is squaring and multiplying (exact parts stay exact);
/// anything else goes round through the polar form.
fn pow(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> {
    if let Value::Int(n) = y {
        if n >= 0 { return pow_int(vm, x, n as u64); }
        if n != i64::MIN {
            let p = pow_int(vm, x, n.unsigned_abs())?;
            let one = new_complex(vm, Value::Int(1), Value::Int(0))?;
            return div(vm, one, p);
        }
    }
    let (sr, si) = float_parts(vm, x);
    if is_complex(vm, y) {
        let (xr, yi) = float_parts(vm, y);
        let log_abs = libm::log(libm::hypot(sr, si));
        let arg = libm::atan2(si, sr);
        let a = xr * log_abs - yi * arg;
        let b = xr * arg + yi * log_abs;
        let e = libm::exp(a);
        return new_complex(vm, Value::Float(e * libm::cos(b)), Value::Float(e * libm::sin(b)));
    }
    let f = match part_f64(vm, y) { Some(f) => f, None => { let d = vm.describe_for_type_error(y); return Err(vm.raise_type(&format!("{d} cannot be converted to Float"))); } };
    let abs = libm::hypot(sr, si);
    let arg = libm::atan2(si, sr);
    let m = libm::pow(abs, f);
    let a = arg * f;
    new_complex(vm, Value::Float(m * libm::cos(a)), Value::Float(m * libm::sin(a)))
}

fn pow_int(vm: &mut Vm, x: Value, mut n: u64) -> VmResult<Value> {
    let mut r = new_complex(vm, Value::Int(1), Value::Int(0))?;
    let mut z = x;
    while n > 0 {
        if n & 1 == 1 { r = arith(vm, r, z, '*')?; }
        n >>= 1;
        if n > 0 { z = arith(vm, z, z, '*')?; }
    }
    Ok(r)
}

/// `part_hash32` folded into one byte string: the parts are keyed by class as well as by
/// value, the distinction `eql?` draws.
fn hash(vm: &mut Vm, x: Value) -> i64 {
    let (r, i) = match parts(vm, x) { Some(p) => p, None => return 0 };
    let mut b: Vec<u8> = Vec::new();
    for v in [r, i] {
        match v {
            Value::Int(n) => { b.push(1); b.extend_from_slice(&n.to_le_bytes()); }
            Value::Float(f) => { b.push(2); b.extend_from_slice(&(if f == 0.0 { 0.0 } else { f }).to_bits().to_le_bytes()); }
            _ if vm.is_bigint(v) => { b.push(3); b.extend_from_slice(&vm.as_bigint(v).unwrap().hash_bytes()); }
            _ => {
                b.push(4);
                if let Some((n, d)) = rat::parts(vm, v) {
                    for p in [n, d] { b.extend_from_slice(&vm.as_bigint(p).unwrap_or_else(crate::bigint::BigInt::zero).hash_bytes()); }
                }
            }
        }
    }
    let sv = vm.str_new(&b);
    vm.value_hash(sv)
}

/// `Complex.rect(real, imag = 0)`, which is `Kernel#Complex` too.
fn complex_rect(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    let re = coerce_part(vm, a[0])?;
    let im = coerce_part(vm, *a.get(1).unwrap_or(&Value::Int(0)))?;
    new_complex(vm, re, im)
}

pub fn init(vm: &mut Vm) {
    let cpx = vm.define_class("Complex", vm.core.numeric);
    vm.core.complex = cpx;
    let sc = vm.singleton_class(Value::Obj(cpx)).expect("Complex singleton");
    let new = vm.intern("new");
    let _ = vm.undef_method(sc, new);
    vm.define_methods(sc, &[("rect", complex_rect), ("rectangular", complex_rect)]);
    vm.define_methods(cpx, &[
        ("real", |vm, s, _a, _b| Ok(parts(vm, s).map(|p| p.0).unwrap_or(Value::Nil))),
        ("imaginary", |vm, s, _a, _b| Ok(parts(vm, s).map(|p| p.1).unwrap_or(Value::Nil))),
        ("to_f", |vm, s, _a, _b| { let f = to_f64(vm, s)?; Ok(Value::Float(f)) }),
        ("to_i", |vm, s, _a, _b| complex_to_i(vm, s)),
        ("to_c", |_vm, s, _a, _b| Ok(s)),
        ("+", |vm, s, a, _b| { argc!(vm, a, 1); arith(vm, s, a[0], '+') }),
        ("-", |vm, s, a, _b| { argc!(vm, a, 1); arith(vm, s, a[0], '-') }),
        ("*", |vm, s, a, _b| { argc!(vm, a, 1); arith(vm, s, a[0], '*') }),
        ("/", |vm, s, a, _b| { argc!(vm, a, 1); div(vm, s, a[0]) }),
        ("quo", |vm, s, a, _b| { argc!(vm, a, 1); div(vm, s, a[0]) }),
        ("==", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(eq(vm, s, a[0])?)) }),
        // `eql?` does not convert across part classes, because `hash` keys each part by its class
        ("eql?", |vm, s, a, _b| {
            argc!(vm, a, 1);
            if !is_complex(vm, a[0]) { return Ok(Value::False); }
            let ((ar, ai), (br, bi)) = (parts(vm, s).unwrap(), parts(vm, a[0]).unwrap());
            let same_kind = |vm: &Vm, x: Value, y: Value| matches!((x, y), (Value::Int(_), Value::Int(_)) | (Value::Float(_), Value::Float(_))) || (vm.is_bigint(x) && vm.is_bigint(y)) || (rat::is_rational(vm, x) && rat::is_rational(vm, y));
            if !same_kind(vm, ar, br) || !same_kind(vm, ai, bi) { return Ok(Value::False); }
            Ok(Value::bool(part_eq(vm, ar, br)? && part_eq(vm, ai, bi)?))
        }),
        ("hash", |vm, s, _a, _b| Ok(Value::Int(hash(vm, s)))),
        ("**", |vm, s, a, _b| { argc!(vm, a, 1); pow(vm, s, a[0]) }),
    ]);
    vm.define_method(vm.core.nil_class, "to_c", |vm, _s, _a, _b| new_complex(vm, Value::Int(0), Value::Int(0)));
    let k = vm.core.kernel;
    // `mrb_define_module_function_id(..., MRB_SYM(Complex), ...)` (complex.c)
    vm.define_module_function(k, "Complex", complex_rect).expect("Kernel singleton");
    let _ = vec![0u8];
}
