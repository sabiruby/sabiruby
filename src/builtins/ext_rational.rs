//! mruby-rational (`mrbgems/mruby-rational/src/rational.c`): `Rational`. Its Ruby part
//! (`inspect`, `to_s`, `<=>`, `Numeric#to_r`) is `src/mrblib/rational.mrb`.
//!
//! A Rational keeps its two halves in hidden instance variables (`__num`, `__den`) instead of
//! the reference's `MRB_TT_RATIONAL` payload, the way Random and Time keep theirs. Both are
//! Integers of any width, always reduced, with the sign on the numerator and a positive
//! denominator; the object is frozen. The reference reduces with `mrb_int` arithmetic and
//! only reaches for a wide integer when that overflows — here everything goes through
//! [`BigInt`] and comes back normalized, which is the same value.

use alloc::{format, vec};

use crate::argc;
use crate::bigint::BigInt;
use crate::error::{VmError, VmResult};
use crate::object::ObjKind;
use crate::value::Value;
use crate::vm::Vm;

use super::numeric::num_f64;

/// True for a Rational (`mrb_type(v) == MRB_TT_RATIONAL`).
pub(crate) fn is_rational(vm: &Vm, v: Value) -> bool {
    matches!(v, Value::Obj(o) if vm.heap.get(o).class == vm.core.rational)
}

/// The numerator and the denominator of a Rational.
pub(crate) fn parts(vm: &Vm, v: Value) -> Option<(Value, Value)> {
    if !is_rational(vm, v) { return None; }
    let o = v.obj()?;
    Some((vm.heap.ivar_get(o, vm.s.num), vm.heap.ivar_get(o, vm.s.den)))
}

fn big_parts(vm: &Vm, v: Value) -> Option<(BigInt, BigInt)> {
    let (n, d) = parts(vm, v)?;
    Some((vm.as_bigint(n)?, vm.as_bigint(d)?))
}

/// The operand as a fraction: a Rational as its halves, an Integer over 1.
fn as_frac(vm: &Vm, v: Value) -> Option<(BigInt, BigInt)> {
    if let Some(p) = big_parts(vm, v) { return Some(p); }
    Some((vm.as_bigint(v)?, BigInt::from_i64(1)))
}

fn zero_div(vm: &mut Vm) -> VmError {
    vm.raise(vm.core.zero_division_error, "divided by 0 in rational")
}

fn type_error(vm: &mut Vm, v: Value) -> VmError {
    let d = vm.describe_for_type_error(v);
    vm.raise_type(&format!("can't convert {d} into Rational"))
}

/// `mrb_rational_new`: the reduced fraction `n/d`, with the sign on the numerator.
pub(crate) fn new_rat(vm: &mut Vm, n: &BigInt, d: &BigInt) -> VmResult<Value> {
    if d.is_zero() { return Err(zero_div(vm)); }
    let (mut n, mut d) = (n.clone(), d.clone());
    if d.sign() < 0 { n = n.neg(); d = d.neg(); }
    let g = n.gcd(&d);
    if !g.is_zero() && g.cmp(&BigInt::from_i64(1)).is_gt() {
        n = n.div_floor(&g);
        d = d.div_floor(&g);
    }
    let (nv, dv) = (vm.bint_value(n), vm.bint_value(d));
    let o = vm.heap.alloc(vm.core.rational, ObjKind::Object);
    let (ns, ds) = (vm.s.num, vm.s.den);
    vm.heap.ivar_set(o, ns, nv);
    vm.heap.ivar_set(o, ds, dv);
    vm.heap.get_mut(o).frozen = true;
    Ok(Value::Obj(o))
}

fn new_i(vm: &mut Vm, n: i64, d: i64) -> VmResult<Value> {
    new_rat(vm, &BigInt::from_i64(n), &BigInt::from_i64(d))
}

/// `rational_new_f`: the fraction a Float is, exactly (its mantissa over a power of two).
pub(crate) fn new_from_f64(vm: &mut Vm, f: f64) -> VmResult<Value> {
    if f.is_nan() || f.is_infinite() {
        let s = super::numeric::float_to_s(f);
        return Err(vm.raise(vm.core.float_domain_error, &s));
    }
    if f == 0.0 { return new_i(vm, 0, 1); }
    let bits = f.abs().to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i64;
    let frac = bits & 0x000f_ffff_ffff_ffff;
    // a subnormal has no implicit leading one
    let (m, e) = if exp == 0 { (frac, -1074i64) } else { (frac | (1u64 << 52), exp - 1075) };
    let mut num = BigInt::from_u64(m);
    if f < 0.0 { num = num.neg(); }
    let (num, den) = if e >= 0 { (num.shl(e as u64), BigInt::from_i64(1)) } else { (num, BigInt::from_i64(1).shl((-e) as u64)) };
    new_rat(vm, &num, &den)
}

/// `mrb_as_rational`.
pub(crate) fn as_rational(vm: &mut Vm, v: Value) -> VmResult<Value> {
    if is_rational(vm, v) { return Ok(v); }
    if let Some(b) = vm.as_bigint(v) { return new_rat(vm, &b, &BigInt::from_i64(1)); }
    match num_f64(vm, v) {
        Some(f) => new_from_f64(vm, f),
        None if super::ext_complex::is_complex(vm, v) => {
            let f = super::ext_complex::to_f64(vm, v)?;
            new_from_f64(vm, f)
        }
        None => Err(type_error(vm, v)),
    }
}

/// `mrb_rational_canonicalize`: `(n/1)` is the Integer `n` to a caller that hands the value
/// back to the rest of the tower (an exact Complex division).
pub(crate) fn canonicalize(vm: &Vm, v: Value) -> Value {
    match parts(vm, v) {
        Some((n, d)) if d == Value::Int(1) => n,
        _ => v,
    }
}

pub(crate) fn to_f(vm: &Vm, v: Value) -> f64 {
    match big_parts(vm, v) {
        Some((n, d)) => n.to_f64() / d.to_f64(),
        None => 0.0,
    }
}

/// The Float value of a numeric operand, for the arms that answer a Float.
fn opnd_f64(vm: &mut Vm, v: Value) -> VmResult<f64> {
    if is_rational(vm, v) { return Ok(to_f(vm, v)); }
    match num_f64(vm, v) {
        Some(f) => Ok(f),
        None => { let d = vm.describe_for_type_error(v); Err(vm.raise_type(&format!("{d} cannot be converted to Float"))) }
    }
}

// ------------------------------------------------------------------ arithmetic

#[derive(Clone, Copy, PartialEq)]
enum Op { Add, Sub, Mul, Div }

fn exact(vm: &mut Vm, x: Value, y: Value, op: Op) -> VmResult<Value> {
    let (n1, d1) = as_frac(vm, x).expect("receiver is a Rational");
    let (n2, d2) = as_frac(vm, y).expect("exact operand");
    let (n, d) = match op {
        Op::Add => (n1.mul(&d2).add(&n2.mul(&d1)), d1.mul(&d2)),
        Op::Sub => (n1.mul(&d2).sub(&n2.mul(&d1)), d1.mul(&d2)),
        Op::Mul => (n1.mul(&n2), d1.mul(&d2)),
        Op::Div => {
            if n2.is_zero() { return Err(zero_div(vm)); }
            (n1.mul(&d2), d1.mul(&n2))
        }
    };
    new_rat(vm, &n, &d)
}

/// The float arms of `mrb_rational_add` and friends, with the reference's formulas (the
/// numerator meets the Float first, and the denominator divides the result).
fn with_float(vm: &mut Vm, x: Value, f: f64, op: Op) -> VmResult<Value> {
    let (n, d) = big_parts(vm, x).expect("receiver is a Rational");
    let (nf, df) = (n.to_f64(), d.to_f64());
    Ok(Value::Float(match op {
        Op::Add => (nf + f * df) / df,
        Op::Sub => (nf - f * df) / df,
        Op::Mul => (nf * f) / df,
        Op::Div => (nf / f) / df,
    }))
}

fn arith(vm: &mut Vm, x: Value, y: Value, op: Op) -> VmResult<Value> {
    if is_rational(vm, y) || matches!(y, Value::Int(_)) || vm.is_bigint(y) {
        return exact(vm, x, y, op);
    }
    if let Value::Float(f) = y { return with_float(vm, x, f, op); }
    if super::ext_complex::is_complex(vm, y) {
        let z = super::ext_complex::new_complex(vm, x, Value::Int(0))?;
        return super::ext_complex::arith(vm, z, y, match op { Op::Add => '+', Op::Sub => '-', Op::Mul => '*', Op::Div => '/' });
    }
    if op == Op::Div { return Err(type_error(vm, y)); }
    // the reference hands the pair to the other operand (`y.+(x)`)
    let mid = vm.intern(match op { Op::Add => "+", Op::Sub => "-", Op::Mul => "*", Op::Div => "/" });
    vm.funcall(y, mid, &[x], Value::Nil)
}

pub(crate) fn add(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> { arith(vm, x, y, Op::Add) }
pub(crate) fn sub(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> { arith(vm, x, y, Op::Sub) }
pub(crate) fn mul(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> { arith(vm, x, y, Op::Mul) }
pub(crate) fn div(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> { arith(vm, x, y, Op::Div) }

/// `mrb_rational_eq`.
pub(crate) fn eq(vm: &mut Vm, x: Value, y: Value) -> VmResult<bool> {
    let (n1, d1) = match big_parts(vm, x) { Some(p) => p, None => return Ok(false) };
    if let Some((n2, d2)) = as_frac(vm, y) {
        if !is_rational(vm, y) && !matches!(y, Value::Int(_)) && !vm.is_bigint(y) {
            // not a number of the exact tower after all
        } else {
            return Ok(n1.mul(&d2).cmp(&n2.mul(&d1)).is_eq());
        }
    }
    if let Value::Float(f) = y { return Ok(n1.to_f64() / d1.to_f64() == f); }
    if super::ext_complex::is_complex(vm, y) { return super::ext_complex::eq(vm, y, x); }
    vm.equal(y, x)
}

/// The exact half of `Rational#<=>` (`__cmp`): `false` for anything but an Integer or
/// another Rational, which the Ruby `<=>` then compares through Float.
fn cmp_exact(vm: &mut Vm, x: Value, y: Value) -> Option<core::cmp::Ordering> {
    let (n1, d1) = big_parts(vm, x)?;
    let (n2, d2) = if is_rational(vm, y) { big_parts(vm, y)? } else { (vm.as_bigint(y)?, BigInt::from_i64(1)) };
    Some(n1.mul(&d2).cmp(&n2.mul(&d1)))
}

fn pow(vm: &mut Vm, x: Value, y: Value) -> VmResult<Value> {
    // a Rational exponent whose denominator is 1 is a whole number
    let e = match parts(vm, y) {
        Some((n, d)) if d == Value::Int(1) => n,
        Some(_) => Value::Nil,
        None => y,
    };
    if matches!(e, Value::Int(_)) || vm.is_bigint(e) {
        let (n, d) = big_parts(vm, x).expect("receiver is a Rational");
        let exp = match e { Value::Int(i) => i, _ => return Err(vm.raise(vm.core.range_error, "exponent too large")) };
        if exp == 0 { return new_i(vm, 1, 1); }
        if exp == i64::MIN { return Err(vm.raise(vm.core.range_error, "integer overflow in rational")); }
        let (a, b) = (n.pow(exp.unsigned_abs()), d.pow(exp.unsigned_abs()));
        // a negative exponent turns the fraction over
        return if exp < 0 { new_rat(vm, &b, &a) } else { new_rat(vm, &a, &b) };
    }
    // an exponent with a fraction in it answers a Float, as in CRuby
    let d1 = to_f(vm, x);
    let d2 = opnd_f64(vm, y)?;
    Ok(Value::Float(libm::pow(d1, d2)))
}

fn hash(vm: &mut Vm, x: Value) -> i64 {
    match big_parts(vm, x) {
        Some((n, d)) => {
            let mut b = n.hash_bytes();
            b.extend_from_slice(&d.hash_bytes());
            let sv = vm.str_new(&b);
            vm.value_hash(sv)
        }
        None => 0,
    }
}

/// `Kernel#Rational(a, b = 1)` (`rational_new`).
fn kernel_rational(vm: &mut Vm, _s: Value, a: &[Value], _b: Value) -> VmResult<Value> {
    argc!(vm, a, 1, 2);
    let (x, y) = (a[0], *a.get(1).unwrap_or(&Value::Int(1)));
    let int_x = matches!(x, Value::Int(_)) || vm.is_bigint(x);
    let int_y = matches!(y, Value::Int(_)) || vm.is_bigint(y);
    if int_x && int_y {
        let (n, d) = (vm.as_bigint(x).unwrap(), vm.as_bigint(y).unwrap());
        return new_rat(vm, &n, &d);
    }
    // a Rational among exact operands makes the quotient exact; a Float anywhere does not
    if (is_rational(vm, x) || is_rational(vm, y)) && !matches!(x, Value::Float(_)) && !matches!(y, Value::Float(_)) {
        let r = as_rational(vm, x)?;
        return div(vm, r, y);
    }
    let (fx, fy) = (opnd_f64(vm, x)?, opnd_f64(vm, y)?);
    new_from_f64(vm, fx / fy)
}

pub fn init(vm: &mut Vm) {
    let rat = vm.define_class("Rational", vm.core.numeric);
    vm.core.rational = rat;
    let sc = vm.singleton_class(Value::Obj(rat)).expect("Rational singleton");
    let new = vm.intern("new");
    let _ = vm.undef_method(sc, new);
    vm.define_methods(rat, &[
        ("numerator", |vm, s, _a, _b| Ok(parts(vm, s).map(|p| p.0).unwrap_or(Value::Nil))),
        ("denominator", |vm, s, _a, _b| Ok(parts(vm, s).map(|p| p.1).unwrap_or(Value::Nil))),
        ("to_i", |vm, s, _a, _b| { let (n, d) = big_parts(vm, s).unwrap_or((BigInt::zero(), BigInt::from_i64(1))); let q = n.divmod_trunc(&d).0; Ok(vm.bint_value(q)) }),
        ("truncate", |vm, s, _a, _b| { let (n, d) = big_parts(vm, s).unwrap_or((BigInt::zero(), BigInt::from_i64(1))); let q = n.divmod_trunc(&d).0; Ok(vm.bint_value(q)) }),
        ("to_r", |_vm, s, _a, _b| Ok(s)),
        ("to_f", |vm, s, _a, _b| Ok(Value::Float(to_f(vm, s)))),
        ("negative?", |vm, s, _a, _b| Ok(Value::bool(big_parts(vm, s).map(|(n, _)| n.sign() < 0).unwrap_or(false)))),
        ("==", |vm, s, a, _b| { argc!(vm, a, 1); Ok(Value::bool(eq(vm, s, a[0])?)) }),
        ("__cmp", |vm, s, a, _b| { argc!(vm, a, 1); Ok(match cmp_exact(vm, s, a[0]) { Some(o) => Value::Int(o as i64), None => Value::False }) }),
        ("-@", |vm, s, _a, _b| { let (n, d) = big_parts(vm, s).unwrap_or((BigInt::zero(), BigInt::from_i64(1))); new_rat(vm, &n.neg(), &d) }),
        ("+", |vm, s, a, _b| { argc!(vm, a, 1); add(vm, s, a[0]) }),
        ("-", |vm, s, a, _b| { argc!(vm, a, 1); sub(vm, s, a[0]) }),
        ("*", |vm, s, a, _b| { argc!(vm, a, 1); mul(vm, s, a[0]) }),
        ("/", |vm, s, a, _b| { argc!(vm, a, 1); div(vm, s, a[0]) }),
        ("quo", |vm, s, a, _b| { argc!(vm, a, 1); div(vm, s, a[0]) }),
        ("**", |vm, s, a, _b| { argc!(vm, a, 1); pow(vm, s, a[0]) }),
        // `mrb_eql`: the reference's `eql?` is true for two values of the same type that
        // are `==`, which for a Rational means the same reduced fraction
        ("eql?", |vm, s, a, _b| { argc!(vm, a, 1); if !is_rational(vm, a[0]) { return Ok(Value::False); } Ok(Value::bool(eq(vm, s, a[0])?)) }),
        ("hash", |vm, s, _a, _b| Ok(Value::Int(hash(vm, s)))),
    ]);
    vm.define_method(vm.core.integer, "to_r", |vm, s, _a, _b| { let n = vm.as_bigint(s).unwrap_or_else(BigInt::zero); new_rat(vm, &n, &BigInt::from_i64(1)) });
    vm.define_method(vm.core.nil_class, "to_r", |vm, _s, _a, _b| new_i(vm, 0, 1));
    vm.define_method(vm.core.float, "to_r", |vm, s, _a, _b| { let f = match s { Value::Float(f) => f, _ => 0.0 }; new_from_f64(vm, f) });
    let k = vm.core.kernel;
    // `mrb_define_module_function_id(..., MRB_SYM(Rational), ...)` (rational.c)
    vm.define_module_function(k, "Rational", kernel_rational).expect("Kernel singleton");
    let _ = vec![0u8];
}
