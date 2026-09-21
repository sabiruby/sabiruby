//! Rust → Ruby: serde's data model as the VM's values.
//!
//! The mapping is [the crate documentation](crate)'s table. Everything here goes through
//! [`Vm`]'s own constructors (`str_new`, `ary_new`, `hash_new`, `hash_set`, `bint_value`), so
//! a value this builds is indistinguishable from one a script built.

use alloc::string::ToString;
use alloc::vec::Vec;

use sabiruby::value::Value;
use sabiruby::Vm;
use serde::ser::{self, Serialize};

use crate::error::{Error, Result};
use crate::Options;

/// The serializer [`to_value`](crate::to_value) drives: a VM to allocate in, and how to write
/// the keys.
///
/// It is used through `&mut`, as serde's own serializers are: every part of a value is
/// serialized by the same one.
pub struct Serializer<'v> {
    vm: &'v mut Vm,
    opts: Options,
}

impl<'v> Serializer<'v> {
    pub fn new(vm: &'v mut Vm, opts: Options) -> Serializer<'v> {
        Serializer { vm, opts }
    }
    /// The VM this serializer allocates in.
    pub fn vm(&mut self) -> &mut Vm { self.vm }

    /// A field or variant name as a Hash key: a String, or a Symbol under
    /// [`Options::symbol_keys`].
    fn key(&mut self, name: &str) -> Value {
        if self.opts.symbol_keys {
            Value::Sym(self.vm.intern(name))
        } else {
            self.vm.str_new(name.as_bytes())
        }
    }

    /// A map's key as it goes into the Hash: what it serialized to, except that under
    /// [`Options::symbol_map_keys`] a key that came out a String becomes a Symbol.
    ///
    /// The test is the value, not the Rust type, because "the key is written as a string" is
    /// exactly what a host means here: `String`, `&str` and `char` all land on a String, and so
    /// does a newtype or a `Some` around one. A String that is not valid UTF-8 has no Symbol to
    /// become and stays a String, and so does one marked binary (`serialize_bytes`), which is
    /// bytes and not text — the same line this crate draws everywhere it reads text
    /// (`docs/design/serde.md`, "The data model").
    fn map_key(&mut self, k: Value) -> Value {
        if !self.opts.symbol_map_keys || self.vm.str_binary(k) { return k; }
        let name = match self.vm.str_bytes(k).map(core::str::from_utf8) {
            Some(Ok(s)) => alloc::string::String::from(s),
            _ => return k,
        };
        Value::Sym(self.vm.intern(&name))
    }

    /// `{ name => value }`: how every variant that carries something is written.
    fn tagged(&mut self, name: &str, value: Value) -> Result<Value> {
        let k = self.key(name);
        let h = self.vm.hash_new();
        self.vm.hash_set(h, k, value).map_err(Error::Vm)?;
        Ok(h)
    }

    fn int128(&mut self, s: alloc::string::String) -> Result<Value> {
        match sabiruby::bigint::BigInt::from_str(s.as_bytes(), 10) {
            Some(b) => Ok(self.vm.bint_value(b)),
            None => Err(Error::Message(alloc::format!("{s} is not an integer"))),
        }
    }
}

impl<'a, 'v> ser::Serializer for &'a mut Serializer<'v> {
    type Ok = Value;
    type Error = Error;
    type SerializeSeq = SeqSer<'a, 'v>;
    type SerializeTuple = SeqSer<'a, 'v>;
    type SerializeTupleStruct = SeqSer<'a, 'v>;
    type SerializeTupleVariant = TupleVariantSer<'a, 'v>;
    type SerializeMap = MapSer<'a, 'v>;
    type SerializeStruct = StructSer<'a, 'v>;
    type SerializeStructVariant = StructVariantSer<'a, 'v>;

    fn serialize_bool(self, v: bool) -> Result<Value> { Ok(Value::bool(v)) }
    fn serialize_i8(self, v: i8) -> Result<Value> { Ok(Value::Int(v as i64)) }
    fn serialize_i16(self, v: i16) -> Result<Value> { Ok(Value::Int(v as i64)) }
    fn serialize_i32(self, v: i32) -> Result<Value> { Ok(Value::Int(v as i64)) }
    fn serialize_i64(self, v: i64) -> Result<Value> { Ok(Value::Int(v)) }
    fn serialize_i128(self, v: i128) -> Result<Value> {
        match i64::try_from(v) { Ok(i) => Ok(Value::Int(i)), Err(_) => self.int128(v.to_string()) }
    }
    fn serialize_u8(self, v: u8) -> Result<Value> { Ok(Value::Int(v as i64)) }
    fn serialize_u16(self, v: u16) -> Result<Value> { Ok(Value::Int(v as i64)) }
    fn serialize_u32(self, v: u32) -> Result<Value> { Ok(Value::Int(v as i64)) }
    /// Wider than an `i64` becomes a wide Integer, as `u64` does through
    /// [`IntoRuby`](sabiruby::IntoRuby): Ruby's Integer has no bound.
    fn serialize_u64(self, v: u64) -> Result<Value> {
        match i64::try_from(v) {
            Ok(i) => Ok(Value::Int(i)),
            Err(_) => Ok(self.vm.bint_value(sabiruby::bigint::BigInt::from_u64(v))),
        }
    }
    fn serialize_u128(self, v: u128) -> Result<Value> {
        match i64::try_from(v) { Ok(i) => Ok(Value::Int(i)), Err(_) => self.int128(v.to_string()) }
    }
    fn serialize_f32(self, v: f32) -> Result<Value> { Ok(Value::Float(v as f64)) }
    fn serialize_f64(self, v: f64) -> Result<Value> { Ok(Value::Float(v)) }
    /// A one-character String, not an Integer: Ruby has no character type.
    fn serialize_char(self, v: char) -> Result<Value> {
        let mut buf = [0u8; 4];
        Ok(self.vm.str_new(v.encode_utf8(&mut buf).as_bytes()))
    }
    fn serialize_str(self, v: &str) -> Result<Value> { Ok(self.vm.str_new(v.as_bytes())) }
    /// A String holding the bytes unchanged, marked binary (`ASCII-8BIT`) because bytes are
    /// not text: this is where a `Vec<u8>` behind `serde_bytes` lands, and where a Ruby String
    /// that is not UTF-8 comes back to.
    fn serialize_bytes(self, v: &[u8]) -> Result<Value> {
        let s = self.vm.str_new(v);
        self.vm.str_set_binary(s, true);
        Ok(s)
    }
    fn serialize_none(self) -> Result<Value> { Ok(Value::Nil) }
    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Value> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Value> { Ok(Value::Nil) }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value> { Ok(Value::Nil) }
    /// A Symbol: `Colour::Red` is `:Red`, which reads in Ruby the way the enum reads in Rust.
    fn serialize_unit_variant(self, _name: &'static str, _idx: u32, variant: &'static str) -> Result<Value> {
        Ok(Value::Sym(self.vm.intern(variant)))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _name: &'static str, value: &T) -> Result<Value> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self, _name: &'static str, _idx: u32, variant: &'static str, value: &T,
    ) -> Result<Value> {
        let v = value.serialize(&mut *self)?;
        self.tagged(variant, v)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSer<'a, 'v>> {
        Ok(SeqSer { ser: self, items: Vec::with_capacity(len.unwrap_or(0)) })
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSer<'a, 'v>> {
        Ok(SeqSer { ser: self, items: Vec::with_capacity(len) })
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SeqSer<'a, 'v>> {
        Ok(SeqSer { ser: self, items: Vec::with_capacity(len) })
    }
    fn serialize_tuple_variant(
        self, _name: &'static str, _idx: u32, variant: &'static str, len: usize,
    ) -> Result<TupleVariantSer<'a, 'v>> {
        Ok(TupleVariantSer { ser: self, variant, items: Vec::with_capacity(len) })
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapSer<'a, 'v>> {
        Ok(MapSer { ser: self, pairs: Vec::with_capacity(len.unwrap_or(0)), key: None })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<StructSer<'a, 'v>> {
        Ok(StructSer { ser: self, pairs: Vec::with_capacity(len) })
    }
    fn serialize_struct_variant(
        self, _name: &'static str, _idx: u32, variant: &'static str, len: usize,
    ) -> Result<StructVariantSer<'a, 'v>> {
        Ok(StructVariantSer { ser: self, variant, pairs: Vec::with_capacity(len) })
    }
}

/// The parts of an Array are collected first and the Array built once: nothing half-built is
/// reachable from Ruby, and the elements stay in a Rust local, where the collector may not run
/// (`docs/design/gc.md`: collection happens at an instruction boundary, never inside a host
/// call that is not running Ruby).
pub struct SeqSer<'a, 'v> {
    ser: &'a mut Serializer<'v>,
    items: Vec<Value>,
}

impl<'a, 'v> ser::SerializeSeq for SeqSer<'a, 'v> {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let v = value.serialize(&mut *self.ser)?;
        self.items.push(v);
        Ok(())
    }
    fn end(self) -> Result<Value> { Ok(self.ser.vm.ary_new(self.items)) }
}

impl<'a, 'v> ser::SerializeTuple for SeqSer<'a, 'v> {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Value> { ser::SerializeSeq::end(self) }
}

impl<'a, 'v> ser::SerializeTupleStruct for SeqSer<'a, 'v> {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<Value> { ser::SerializeSeq::end(self) }
}

/// `Shape::Rect(2, 3)` → `{"Rect" => [2, 3]}`.
pub struct TupleVariantSer<'a, 'v> {
    ser: &'a mut Serializer<'v>,
    variant: &'static str,
    items: Vec<Value>,
}

impl<'a, 'v> ser::SerializeTupleVariant for TupleVariantSer<'a, 'v> {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let v = value.serialize(&mut *self.ser)?;
        self.items.push(v);
        Ok(())
    }
    fn end(self) -> Result<Value> {
        let a = self.ser.vm.ary_new(self.items);
        self.ser.tagged(self.variant, a)
    }
}

/// A map. The keys are whatever the key type serializes to — a Ruby Hash takes any value as a
/// key — so `HashMap<i64, _>` keeps its Integer keys rather than stringifying them.
/// [`Options::symbol_map_keys`] is the one exception: it turns a key that came out a String
/// into a Symbol (`Serializer::map_key`).
pub struct MapSer<'a, 'v> {
    ser: &'a mut Serializer<'v>,
    pairs: Vec<(Value, Value)>,
    key: Option<Value>,
}

impl<'a, 'v> ser::SerializeMap for MapSer<'a, 'v> {
    type Ok = Value;
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<()> {
        let k = key.serialize(&mut *self.ser)?;
        self.key = Some(self.ser.map_key(k));
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<()> {
        let v = value.serialize(&mut *self.ser)?;
        let k = self.key.take().ok_or_else(|| Error::Message("value serialized before its key".into()))?;
        self.pairs.push((k, v));
        Ok(())
    }
    fn end(self) -> Result<Value> { build_hash(self.ser, self.pairs) }
}

/// A struct: a Hash whose keys are the field names.
pub struct StructSer<'a, 'v> {
    ser: &'a mut Serializer<'v>,
    pairs: Vec<(Value, Value)>,
}

impl<'a, 'v> ser::SerializeStruct for StructSer<'a, 'v> {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, name: &'static str, value: &T) -> Result<()> {
        let v = value.serialize(&mut *self.ser)?;
        let k = self.ser.key(name);
        self.pairs.push((k, v));
        Ok(())
    }
    fn end(self) -> Result<Value> { build_hash(self.ser, self.pairs) }
}

/// `Shape::Rect { w, h }` → `{"Rect" => {"w" => …, "h" => …}}`.
pub struct StructVariantSer<'a, 'v> {
    ser: &'a mut Serializer<'v>,
    variant: &'static str,
    pairs: Vec<(Value, Value)>,
}

impl<'a, 'v> ser::SerializeStructVariant for StructVariantSer<'a, 'v> {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, name: &'static str, value: &T) -> Result<()> {
        let v = value.serialize(&mut *self.ser)?;
        let k = self.ser.key(name);
        self.pairs.push((k, v));
        Ok(())
    }
    fn end(self) -> Result<Value> {
        let inner = build_hash(self.ser, self.pairs)?;
        self.ser.tagged(self.variant, inner)
    }
}

fn build_hash(ser: &mut Serializer<'_>, pairs: Vec<(Value, Value)>) -> Result<Value> {
    let h = ser.vm.hash_new();
    for (k, v) in pairs {
        ser.vm.hash_set(h, k, v).map_err(Error::Vm)?;
    }
    Ok(h)
}
