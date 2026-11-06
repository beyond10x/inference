//! The protocol buffers wire format, as far as the Opal messages use it: varints, 32- and 64-bit
//! fixed fields and length-delimited fields, written and read without a code generator.
//!
//! Writing follows proto3: a scalar field at its default (zero, false, empty) is left out, and a
//! message field is written whenever it is present, empty or not, so an empty member of a
//! `oneof` (`Ping {}`) still says which member it is. Reading follows the parsing rules of the
//! encoding guide: fields come in any order, the last occurrence of a scalar wins, and a field
//! number the reader does not know is skipped, whatever its wire type except the deprecated
//! groups, which no Opal message uses and which are refused.

use std::fmt;

/// The wire types.
const VARINT: u8 = 0;
const FIXED64: u8 = 1;
const LEN: u8 = 2;
const FIXED32: u8 = 5;

/// Why bytes are not a message of the wire format, or not the message read from them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The bytes end inside a field.
    Truncated,
    /// A varint runs past ten bytes or past 64 bits.
    Varint,
    /// A field number of 0, or a group, or a wire type the format does not define.
    WireType { field: u64, wire_type: u8 },
    /// A field the reader knows came in a wire type other than its own.
    Mismatch { field: u32 },
    /// A string field is not UTF-8.
    Utf8 { field: u32 },
    /// A value lies outside its field's range, such as a `uint32` above `u32::MAX`.
    Range { field: u32 },
    /// Messages nest deeper than the reader follows.
    Depth,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("the bytes end inside a field"),
            Self::Varint => f.write_str("a varint is longer than 64 bits"),
            Self::WireType { field, wire_type } => {
                write!(
                    f,
                    "field {field} has the wire type {wire_type}, which is not read"
                )
            }
            Self::Mismatch { field } => write!(f, "field {field} has the wrong wire type"),
            Self::Utf8 { field } => write!(f, "field {field} is not UTF-8"),
            Self::Range { field } => write!(f, "field {field} is out of range"),
            Self::Depth => f.write_str("the message nests too deeply"),
        }
    }
}

impl std::error::Error for WireError {}

/// Builds one message.
#[derive(Debug, Default)]
pub(crate) struct Writer(Vec<u8>);

impl Writer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.0
    }

    fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push((value as u8) | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }

    fn tag(&mut self, field: u32, wire_type: u8) {
        self.varint((u64::from(field) << 3) | u64::from(wire_type));
    }

    /// A `uint32`, `uint64` or enum field; left out at zero.
    pub(crate) fn uint(&mut self, field: u32, value: u64) {
        if value != 0 {
            self.tag(field, VARINT);
            self.varint(value);
        }
    }

    /// An `optional uint32`: written whenever it is present, zero included.
    pub(crate) fn optional_uint(&mut self, field: u32, value: Option<u64>) {
        if let Some(value) = value {
            self.tag(field, VARINT);
            self.varint(value);
        }
    }

    /// A `bool` field; left out when false.
    pub(crate) fn bool(&mut self, field: u32, value: bool) {
        self.uint(field, u64::from(value));
    }

    /// A `float` field; left out at positive zero.
    pub(crate) fn float(&mut self, field: u32, value: f32) {
        if value.to_bits() != 0 {
            self.tag(field, FIXED32);
            self.0.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// A `double` field; left out at positive zero.
    pub(crate) fn double(&mut self, field: u32, value: f64) {
        if value.to_bits() != 0 {
            self.tag(field, FIXED64);
            self.0.extend_from_slice(&value.to_le_bytes());
        }
    }

    /// A `double` that is the member of a `oneof`: written whatever its value.
    pub(crate) fn double_always(&mut self, field: u32, value: f64) {
        self.tag(field, FIXED64);
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// A `bytes` field; left out when empty.
    pub(crate) fn bytes(&mut self, field: u32, value: &[u8]) {
        if !value.is_empty() {
            self.bytes_always(field, value);
        }
    }

    /// A `bytes` or `string` field written whatever its value: an `optional` one, or the member
    /// of a `oneof`.
    pub(crate) fn bytes_always(&mut self, field: u32, value: &[u8]) {
        self.tag(field, LEN);
        self.varint(value.len() as u64);
        self.0.extend_from_slice(value);
    }

    /// A `string` field; left out when empty.
    pub(crate) fn string(&mut self, field: u32, value: &str) {
        self.bytes(field, value.as_bytes());
    }

    /// An `optional string`: written whenever it is present, empty included.
    pub(crate) fn optional_string(&mut self, field: u32, value: Option<&str>) {
        if let Some(value) = value {
            self.bytes_always(field, value.as_bytes());
        }
    }

    /// A message field, written whatever it holds.
    pub(crate) fn message(&mut self, field: u32, build: impl FnOnce(&mut Writer)) {
        let mut inner = Writer::new();
        build(&mut inner);
        self.bytes_always(field, &inner.0);
    }
}

/// One field as read, before its type is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Value<'a> {
    pub(crate) fn uint(self, field: u32) -> Result<u64, WireError> {
        match self {
            Self::Varint(value) => Ok(value),
            _ => Err(WireError::Mismatch { field }),
        }
    }

    pub(crate) fn uint32(self, field: u32) -> Result<u32, WireError> {
        u32::try_from(self.uint(field)?).map_err(|_| WireError::Range { field })
    }

    /// An enum: proto3 enums are `int32` on the wire, so a negative value is sign-extended to 64
    /// bits; it is read back as the `i32` it was.
    pub(crate) fn enumeration(self, field: u32) -> Result<i32, WireError> {
        let raw = self.uint(field)?;
        i32::try_from(raw as i64).map_err(|_| WireError::Range { field })
    }

    pub(crate) fn bool(self, field: u32) -> Result<bool, WireError> {
        Ok(self.uint(field)? != 0)
    }

    pub(crate) fn float(self, field: u32) -> Result<f32, WireError> {
        match self {
            Self::Fixed32(bits) => Ok(f32::from_bits(bits)),
            _ => Err(WireError::Mismatch { field }),
        }
    }

    pub(crate) fn double(self, field: u32) -> Result<f64, WireError> {
        match self {
            Self::Fixed64(bits) => Ok(f64::from_bits(bits)),
            _ => Err(WireError::Mismatch { field }),
        }
    }

    pub(crate) fn bytes(self, field: u32) -> Result<&'a [u8], WireError> {
        match self {
            Self::Len(bytes) => Ok(bytes),
            _ => Err(WireError::Mismatch { field }),
        }
    }

    pub(crate) fn string(self, field: u32) -> Result<String, WireError> {
        std::str::from_utf8(self.bytes(field)?)
            .map(str::to_owned)
            .map_err(|_| WireError::Utf8 { field })
    }
}

/// Reads the fields of one message in order.
#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn varint(&mut self) -> Result<u64, WireError> {
        let mut value = 0_u64;
        for index in 0..10 {
            let (&byte, rest) = self.bytes.split_first().ok_or(WireError::Truncated)?;
            self.bytes = rest;
            let low = u64::from(byte & 0x7f);
            if index == 9 && low > 1 {
                return Err(WireError::Varint);
            }
            value |= low << (7 * index);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(WireError::Varint)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], WireError> {
        if self.bytes.len() < count {
            return Err(WireError::Truncated);
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    /// The next field and its number, `None` at the end of the message. A field number beyond
    /// `u32` is refused, since no Opal message has one.
    pub(crate) fn next_field(&mut self) -> Option<Result<(u32, Value<'a>), WireError>> {
        if self.bytes.is_empty() {
            return None;
        }
        Some(self.field())
    }

    fn field(&mut self) -> Result<(u32, Value<'a>), WireError> {
        let key = self.varint()?;
        let wire_type = (key & 0x7) as u8;
        let number = key >> 3;
        let field = match u32::try_from(number) {
            Ok(field) if field != 0 => field,
            _ => {
                return Err(WireError::WireType {
                    field: number,
                    wire_type,
                });
            }
        };
        let value = match wire_type {
            VARINT => Value::Varint(self.varint()?),
            FIXED64 => {
                let bytes = self.take(8)?;
                let mut word = [0; 8];
                word.copy_from_slice(bytes);
                Value::Fixed64(u64::from_le_bytes(word))
            }
            LEN => {
                let length = usize::try_from(self.varint()?).map_err(|_| WireError::Truncated)?;
                Value::Len(self.take(length)?)
            }
            FIXED32 => {
                let bytes = self.take(4)?;
                let mut word = [0; 4];
                word.copy_from_slice(bytes);
                Value::Fixed32(u32::from_le_bytes(word))
            }
            _ => {
                return Err(WireError::WireType {
                    field: number,
                    wire_type,
                });
            }
        };
        Ok((field, value))
    }
}

/// Calls `each` with every field of `bytes`, in order.
pub(crate) fn fields<'a>(
    bytes: &'a [u8],
    mut each: impl FnMut(u32, Value<'a>) -> Result<(), WireError>,
) -> Result<(), WireError> {
    let mut reader = Reader::new(bytes);
    while let Some(field) = reader.next_field() {
        let (number, value) = field?;
        each(number, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoding guide's own examples: 150 in field 1 is `08 96 01`, "testing" in field 2 is
    /// `12 07 74 65 73 74 69 6e 67`.
    #[test]
    fn the_encoding_guides_examples_are_written_and_read_back() {
        let mut writer = Writer::new();
        writer.uint(1, 150);
        writer.string(2, "testing");
        let bytes = writer.finish();
        assert_eq!(
            bytes,
            [
                0x08, 0x96, 0x01, 0x12, 0x07, b't', b'e', b's', b't', b'i', b'n', b'g'
            ]
        );
        let mut read = Vec::new();
        fields(&bytes, |field, value| {
            read.push((field, value));
            Ok(())
        })
        .unwrap();
        assert_eq!(read, [(1, Value::Varint(150)), (2, Value::Len(b"testing"))]);
    }

    #[test]
    fn defaults_are_left_out_and_present_messages_are_kept() {
        let mut writer = Writer::new();
        writer.uint(1, 0);
        writer.bool(2, false);
        writer.string(3, "");
        writer.float(4, 0.0);
        writer.double(5, 0.0);
        writer.message(11, |_| {});
        writer.optional_uint(6, Some(0));
        assert_eq!(writer.finish(), [0x5a, 0x00, 0x30, 0x00]);
    }

    #[test]
    fn a_float_and_a_double_are_little_endian_fixed_fields() {
        let mut writer = Writer::new();
        writer.float(1, 0.4);
        writer.double(2, 0.7);
        let bytes = writer.finish();
        let mut floats = Vec::new();
        fields(&bytes, |field, value| {
            match field {
                1 => floats.push(f64::from(value.float(1)?)),
                _ => floats.push(value.double(2)?),
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(floats, [f64::from(0.4_f32), 0.7]);
        assert_eq!(bytes[0], 0x0d);
        assert_eq!(bytes[5], 0x11);
    }

    #[test]
    fn a_negative_enum_reads_back_as_itself() {
        let mut writer = Writer::new();
        writer.uint(1, (-3_i64) as u64);
        let bytes = writer.finish();
        assert_eq!(bytes.len(), 11);
        fields(&bytes, |field, value| {
            assert_eq!(value.enumeration(field)?, -3);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn broken_bytes_are_refused_never_read_past() {
        for (bytes, error) in [
            (&[0x08][..], WireError::Truncated),
            (&[0x12, 0x05, b'a'][..], WireError::Truncated),
            (&[0x0d, 0x00][..], WireError::Truncated),
            (
                &[
                    0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
                ][..],
                WireError::Varint,
            ),
            (
                &[0x0b][..],
                WireError::WireType {
                    field: 1,
                    wire_type: 3,
                },
            ),
            (
                &[0x00, 0x00][..],
                WireError::WireType {
                    field: 0,
                    wire_type: 0,
                },
            ),
        ] {
            assert_eq!(fields(bytes, |_, _| Ok(())), Err(error), "{bytes:?}");
        }
    }

    #[test]
    fn a_value_read_as_another_wire_type_is_a_mismatch() {
        assert_eq!(
            Value::Varint(1).bytes(3),
            Err(WireError::Mismatch { field: 3 })
        );
        assert_eq!(
            Value::Len(b"x").uint(1),
            Err(WireError::Mismatch { field: 1 })
        );
        assert_eq!(
            Value::Varint(u64::from(u32::MAX) + 1).uint32(2),
            Err(WireError::Range { field: 2 })
        );
        assert_eq!(
            Value::Len(&[0xff]).string(4),
            Err(WireError::Utf8 { field: 4 })
        );
    }
}
