// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Messages of a pool's types, read and written by reflection (spec 0424
//! S3).
//!
//! A [`DynamicMessage`] owns an arena, where upb allocates the message, its
//! sub-messages, arrays, maps and strings. [`MessageRef`] reads a message
//! or a sub-message, borrowed from the [`DynamicMessage`] that owns it;
//! [`MessageMut`] writes one. No raw pointer leaves this module.
//!
//! **Fields belong to their message.** Every method that takes a [`Field`]
//! panics unless the field is one of the message's own, or an extension of
//! its type: upb would read or write the wrong memory otherwise.
//!
//! **Threads.** Reads never allocate (encoding uses an arena of its own),
//! so a [`DynamicMessage`] is `Sync`: any number of threads may read it,
//! while writing takes `&mut`.
//!
//! **Strings** come back as bytes, as `bytes` fields do: upb checks UTF-8
//! only where the schema asks it to, so whether a value is text is the
//! caller's to decide.

use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::{ffi, Cardinality, Field, Kind, Message};

// ── Arena ───────────────────────────────────────────────────────────────────

struct Arena(NonNull<ffi::Arena>);

impl Arena {
    fn new() -> Arena {
        // SAFETY: no precondition; null only when out of memory.
        NonNull::new(unsafe { ffi::upb_Arena_New() })
            .map(Arena)
            .expect("upb_Arena_New: out of memory")
    }

    fn raw(&self) -> *mut ffi::Arena {
        self.0.as_ptr()
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        // SAFETY: from `upb_Arena_New`, freed once.
        unsafe { ffi::upb_Arena_Free(self.0.as_ptr()) }
    }
}

// ── Errors and values ───────────────────────────────────────────────────────

/// Why a message could not be decoded, encoded or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageError(pub String);

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MessageError {}

/// What a field holds. `string` and `bytes` are both [`Value::Bytes`];
/// an enum is its number.
#[derive(Debug, Clone)]
pub enum Value<'a> {
    Bool(bool),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Enum(i32),
    Bytes(&'a [u8]),
    /// `None` for a message field that is not set.
    Message(Option<MessageRef<'a>>),
    List(List<'a>),
    Map(MapRef<'a>),
}

/// A value to write: what a singular scalar field, a list element or a map
/// key or value can hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar<'b> {
    Bool(bool),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Enum(i32),
    Bytes(&'b [u8]),
}

/// Read one value of type `kind` out of a upb value.
///
/// # Safety
/// `raw` holds a value of `kind`, alive for `'a`.
unsafe fn read_value<'a>(raw: ffi::MessageValue, kind: &Kind) -> Value<'a> {
    // SAFETY (each arm): the union member that `kind` selects, which is
    // the one upb wrote.
    unsafe {
        match kind {
            Kind::Double => Value::F64(raw.double_val),
            Kind::Float => Value::F32(raw.float_val),
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => Value::I64(raw.int64_val),
            Kind::Uint64 | Kind::Fixed64 => Value::U64(raw.uint64_val),
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => Value::I32(raw.int32_val),
            Kind::Uint32 | Kind::Fixed32 => Value::U32(raw.uint32_val),
            Kind::Bool => Value::Bool(raw.bool_val),
            Kind::Enum(_) => Value::Enum(raw.int32_val),
            Kind::String | Kind::Bytes => {
                let s = raw.str_val;
                Value::Bytes(if s.size == 0 {
                    &[]
                } else {
                    std::slice::from_raw_parts(s.data.cast(), s.size)
                })
            }
            Kind::Message(def) => {
                Value::Message(NonNull::new(raw.msg_val.cast_mut()).map(|msg| MessageRef {
                    msg,
                    def: def.clone(),
                    _life: PhantomData,
                }))
            }
        }
    }
}

/// Write a scalar of type `kind` into a upb value, copying bytes into
/// `arena`.
fn write_value(
    kind: &Kind,
    value: &Scalar<'_>,
    arena: *mut ffi::Arena,
) -> Result<ffi::MessageValue, MessageError> {
    let mismatch = || MessageError(format!("a {value:?} cannot be written to a {kind:?} field"));
    Ok(match (kind, value) {
        (Kind::Double, Scalar::F64(v)) => ffi::MessageValue { double_val: *v },
        (Kind::Float, Scalar::F32(v)) => ffi::MessageValue { float_val: *v },
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, Scalar::I64(v)) => {
            ffi::MessageValue { int64_val: *v }
        }
        (Kind::Uint64 | Kind::Fixed64, Scalar::U64(v)) => ffi::MessageValue { uint64_val: *v },
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, Scalar::I32(v)) => {
            ffi::MessageValue { int32_val: *v }
        }
        (Kind::Uint32 | Kind::Fixed32, Scalar::U32(v)) => ffi::MessageValue { uint32_val: *v },
        (Kind::Bool, Scalar::Bool(v)) => ffi::MessageValue { bool_val: *v },
        (Kind::Enum(_), Scalar::Enum(v)) => ffi::MessageValue { int32_val: *v },
        (Kind::String | Kind::Bytes, Scalar::Bytes(b)) => {
            // upb keeps a pointer to the bytes: they are copied into the
            // message's arena, which lives as long as the message.
            // SAFETY: a valid arena; at least one byte is asked for.
            let data = unsafe { ffi::upb_Arena_Malloc(arena, b.len().max(1)) };
            assert!(!data.is_null(), "upb_Arena_Malloc: out of memory");
            // SAFETY: `data` has room for `b.len()` bytes and does not
            // overlap `b`.
            unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), data.cast(), b.len()) };
            ffi::MessageValue {
                str_val: ffi::StringView {
                    data: data.cast(),
                    size: b.len(),
                },
            }
        }
        _ => return Err(mismatch()),
    })
}

/// Panic unless `field` belongs to `def`: one of its fields, or an
/// extension of it.
fn check_field(def: &Message, field: &Field) {
    assert!(
        field.containing_message() == *def,
        "field {} does not belong to message {}",
        field.full_name(),
        def.full_name()
    );
}

/// The key and value fields of a map field's entry type.
fn map_entry_fields(field: &Field) -> (Field, Field) {
    let Kind::Message(entry) = field.kind() else {
        unreachable!("a map field's type is its entry message");
    };
    let key = entry.get_field(1).expect("a map entry has a key");
    let value = entry.get_field(2).expect("a map entry has a value");
    (key, value)
}

fn mini_table(def: &Message) -> *const ffi::MiniTable {
    // SAFETY: a valid definition; the table lives in the pool.
    unsafe { ffi::upb_MessageDef_MiniTable(def.raw()) }
}

// ── DynamicMessage ──────────────────────────────────────────────────────────

/// A message of one of a pool's types, owning its memory.
pub struct DynamicMessage {
    // Declared before `arena`: nothing to drop, but the order states that
    // the message lives in the arena.
    msg: NonNull<ffi::RawMessage>,
    def: Message,
    arena: Arena,
}

// SAFETY: the message and its arena are owned. `&self` methods only read
// (encoding allocates in an arena of its own), and upb's reads keep no
// state; `&mut self` methods are exclusive.
unsafe impl Send for DynamicMessage {}
unsafe impl Sync for DynamicMessage {}

impl DynamicMessage {
    /// An empty message of type `def`.
    pub fn new(def: &Message) -> DynamicMessage {
        let arena = Arena::new();
        // SAFETY: a valid table and arena.
        let msg = NonNull::new(unsafe { ffi::upb_Message_New(mini_table(def), arena.raw()) })
            .expect("upb_Message_New: out of memory");
        DynamicMessage {
            msg,
            def: def.clone(),
            arena,
        }
    }

    /// Decode `bytes` as a message of type `def`. Extensions known to the
    /// pool are decoded as extensions, the others kept as unknown fields;
    /// unknown fields are re-encoded as they came.
    pub fn decode(def: &Message, bytes: &[u8]) -> Result<DynamicMessage, MessageError> {
        let message = DynamicMessage::new(def);
        let status = {
            // The extension registry grows as files are added.
            let _state = def.pool.read();
            // SAFETY: a valid pool, message, table and arena; `bytes` is
            // valid for its length and copied (option 0: no aliasing).
            unsafe {
                let extreg = ffi::upb_DefPool_ExtensionRegistry(def.pool.raw.as_ptr());
                ffi::upb_Decode(
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    message.msg.as_ptr(),
                    mini_table(def),
                    extreg,
                    0,
                    message.arena.raw(),
                )
            }
        };
        match status {
            ffi::STATUS_OK => Ok(message),
            1 => Err(MessageError("malformed message".into())),
            2 => Err(MessageError("out of memory".into())),
            3 => Err(MessageError("invalid UTF-8 in a string field".into())),
            4 => Err(MessageError("nested too deep".into())),
            other => Err(MessageError(format!("upb_Decode status {other}"))),
        }
    }

    /// The message's type.
    pub fn descriptor(&self) -> &Message {
        &self.def
    }

    pub fn as_ref(&self) -> MessageRef<'_> {
        MessageRef {
            msg: self.msg,
            def: self.def.clone(),
            _life: PhantomData,
        }
    }

    pub fn as_mut(&mut self) -> MessageMut<'_> {
        MessageMut {
            msg: self.msg,
            def: self.def.clone(),
            arena: self.arena.raw(),
            _life: PhantomData,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, MessageError> {
        self.as_ref().encode()
    }
}

impl Clone for DynamicMessage {
    /// A deep copy, in an arena of its own.
    fn clone(&self) -> DynamicMessage {
        let arena = Arena::new();
        // SAFETY: a valid message of this table; a valid arena.
        let msg = NonNull::new(unsafe {
            ffi::upb_Message_DeepClone(self.msg.as_ptr(), mini_table(&self.def), arena.raw())
        })
        .expect("upb_Message_DeepClone: out of memory");
        DynamicMessage {
            msg,
            def: self.def.clone(),
            arena,
        }
    }
}

impl fmt::Debug for DynamicMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DynamicMessage({})", self.def.full_name())
    }
}

// ── Reading ─────────────────────────────────────────────────────────────────

/// A message to read: a [`DynamicMessage`] or one of its sub-messages.
#[derive(Clone)]
pub struct MessageRef<'a> {
    msg: NonNull<ffi::RawMessage>,
    def: Message,
    _life: PhantomData<&'a DynamicMessage>,
}

impl fmt::Debug for MessageRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MessageRef({})", self.def.full_name())
    }
}

impl<'a> MessageRef<'a> {
    pub fn descriptor(&self) -> &Message {
        &self.def
    }

    /// Whether the field is set: for a field with presence, as upb tracks
    /// it; for a list or a map, not empty; for a field without presence,
    /// not its zero value. The fields [`set_fields`](Self::set_fields)
    /// lists.
    pub fn has(&self, field: &Field) -> bool {
        check_field(&self.def, field);
        if field.has_presence() {
            // SAFETY: a field of this message, with presence.
            return unsafe { ffi::upb_Message_HasFieldByDef(self.msg.as_ptr(), field.raw()) };
        }
        match self.get(field) {
            Value::List(l) => !l.is_empty(),
            Value::Map(m) => !m.is_empty(),
            Value::Bool(v) => v,
            Value::I32(v) | Value::Enum(v) => v != 0,
            Value::I64(v) => v != 0,
            Value::U32(v) => v != 0,
            Value::U64(v) => v != 0,
            // As upb's own test: the bits, so that -0.0 counts as set.
            Value::F32(v) => v.to_bits() != 0,
            Value::F64(v) => v.to_bits() != 0,
            Value::Bytes(b) => !b.is_empty(),
            Value::Message(m) => m.is_some(),
        }
    }

    /// The field's value: its default when it is not set, `None` for an
    /// unset message, an empty list or map for an empty repeated field.
    pub fn get(&self, field: &Field) -> Value<'a> {
        check_field(&self.def, field);
        // SAFETY: a field of this message.
        let raw = unsafe { ffi::upb_Message_GetFieldByDef(self.msg.as_ptr(), field.raw()) };
        // SAFETY: the member each branch reads is the one upb filled for
        // this kind of field, and lives in the message's arena.
        unsafe { self.value(field, raw) }
    }

    /// Interpret a value upb returned for `field`.
    ///
    /// # Safety
    /// `raw` is the value of `field`, in this message.
    unsafe fn value(&self, field: &Field, raw: ffi::MessageValue) -> Value<'a> {
        if field.is_map() {
            let (key, value) = map_entry_fields(field);
            // SAFETY: a map field's value is its map, or null when empty.
            Value::Map(MapRef {
                map: unsafe { raw.map_val },
                key,
                value,
                _life: PhantomData,
            })
        } else if field.cardinality() == Cardinality::Repeated {
            Value::List(List {
                // SAFETY: a list field's value is its array, or null.
                arr: unsafe { raw.array_val },
                kind: field.kind(),
                _life: PhantomData,
            })
        } else {
            // SAFETY: a singular field's value has the field's type.
            unsafe { read_value(raw, &field.kind()) }
        }
    }

    /// The fields that are set, with their values, as Python's
    /// `ListFields()` lists them: the message's fields in field-number
    /// order, then its extensions known to the pool. A field without
    /// presence is listed when it is not its zero value.
    pub fn set_fields(&self) -> Vec<(Field, Value<'a>)> {
        let mut out = Vec::new();
        let _state = self.def.pool.read();
        let mut iter = ffi::ITER_BEGIN;
        loop {
            let mut f: *const ffi::FieldDef = std::ptr::null();
            let mut raw = ffi::MessageValue { uint64_val: 0 };
            // SAFETY: a valid message of this type and its pool, under the
            // pool's read lock; upb writes `f` and `raw` when it returns
            // true.
            let more = unsafe {
                ffi::upb_Message_Next(
                    self.msg.as_ptr(),
                    self.def.raw(),
                    self.def.pool.raw.as_ptr(),
                    &mut f,
                    &mut raw,
                    &mut iter,
                )
            };
            if !more {
                break;
            }
            let Some(field) = self.def.wrap(f).map(Field::from) else {
                continue;
            };
            // SAFETY: `raw` is `field`'s value in this message.
            let value = unsafe { self.value(&field, raw) };
            out.push((field, value));
        }
        out
    }

    /// The field with this number in the message's type, if that version
    /// of the type has one.
    pub fn field_by_number(&self, number: u32) -> Option<Field> {
        self.def.get_field(number)
    }

    /// [`get`](Self::get) by field number: `None` when the message's type
    /// has no such field (spec 0424 S6).
    pub fn get_by_number(&self, number: u32) -> Option<Value<'a>> {
        self.field_by_number(number).map(|f| self.get(&f))
    }

    /// [`has`](Self::has) by field number: false when the message's type
    /// has no such field.
    pub fn has_by_number(&self, number: u32) -> bool {
        self.field_by_number(number).is_some_and(|f| self.has(&f))
    }

    /// The message serialized: its fields in field-number order, then its
    /// extensions, then its unknown fields as they were decoded.
    pub fn encode(&self) -> Result<Vec<u8>, MessageError> {
        let arena = Arena::new();
        let mut buf: *mut std::ffi::c_char = std::ptr::null_mut();
        let mut size = 0usize;
        // SAFETY: a valid message of this table; the output is allocated
        // in `arena`, alive until the copy below is made.
        let status = unsafe {
            ffi::upb_Encode(
                self.msg.as_ptr(),
                mini_table(&self.def),
                0,
                arena.raw(),
                &mut buf,
                &mut size,
            )
        };
        if status != ffi::STATUS_OK {
            return Err(MessageError(format!("upb_Encode status {status}")));
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: upb wrote `size` bytes at `buf`.
        Ok(unsafe { std::slice::from_raw_parts(buf.cast::<u8>(), size) }.to_vec())
    }
}

/// The elements of a repeated field that is not a map.
#[derive(Clone)]
pub struct List<'a> {
    arr: *const ffi::Array,
    kind: Kind,
    _life: PhantomData<&'a DynamicMessage>,
}

impl fmt::Debug for List<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a> List<'a> {
    pub fn len(&self) -> usize {
        if self.arr.is_null() {
            0
        } else {
            // SAFETY: a valid array.
            unsafe { ffi::upb_Array_Size(self.arr) }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The element at `i`; panics when `i` is out of bounds.
    pub fn get(&self, i: usize) -> Value<'a> {
        assert!(
            i < self.len(),
            "index {i} out of bounds of a list of {}",
            self.len()
        );
        // SAFETY: a valid array; `i` is within it; its elements are of
        // the field's type.
        unsafe { read_value(ffi::upb_Array_Get(self.arr, i), &self.kind) }
    }

    pub fn iter(&self) -> impl Iterator<Item = Value<'a>> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }
}

/// The entries of a map field. upb iterates them in its hash table's
/// order, which is not the order they were decoded in.
#[derive(Clone)]
pub struct MapRef<'a> {
    map: *const ffi::Map,
    key: Field,
    value: Field,
    _life: PhantomData<&'a DynamicMessage>,
}

impl fmt::Debug for MapRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.entries()).finish()
    }
}

impl<'a> MapRef<'a> {
    pub fn len(&self) -> usize {
        if self.map.is_null() {
            0
        } else {
            // SAFETY: a valid map.
            unsafe { ffi::upb_Map_Size(self.map) }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `(key, value)` entries, in upb's order.
    pub fn entries(&self) -> Vec<(Value<'a>, Value<'a>)> {
        let mut out = Vec::with_capacity(self.len());
        if self.map.is_null() {
            return out;
        }
        let (key_kind, value_kind) = (self.key.kind(), self.value.kind());
        let mut iter = ffi::ITER_BEGIN;
        loop {
            let mut k = ffi::MessageValue { uint64_val: 0 };
            let mut v = ffi::MessageValue { uint64_val: 0 };
            // SAFETY: a valid map; upb writes `k` and `v` when it returns
            // true.
            if !unsafe { ffi::upb_Map_Next(self.map, &mut k, &mut v, &mut iter) } {
                break;
            }
            // SAFETY: the key and value have the entry fields' types.
            out.push(unsafe { (read_value(k, &key_kind), read_value(v, &value_kind)) });
        }
        out
    }
}

// ── Writing ─────────────────────────────────────────────────────────────────

/// A message to write: a [`DynamicMessage`] or one of its sub-messages.
pub struct MessageMut<'a> {
    msg: NonNull<ffi::RawMessage>,
    def: Message,
    arena: *mut ffi::Arena,
    _life: PhantomData<&'a mut DynamicMessage>,
}

impl fmt::Debug for MessageMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MessageMut({})", self.def.full_name())
    }
}

impl MessageMut<'_> {
    pub fn descriptor(&self) -> &Message {
        &self.def
    }

    pub fn as_ref(&self) -> MessageRef<'_> {
        MessageRef {
            msg: self.msg,
            def: self.def.clone(),
            _life: PhantomData,
        }
    }

    /// Set a singular scalar field.
    pub fn set(&mut self, field: &Field, value: Scalar<'_>) -> Result<(), MessageError> {
        check_field(&self.def, field);
        if field.cardinality() == Cardinality::Repeated {
            return Err(MessageError(format!("{} is repeated", field.full_name())));
        }
        let raw = write_value(&field.kind(), &value, self.arena)?;
        // SAFETY: a field of this message; `raw` has its type and lives in
        // the message's arena.
        if unsafe {
            ffi::upb_Message_SetFieldByDef(self.msg.as_ptr(), field.raw(), raw, self.arena)
        } {
            Ok(())
        } else {
            Err(MessageError("out of memory".into()))
        }
    }

    /// Clear a field: unset, empty, or zero.
    pub fn clear(&mut self, field: &Field) {
        check_field(&self.def, field);
        // SAFETY: a field of this message.
        unsafe { ffi::upb_Message_ClearFieldByDef(self.msg.as_ptr(), field.raw()) }
    }

    /// A singular message field, created when it is not set.
    pub fn message_mut(&mut self, field: &Field) -> Result<MessageMut<'_>, MessageError> {
        check_field(&self.def, field);
        let Kind::Message(sub) = field.kind() else {
            return Err(MessageError(format!(
                "{} is not a message",
                field.full_name()
            )));
        };
        if field.cardinality() == Cardinality::Repeated {
            return Err(MessageError(format!("{} is repeated", field.full_name())));
        }
        // SAFETY: a singular message field of this message.
        let value = unsafe { ffi::upb_Message_Mutable(self.msg.as_ptr(), field.raw(), self.arena) };
        // SAFETY: for a message field, upb returns the sub-message.
        let msg = NonNull::new(unsafe { value.msg })
            .ok_or_else(|| MessageError("out of memory".into()))?;
        Ok(MessageMut {
            msg,
            def: sub,
            arena: self.arena,
            _life: PhantomData,
        })
    }

    /// The array of a list field, created when it is empty.
    fn array(&mut self, field: &Field) -> Result<*mut ffi::Array, MessageError> {
        check_field(&self.def, field);
        if !field.is_list() {
            return Err(MessageError(format!("{} is not a list", field.full_name())));
        }
        // SAFETY: a list field of this message.
        let value = unsafe { ffi::upb_Message_Mutable(self.msg.as_ptr(), field.raw(), self.arena) };
        // SAFETY: for a list field, upb returns its array.
        let arr = unsafe { value.array };
        if arr.is_null() {
            return Err(MessageError("out of memory".into()));
        }
        Ok(arr)
    }

    /// Append a scalar to a list field.
    pub fn push(&mut self, field: &Field, value: Scalar<'_>) -> Result<(), MessageError> {
        let arr = self.array(field)?;
        let raw = write_value(&field.kind(), &value, self.arena)?;
        // SAFETY: a valid array of the field's type; `raw` has that type.
        if unsafe { ffi::upb_Array_Append(arr, raw, self.arena) } {
            Ok(())
        } else {
            Err(MessageError("out of memory".into()))
        }
    }

    /// Append an empty message to a list of messages, and return it.
    pub fn push_message(&mut self, field: &Field) -> Result<MessageMut<'_>, MessageError> {
        let Kind::Message(sub) = field.kind() else {
            return Err(MessageError(format!(
                "{} is not a message",
                field.full_name()
            )));
        };
        let arr = self.array(field)?;
        // SAFETY: a valid table and arena.
        let msg = NonNull::new(unsafe { ffi::upb_Message_New(mini_table(&sub), self.arena) })
            .ok_or_else(|| MessageError("out of memory".into()))?;
        let raw = ffi::MessageValue {
            msg_val: msg.as_ptr(),
        };
        // SAFETY: an array of messages of type `sub`; the message lives in
        // the same arena.
        if !unsafe { ffi::upb_Array_Append(arr, raw, self.arena) } {
            return Err(MessageError("out of memory".into()));
        }
        Ok(MessageMut {
            msg,
            def: sub,
            arena: self.arena,
            _life: PhantomData,
        })
    }

    /// The field with this number in the message's type, or an error
    /// naming it when that version of the type has none.
    fn field_or_err(&self, number: u32) -> Result<Field, MessageError> {
        self.def
            .get_field(number)
            .ok_or_else(|| MessageError(format!("{} has no field {number}", self.def.full_name())))
    }

    /// [`set`](Self::set) by field number.
    pub fn set_by_number(&mut self, number: u32, value: Scalar<'_>) -> Result<(), MessageError> {
        let f = self.field_or_err(number)?;
        self.set(&f, value)
    }

    /// [`clear`](Self::clear) by field number; nothing to do when the
    /// message's type has no such field.
    pub fn clear_by_number(&mut self, number: u32) {
        if let Some(f) = self.def.get_field(number) {
            self.clear(&f);
        }
    }

    /// [`message_mut`](Self::message_mut) by field number.
    pub fn message_mut_by_number(&mut self, number: u32) -> Result<MessageMut<'_>, MessageError> {
        let f = self.field_or_err(number)?;
        self.message_mut(&f)
    }

    /// [`push`](Self::push) by field number.
    pub fn push_by_number(&mut self, number: u32, value: Scalar<'_>) -> Result<(), MessageError> {
        let f = self.field_or_err(number)?;
        self.push(&f, value)
    }

    /// [`push_message`](Self::push_message) by field number.
    pub fn push_message_by_number(&mut self, number: u32) -> Result<MessageMut<'_>, MessageError> {
        let f = self.field_or_err(number)?;
        self.push_message(&f)
    }

    /// Set the entry `key` of a map field whose values are scalars.
    pub fn insert(
        &mut self,
        field: &Field,
        key: Scalar<'_>,
        value: Scalar<'_>,
    ) -> Result<(), MessageError> {
        check_field(&self.def, field);
        if !field.is_map() {
            return Err(MessageError(format!("{} is not a map", field.full_name())));
        }
        let (key_field, value_field) = map_entry_fields(field);
        let raw_key = write_value(&key_field.kind(), &key, self.arena)?;
        let raw_value = write_value(&value_field.kind(), &value, self.arena)?;
        // SAFETY: a map field of this message.
        let map =
            unsafe { ffi::upb_Message_Mutable(self.msg.as_ptr(), field.raw(), self.arena).map };
        if map.is_null() {
            return Err(MessageError("out of memory".into()));
        }
        // SAFETY: a valid map; the key and value have its entry's types.
        if unsafe { ffi::upb_Map_Set(map, raw_key, raw_value, self.arena) } {
            Ok(())
        } else {
            Err(MessageError("out of memory".into()))
        }
    }
}
