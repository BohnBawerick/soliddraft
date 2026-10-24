//! Exact bytes of a value, for cache keys.
//!
//! Two values get the same bytes only when every field is the same: floats by their bit
//! pattern, so `0.0` and `-0.0` differ, `NaN` and infinity differ, and nothing is rounded
//! to a grid. The encoding is prefix-free: each value starts with a tag, strings and
//! byte runs carry their length, and sequences, maps, structs and variants end with a
//! marker.

use serde::Serialize;
use serde::ser::{self, Serializer};

/// The exact bytes of `value`. `None` when serde cannot serialize it, such as a map
/// with a key that is not a plain value.
pub(crate) fn bytes<T: Serialize + ?Sized>(value: &T) -> Option<Vec<u8>> {
    let mut out = Exact(Vec::new());
    value.serialize(&mut out).ok()?;
    Some(out.0)
}

struct Exact(Vec<u8>);

#[derive(Debug)]
struct Unserializable(String);

impl std::fmt::Display for Unserializable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unserializable {}

impl ser::Error for Unserializable {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Unserializable(msg.to_string())
    }
}

const END: u8 = 0;

impl Exact {
    fn tag(&mut self, tag: u8) {
        self.0.push(tag);
    }

    fn len(&mut self, len: usize) {
        self.0.extend_from_slice(&(len as u64).to_le_bytes());
    }

    fn run(&mut self, tag: u8, bytes: &[u8]) {
        self.tag(tag);
        self.len(bytes.len());
        self.0.extend_from_slice(bytes);
    }

    fn variant(&mut self, tag: u8, index: u32) {
        self.tag(tag);
        self.0.extend_from_slice(&index.to_le_bytes());
    }
}

type Result<T = ()> = std::result::Result<T, Unserializable>;

impl Serializer for &mut Exact {
    type Ok = ();
    type Error = Unserializable;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> Result {
        self.tag(if v { 2 } else { 1 });
        Ok(())
    }

    fn serialize_i8(self, v: i8) -> Result {
        self.serialize_i64(v.into())
    }

    fn serialize_i16(self, v: i16) -> Result {
        self.serialize_i64(v.into())
    }

    fn serialize_i32(self, v: i32) -> Result {
        self.serialize_i64(v.into())
    }

    fn serialize_i64(self, v: i64) -> Result {
        self.tag(3);
        self.0.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_i128(self, v: i128) -> Result {
        self.tag(4);
        self.0.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_u8(self, v: u8) -> Result {
        self.serialize_u64(v.into())
    }

    fn serialize_u16(self, v: u16) -> Result {
        self.serialize_u64(v.into())
    }

    fn serialize_u32(self, v: u32) -> Result {
        self.serialize_u64(v.into())
    }

    fn serialize_u64(self, v: u64) -> Result {
        self.tag(5);
        self.0.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_u128(self, v: u128) -> Result {
        self.tag(6);
        self.0.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    fn serialize_f32(self, v: f32) -> Result {
        self.tag(7);
        self.0.extend_from_slice(&v.to_bits().to_le_bytes());
        Ok(())
    }

    fn serialize_f64(self, v: f64) -> Result {
        self.tag(8);
        self.0.extend_from_slice(&v.to_bits().to_le_bytes());
        Ok(())
    }

    fn serialize_char(self, v: char) -> Result {
        self.tag(9);
        self.0.extend_from_slice(&u32::from(v).to_le_bytes());
        Ok(())
    }

    fn serialize_str(self, v: &str) -> Result {
        self.run(10, v.as_bytes());
        Ok(())
    }

    fn serialize_bytes(self, v: &[u8]) -> Result {
        self.run(11, v);
        Ok(())
    }

    fn serialize_none(self) -> Result {
        self.tag(12);
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result {
        self.tag(13);
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result {
        self.tag(14);
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result {
        self.serialize_unit()
    }

    fn serialize_unit_variant(self, _: &'static str, index: u32, _: &'static str) -> Result {
        self.variant(15, index);
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, value: &T) -> Result {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        value: &T,
    ) -> Result {
        self.variant(16, index);
        value.serialize(self)
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self> {
        self.tag(17);
        Ok(self)
    }

    fn serialize_tuple(self, _: usize) -> Result<Self> {
        self.tag(18);
        Ok(self)
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self> {
        self.tag(18);
        Ok(self)
    }

    fn serialize_tuple_variant(self, _: &'static str, index: u32, _: &'static str, _: usize) -> Result<Self> {
        self.variant(19, index);
        Ok(self)
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self> {
        self.tag(20);
        Ok(self)
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self> {
        self.tag(21);
        Ok(self)
    }

    fn serialize_struct_variant(self, _: &'static str, index: u32, _: &'static str, _: usize) -> Result<Self> {
        self.variant(22, index);
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut Exact {
    type Ok = ();
    type Error = Unserializable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result {
        self.tag(END);
        Ok(())
    }
}

impl ser::SerializeTuple for &mut Exact {
    type Ok = ();
    type Error = Unserializable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result {
        self.tag(END);
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut Exact {
    type Ok = ();
    type Error = Unserializable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result {
        self.tag(END);
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut Exact {
    type Ok = ();
    type Error = Unserializable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result {
        self.tag(END);
        Ok(())
    }
}

impl ser::SerializeMap for &mut Exact {
    type Ok = ();
    type Error = Unserializable;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result {
        key.serialize(&mut **self)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result {
        value.serialize(&mut **self)
    }

    fn end(self) -> Result {
        self.tag(END);
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Exact {
    type Ok = ();
    type Error = Unserializable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result {
        // Fields a type skips leave no bytes, so the name keeps the remaining ones apart.
        self.run(10, key.as_bytes());
        value.serialize(&mut **self)
    }

    fn end(self) -> Result {
        self.tag(END);
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Exact {
    type Ok = ();
    type Error = Unserializable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result {
        self.run(10, key.as_bytes());
        value.serialize(&mut **self)
    }

    fn end(self) -> Result {
        self.tag(END);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::bytes;

    #[test]
    fn floats_are_told_apart_by_their_bits() {
        assert_ne!(bytes(&0.0f64), bytes(&-0.0f64));
        assert_ne!(bytes(&f64::NAN), bytes(&f64::INFINITY));
        assert_ne!(bytes(&f64::INFINITY), bytes(&f64::NEG_INFINITY));
        assert_ne!(bytes(&1.0f64), bytes(&(1.0f64 + f64::EPSILON)));
        assert_eq!(bytes(&[0.1f64, 2.5]), bytes(&[0.1f64, 2.5]));
    }

    #[test]
    fn neighbouring_values_do_not_run_together() {
        assert_ne!(bytes(&("ab", "c")), bytes(&("a", "bc")));
        assert_ne!(bytes(&vec![vec![1u32], vec![]]), bytes(&vec![vec![], vec![1u32]]));
        assert_ne!(bytes(&Some(())), bytes(&None::<()>));
    }
}
