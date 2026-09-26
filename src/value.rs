
use crate::symbol::Sym;
extern crate alloc;

/// Handle to a heap object (index into [`crate::object::Heap`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjId(pub u32);

/// A Ruby value. Immediates are stored inline; everything else is a heap
/// handle. This is the "No Boxing" representation of the book's VM chapter:
/// the type tag is a Rust enum discriminant instead of tag bits in a word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Nil,
    False,
    True,
    Int(i64),
    Float(f64),
    Sym(Sym),
    Obj(ObjId),
}

impl Value {
    #[inline]
    pub fn truthy(self) -> bool {
        !matches!(self, Value::Nil | Value::False)
    }
    #[inline]
    pub fn is_nil(self) -> bool {
        matches!(self, Value::Nil)
    }
    #[inline]
    pub fn bool(b: bool) -> Value {
        if b { Value::True } else { Value::False }
    }
    #[inline]
    pub fn obj(self) -> Option<ObjId> {
        match self {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }
    /// Identity comparison (`equal?`). Floats compare by value like mruby's
    /// immediate floats.
    pub fn same(self, other: Value) -> bool {
        self == other
    }
}

/// A stored value: what registers, array elements, hash entries, instance variables,
/// environments, constants and globals hold. Code that computes works on [`Value`];
/// the only ways across the boundary are [`Slot::get`] and [`Slot::from`].
///
/// Today a `Slot` is a `Value` (16 bytes, `repr(transparent)`, zero cost). The point of
/// the type is that a compact 8-byte representation can be tried later by changing this
/// file alone (see `docs/design/performance.md`).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot(Value);

impl Slot {
    pub const NIL: Slot = Slot(Value::Nil);
    #[inline(always)]
    pub fn get(self) -> Value { self.0 }
    #[inline(always)]
    pub fn set(&mut self, v: Value) { self.0 = v; }
    #[inline(always)]
    pub fn is_nil(self) -> bool { matches!(self.0, Value::Nil) }
}

impl From<Value> for Slot {
    #[inline(always)]
    fn from(v: Value) -> Slot { Slot(v) }
}

impl Default for Slot {
    fn default() -> Self { Slot::NIL }
}

/// Copies stored values out (a conversion, free while `Slot` is transparent).
#[inline]
pub fn values_of(slots: &[Slot]) -> alloc::vec::Vec<Value> { slots.iter().map(|s| s.get()).collect() }
/// Stores values in (the inverse conversion).
#[inline]
pub fn slots_of(values: &[Value]) -> alloc::vec::Vec<Slot> { values.iter().map(|v| Slot::from(*v)).collect() }
