// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! A safe wrapper over upb's descriptor pool (spec 0415).
//!
//! upb, Google's C protobuf runtime, resolves editions features the way
//! every Google runtime does; this crate vendors its single-file build and
//! exposes the part of it a schema-driven decoder needs: loading
//! `FileDescriptorProto`s, finding definitions, and asking a field its
//! feature-resolved properties (packing, presence, delimited encoding).
//!
//! **Handles are owned** (S2). [`Pool`], [`File`], [`Message`], [`Field`],
//! [`Oneof`], [`Enum`] and [`EnumValue`] each hold an `Arc` to the pool, so
//! they keep it alive, are cheap to clone, and are `Send` and `Sync`.
//!
//! **Concurrency** (S2). upb definitions are immutable once built, and
//! adding a file never moves an existing one, so queries on a handle need
//! no lock. The pool's own symbol tables do change when a file is added:
//! [`Pool::add_file`] takes a write lock, and every lookup by name or
//! number on the pool a read lock.

pub mod descriptor;
mod ffi;
mod set;

use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ptr::NonNull;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

// ── Strings ─────────────────────────────────────────────────────────────────

/// A string upb returned, borrowed for as long as `_owner` (a handle, which
/// keeps the pool and so the string alive).
///
/// Not re-validated as UTF-8 on each call: the renderer asks for names once
/// per line, and validating them was 6.5% of a 25.6 MB decode (spec 0415
/// S12). It cannot fail, for two reasons this crate relies on:
///
/// - every identifier upb builds — message, field, enum, value and package
///   names, and the full names made of them — is checked by upb to be
///   `[A-Za-z_][A-Za-z0-9_]*` joined by dots before it is stored
///   (`_upb_DefBuilder_CheckIdentSlow`), so it is ASCII;
/// - the one name upb does not check, a file's, is checked to be UTF-8
///   before the file reaches upb ([`set::name_and_imports`]).
///
/// `json_name`, which upb stores unchecked, never comes through here.
fn upb_str<T: ?Sized>(_owner: &T, p: *const c_char) -> &str {
    if p.is_null() {
        return "";
    }
    // SAFETY: `p` is a NUL-terminated string in the pool's arena, which
    // `_owner` keeps alive for the returned lifetime.
    let bytes = unsafe { CStr::from_ptr(p) }.to_bytes();
    debug_assert!(std::str::from_utf8(bytes).is_ok(), "{bytes:?}");
    // SAFETY: UTF-8 by the two invariants above.
    unsafe { std::str::from_utf8_unchecked(bytes) }
}

// ── Pool ────────────────────────────────────────────────────────────────────

/// Decides whether a serialized `FileDescriptorProto` may reach upb's
/// parser (spec 0415 S4). `Err` carries the reason it may not.
pub type Gate = fn(&[u8]) -> Result<(), String>;

struct PoolState {
    /// Files in the order they were added.
    files: Vec<NonNull<ffi::FileDef>>,
    /// Each loaded file's original bytes (S3): upb does not keep
    /// `source_code_info`, and has no def-to-proto conversion.
    bytes: HashMap<String, Arc<[u8]>>,
}

struct PoolInner {
    raw: NonNull<ffi::DefPool>,
    gate: Option<Gate>,
    state: RwLock<PoolState>,
}

// SAFETY: the pool is only mutated under `state`'s write lock, and read
// under its read lock (lookups) or not at all (queries on immutable
// definitions). upb keeps no thread-local state.
unsafe impl Send for PoolInner {}
unsafe impl Sync for PoolInner {}

impl Drop for PoolInner {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `upb_DefPool_New` and is freed once, when
        // the last handle into the pool is gone.
        unsafe { ffi::upb_DefPool_Free(self.raw.as_ptr()) }
    }
}

impl PoolInner {
    fn read(&self) -> RwLockReadGuard<'_, PoolState> {
        // A panic while the lock is held leaves the state consistent: the
        // only writer pushes after upb has succeeded.
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, PoolState> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// A descriptor pool: the definitions of the files added to it.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<PoolInner>,
}

/// Why a file was not added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError(pub String);

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

/// What [`Pool::load_set`] loaded and what it skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Files added, in the order they were added.
    pub loaded: Vec<String>,
    /// `(file, reason)` for each file not added. A file with no readable
    /// name appears as `#<index in the set>`.
    pub skipped: Vec<(String, String)>,
    /// Set when the input is not a `FileDescriptorSet` at all; nothing was
    /// loaded then.
    pub set_error: Option<String>,
}

impl Default for Pool {
    fn default() -> Self {
        Pool::new()
    }
}

impl Pool {
    /// An empty pool with no gate.
    pub fn new() -> Pool {
        Pool::build(None)
    }

    /// An empty pool whose every [`add_file`](Pool::add_file) first passes
    /// the bytes through `gate` (spec 0415 S4).
    pub fn with_gate(gate: Gate) -> Pool {
        Pool::build(Some(gate))
    }

    fn build(gate: Option<Gate>) -> Pool {
        // SAFETY: no precondition. It returns null only when out of memory.
        let raw = NonNull::new(unsafe { ffi::upb_DefPool_New() })
            .expect("upb_DefPool_New: out of memory");
        Pool {
            inner: Arc::new(PoolInner {
                raw,
                gate,
                state: RwLock::new(PoolState {
                    files: Vec::new(),
                    bytes: HashMap::new(),
                }),
            }),
        }
    }

    fn raw(&self) -> *mut ffi::DefPool {
        self.inner.raw.as_ptr()
    }

    /// Add one serialized `FileDescriptorProto`. Its imports must already
    /// be in the pool. A failed add leaves the pool unchanged.
    pub fn add_file(&self, bytes: &[u8]) -> Result<File, LoadError> {
        if let Some(gate) = self.inner.gate {
            gate(bytes).map_err(LoadError)?;
        }
        let (name, _) = set::name_and_imports(bytes).map_err(LoadError)?;
        let mut state = self.inner.write();
        let mut err = [0 as c_char; 512];
        // SAFETY: the pool is exclusively ours under the write lock; `bytes`
        // and `err` are valid for the given lengths; upb copies what it keeps.
        let file = unsafe {
            ffi::prototext_upb_add_file(
                self.raw(),
                bytes.as_ptr().cast(),
                bytes.len(),
                err.as_mut_ptr(),
                err.len(),
            )
        };
        let Some(file) = NonNull::new(file.cast_mut()) else {
            // SAFETY: the shim always NUL-terminates `err`.
            let msg = unsafe { CStr::from_ptr(err.as_ptr()) };
            return Err(LoadError(msg.to_string_lossy().into_owned()));
        };
        state.files.push(file);
        state.bytes.insert(name, Arc::from(bytes));
        Ok(File {
            pool: self.inner.clone(),
            ptr: file,
        })
    }

    /// Load every file of a serialized `FileDescriptorSet` that can be
    /// loaded, imports first (spec 0415 S3). Never fails as a whole: what
    /// cannot be loaded is in the report.
    pub fn load_set(&self, bytes: &[u8]) -> LoadReport {
        let mut report = LoadReport::default();
        let files = match set::split_set(bytes) {
            Ok(files) => files,
            Err(e) => {
                report.set_error = Some(format!("not a FileDescriptorSet: {e}"));
                return report;
            }
        };

        // Read names and imports; drop unreadable files and duplicates.
        let mut order: Vec<String> = Vec::new();
        let mut by_name: HashMap<String, (&[u8], Vec<String>)> = HashMap::new();
        for (i, fdp) in files.iter().enumerate() {
            match set::name_and_imports(fdp) {
                Err(e) => report.skipped.push((format!("#{i}"), e)),
                Ok((name, imports)) => {
                    if by_name.contains_key(&name) || self.file(&name).is_some() {
                        report
                            .skipped
                            .push((name, "duplicate file name".to_string()));
                    } else {
                        order.push(name.clone());
                        by_name.insert(name, (fdp, imports));
                    }
                }
            }
        }

        // Add each file after its imports: an iterative depth-first walk,
        // so a long chain of imports cannot exhaust the stack.
        #[derive(Clone, PartialEq)]
        enum Mark {
            Visiting,
            Done,
            Skipped(String),
        }
        let mut marks: HashMap<String, Mark> = HashMap::new();
        for root in &order {
            if marks.contains_key(root) {
                continue;
            }
            // Stack of (file, index of the next import to look at).
            let mut stack: Vec<(String, usize)> = vec![(root.clone(), 0)];
            marks.insert(root.clone(), Mark::Visiting);
            while let Some((file, next)) = stack.last().cloned() {
                let imports = &by_name[&file].1;
                if next < imports.len() {
                    stack.last_mut().unwrap().1 += 1;
                    let dep = &imports[next];
                    if by_name.contains_key(dep) {
                        match marks.get(dep) {
                            None => {
                                marks.insert(dep.clone(), Mark::Visiting);
                                stack.push((dep.clone(), 0));
                            }
                            Some(Mark::Visiting) => {
                                // A cycle: every file on it is skipped.
                                let start = stack.iter().position(|(f, _)| f == dep).unwrap_or(0);
                                for (f, _) in &stack[start..] {
                                    marks.insert(
                                        f.clone(),
                                        Mark::Skipped(format!(
                                            "on an import cycle through '{dep}'"
                                        )),
                                    );
                                }
                            }
                            Some(_) => {}
                        }
                    }
                    continue;
                }
                stack.pop();
                if let Some(Mark::Skipped(_)) = marks.get(&file) {
                    continue;
                }
                let verdict = match imports.iter().find_map(|dep| {
                    if self.file(dep).is_some() {
                        None
                    } else if !by_name.contains_key(dep) {
                        Some(format!("import '{dep}' is missing"))
                    } else if let Some(Mark::Skipped(_)) = marks.get(dep) {
                        Some(format!("import '{dep}' was skipped"))
                    } else {
                        None
                    }
                }) {
                    Some(reason) => Mark::Skipped(reason),
                    None => match self.add_file(by_name[&file].0) {
                        Ok(_) => Mark::Done,
                        Err(e) => Mark::Skipped(e.0),
                    },
                };
                marks.insert(file, verdict);
            }
        }

        // Report in the set's order.
        for name in order {
            match marks.get(&name) {
                Some(Mark::Done) => report.loaded.push(name),
                Some(Mark::Skipped(reason)) => report.skipped.push((name, reason.clone())),
                _ => report.skipped.push((name, "not loaded".to_string())),
            }
        }
        report
    }

    /// A file by name.
    pub fn file(&self, name: &str) -> Option<File> {
        let _state = self.inner.read();
        // SAFETY: lookups run under the read lock; `name` is valid for its
        // length.
        let p = unsafe {
            ffi::upb_DefPool_FindFileByNameWithSize(self.raw(), name.as_ptr().cast(), name.len())
        };
        NonNull::new(p.cast_mut()).map(|ptr| File {
            pool: self.inner.clone(),
            ptr,
        })
    }

    /// The files, in the order they were added.
    pub fn files(&self) -> Vec<File> {
        let state = self.inner.read();
        state
            .files
            .iter()
            .map(|&ptr| File {
                pool: self.inner.clone(),
                ptr,
            })
            .collect()
    }

    /// The bytes a loaded file was added from.
    pub fn file_bytes(&self, name: &str) -> Option<Arc<[u8]>> {
        self.inner.read().bytes.get(name).cloned()
    }

    /// A message by fully-qualified name (no leading dot).
    pub fn get_message_by_name(&self, name: &str) -> Option<Message> {
        let _state = self.inner.read();
        // SAFETY: as in `file`.
        let p = unsafe {
            ffi::upb_DefPool_FindMessageByNameWithSize(self.raw(), name.as_ptr().cast(), name.len())
        };
        NonNull::new(p.cast_mut()).map(|ptr| Message {
            pool: self.inner.clone(),
            ptr,
        })
    }

    /// An enum by fully-qualified name (no leading dot).
    pub fn get_enum_by_name(&self, name: &str) -> Option<Enum> {
        let _state = self.inner.read();
        // SAFETY: as in `file`.
        let p = unsafe {
            ffi::upb_DefPool_FindEnumByNameWithSize(self.raw(), name.as_ptr().cast(), name.len())
        };
        NonNull::new(p.cast_mut()).map(|ptr| Enum {
            pool: self.inner.clone(),
            ptr,
        })
    }

    /// An extension by fully-qualified name (no leading dot).
    pub fn get_extension_by_name(&self, name: &str) -> Option<Field> {
        let _state = self.inner.read();
        // SAFETY: as in `file`.
        let p = unsafe {
            ffi::upb_DefPool_FindExtensionByNameWithSize(
                self.raw(),
                name.as_ptr().cast(),
                name.len(),
            )
        };
        NonNull::new(p.cast_mut()).map(|ptr| Field {
            pool: self.inner.clone(),
            ptr,
        })
    }

    /// Every message in the pool, nested ones included: files in the order
    /// they were added, each file's messages depth-first in declaration
    /// order.
    pub fn all_messages(&self) -> Vec<Message> {
        let mut out = Vec::new();
        for file in self.files() {
            let mut stack: Vec<Message> = file.messages().collect();
            stack.reverse();
            while let Some(m) = stack.pop() {
                let mut nested: Vec<Message> = m.child_messages().collect();
                nested.reverse();
                stack.extend(nested);
                out.push(m);
            }
        }
        out
    }

    /// Every enum in the pool, nested ones included, in the same order as
    /// [`all_messages`](Pool::all_messages) visits their scopes.
    pub fn all_enums(&self) -> Vec<Enum> {
        let mut out = Vec::new();
        for file in self.files() {
            out.extend(file.enums());
            let mut stack: Vec<Message> = file.messages().collect();
            stack.reverse();
            while let Some(m) = stack.pop() {
                out.extend(m.child_enums());
                let mut nested: Vec<Message> = m.child_messages().collect();
                nested.reverse();
                stack.extend(nested);
            }
        }
        out
    }
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("files", &self.inner.read().files.len())
            .finish()
    }
}

// ── Handles ─────────────────────────────────────────────────────────────────

macro_rules! handle {
    ($(#[$doc:meta])* $name:ident, $raw:ty, $label:expr) => {
        $(#[$doc])*
        #[derive(Clone)]
        pub struct $name {
            pool: Arc<PoolInner>,
            ptr: NonNull<$raw>,
        }

        // SAFETY: a definition is immutable once built and lives as long as
        // the pool, which the handle keeps alive.
        unsafe impl Send for $name {}
        unsafe impl Sync for $name {}

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                self.ptr == other.ptr
            }
        }

        impl Eq for $name {}

        impl Hash for $name {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.ptr.hash(state);
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", $label, self.debug_name())
            }
        }

        impl $name {
            fn raw(&self) -> *const $raw {
                self.ptr.as_ptr()
            }

            /// Wrap a pointer upb returned, null meaning absent.
            fn wrap<T>(&self, p: *const T) -> Option<Handle<T>> {
                NonNull::new(p.cast_mut()).map(|ptr| Handle {
                    pool: self.pool.clone(),
                    ptr,
                })
            }
        }
    };
}

/// A handle under construction: the pool plus a pointer, converted into
/// the right handle type by `From`.
struct Handle<T> {
    pool: Arc<PoolInner>,
    ptr: NonNull<T>,
}

macro_rules! from_handle {
    ($name:ident, $raw:ty) => {
        impl From<Handle<$raw>> for $name {
            fn from(h: Handle<$raw>) -> Self {
                $name {
                    pool: h.pool,
                    ptr: h.ptr,
                }
            }
        }
    };
}

handle!(
    /// A `.proto` file.
    File, ffi::FileDef, "File"
);
handle!(
    /// A message type.
    Message, ffi::MessageDef, "Message"
);
handle!(
    /// A field or an extension.
    Field, ffi::FieldDef, "Field"
);
handle!(
    /// A oneof.
    Oneof, ffi::OneofDef, "Oneof"
);
handle!(
    /// An enum type.
    Enum, ffi::EnumDef, "Enum"
);
handle!(
    /// One value of an enum.
    EnumValue, ffi::EnumValueDef, "EnumValue"
);
from_handle!(File, ffi::FileDef);
from_handle!(Message, ffi::MessageDef);
from_handle!(Field, ffi::FieldDef);
from_handle!(Oneof, ffi::OneofDef);
from_handle!(Enum, ffi::EnumDef);
from_handle!(EnumValue, ffi::EnumValueDef);

// Field numbers in `descriptor.proto` that a declaration path steps through.
const FILE_MESSAGE_TYPE: i32 = 4;
const FILE_ENUM_TYPE: i32 = 5;
const MESSAGE_NESTED_TYPE: i32 = 3;
const MESSAGE_ENUM_TYPE: i32 = 4;

/// The position of `def` among `siblings`, which upb lists in declaration
/// order.
fn sibling_index<T: PartialEq>(mut siblings: impl Iterator<Item = T>, def: &T) -> i32 {
    let i = siblings
        .position(|s| s == *def)
        .expect("a definition is among its parent's children");
    i32::try_from(i).expect("upb counts children in an int")
}

/// Iterate `count` children of `$owner`, built by `$get(raw, i)`.
macro_rules! children {
    ($owner:expr, $count:path, $get:path) => {{
        let owner = $owner;
        // SAFETY: a valid definition; the index is within its count.
        let n = unsafe { $count(owner.raw()) }.max(0);
        (0..n).filter_map(move |i| owner.wrap(unsafe { $get(owner.raw(), i) }).map(Into::into))
    }};
}

/// A file's syntax, resolved by upb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    Proto2,
    Proto3,
    Edition2023,
    Edition2024,
    /// Any other value of `google.protobuf.Edition`.
    Other(i32),
}

impl File {
    fn debug_name(&self) -> String {
        self.name().to_string()
    }

    pub fn name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_FileDef_Name(self.raw()) })
    }

    pub fn package_name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_FileDef_Package(self.raw()) })
    }

    pub fn edition(&self) -> Edition {
        // SAFETY: a valid definition.
        match unsafe { ffi::upb_FileDef_Edition(self.raw()) } {
            998 => Edition::Proto2,
            999 => Edition::Proto3,
            1000 => Edition::Edition2023,
            1001 => Edition::Edition2024,
            other => Edition::Other(other),
        }
    }

    pub fn dependencies(&self) -> impl Iterator<Item = File> + '_ {
        children!(
            self,
            ffi::upb_FileDef_DependencyCount,
            ffi::upb_FileDef_Dependency
        )
    }

    /// Top-level messages, in declaration order.
    pub fn messages(&self) -> impl Iterator<Item = Message> + '_ {
        children!(
            self,
            ffi::upb_FileDef_TopLevelMessageCount,
            ffi::upb_FileDef_TopLevelMessage
        )
    }

    /// Top-level enums, in declaration order.
    pub fn enums(&self) -> impl Iterator<Item = Enum> + '_ {
        children!(
            self,
            ffi::upb_FileDef_TopLevelEnumCount,
            ffi::upb_FileDef_TopLevelEnum
        )
    }

    /// Top-level extensions, in declaration order.
    pub fn extensions(&self) -> impl Iterator<Item = Field> + '_ {
        children!(
            self,
            ffi::upb_FileDef_TopLevelExtensionCount,
            ffi::upb_FileDef_TopLevelExtension
        )
    }
}

impl Message {
    fn debug_name(&self) -> String {
        self.full_name().to_string()
    }

    pub fn full_name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_MessageDef_FullName(self.raw()) })
    }

    pub fn name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_MessageDef_Name(self.raw()) })
    }

    pub fn parent_file(&self) -> File {
        // SAFETY: a valid definition; every message has a file.
        let p = unsafe { ffi::upb_MessageDef_File(self.raw()) };
        self.wrap(p).map(Into::into).expect("a message has a file")
    }

    /// The message this one is nested in, if any.
    pub fn parent_message(&self) -> Option<Message> {
        // SAFETY: a valid definition.
        self.wrap(unsafe { ffi::upb_MessageDef_ContainingType(self.raw()) })
            .map(Into::into)
    }

    /// The message's `SourceCodeInfo.Location.path` in its file: `[4, i]`
    /// at top level, its parent's path then `[3, j]` when nested.
    pub fn declaration_path(&self) -> Vec<i32> {
        match self.parent_message() {
            None => vec![
                FILE_MESSAGE_TYPE,
                sibling_index(self.parent_file().messages(), self),
            ],
            Some(m) => {
                let mut path = m.declaration_path();
                path.extend([MESSAGE_NESTED_TYPE, sibling_index(m.child_messages(), self)]);
                path
            }
        }
    }

    /// Fields in declaration order (extensions excluded).
    pub fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        children!(
            self,
            ffi::upb_MessageDef_FieldCount,
            ffi::upb_MessageDef_Field
        )
    }

    pub fn get_field(&self, number: u32) -> Option<Field> {
        // SAFETY: a valid definition.
        self.wrap(unsafe { ffi::upb_MessageDef_FindFieldByNumber(self.raw(), number) })
            .map(Into::into)
    }

    pub fn get_field_by_name(&self, name: &str) -> Option<Field> {
        // SAFETY: a valid definition; `name` is valid for its length.
        self.wrap(unsafe {
            ffi::upb_MessageDef_FindFieldByNameWithSize(
                self.raw(),
                name.as_ptr().cast(),
                name.len(),
            )
        })
        .map(Into::into)
    }

    /// The extension of this message with field number `number`, among the
    /// files loaded so far.
    pub fn get_extension(&self, number: u32) -> Option<Field> {
        let number = i32::try_from(number).ok()?;
        let _state = self.pool.read();
        // SAFETY: a pool lookup, under the read lock.
        let p = unsafe {
            ffi::upb_DefPool_FindExtensionByNumber(self.pool.raw.as_ptr(), self.raw(), number)
        };
        self.wrap(p).map(Into::into)
    }

    /// Messages nested in this one, in declaration order.
    pub fn child_messages(&self) -> impl Iterator<Item = Message> + '_ {
        children!(
            self,
            ffi::upb_MessageDef_NestedMessageCount,
            ffi::upb_MessageDef_NestedMessage
        )
    }

    /// Enums nested in this one, in declaration order.
    pub fn child_enums(&self) -> impl Iterator<Item = Enum> + '_ {
        children!(
            self,
            ffi::upb_MessageDef_NestedEnumCount,
            ffi::upb_MessageDef_NestedEnum
        )
    }

    /// Extensions declared in this message's scope.
    pub fn child_extensions(&self) -> impl Iterator<Item = Field> + '_ {
        children!(
            self,
            ffi::upb_MessageDef_NestedExtensionCount,
            ffi::upb_MessageDef_NestedExtension
        )
    }

    pub fn oneofs(&self) -> impl Iterator<Item = Oneof> + '_ {
        children!(
            self,
            ffi::upb_MessageDef_OneofCount,
            ffi::upb_MessageDef_Oneof
        )
    }

    pub fn is_map_entry(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_MessageDef_IsMapEntry(self.raw()) }
    }

    /// `option message_set_wire_format = true`.
    pub fn is_message_set_wire_format(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::prototext_upb_message_set_wire_format(self.raw()) }
    }
}

/// A field's type. A group, or an editions field with delimited encoding,
/// is a `Message` whose field answers `is_group()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Double,
    Float,
    Int64,
    Uint64,
    Int32,
    Fixed64,
    Fixed32,
    Bool,
    String,
    Bytes,
    Uint32,
    Sfixed32,
    Sfixed64,
    Sint32,
    Sint64,
    Message(Message),
    Enum(Enum),
}

/// A field's label, resolved by upb (editions' `LEGACY_REQUIRED` is
/// `Required`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cardinality {
    Optional,
    Required,
    Repeated,
}

const TYPE_GROUP: ffi::FieldType = 10;

impl Field {
    fn debug_name(&self) -> String {
        self.full_name().to_string()
    }

    pub fn name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_FieldDef_Name(self.raw()) })
    }

    pub fn full_name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_FieldDef_FullName(self.raw()) })
    }

    pub fn number(&self) -> u32 {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_FieldDef_Number(self.raw()) }
    }

    pub fn kind(&self) -> Kind {
        // SAFETY: a valid definition.
        match unsafe { ffi::upb_FieldDef_Type(self.raw()) } {
            1 => Kind::Double,
            2 => Kind::Float,
            3 => Kind::Int64,
            4 => Kind::Uint64,
            5 => Kind::Int32,
            6 => Kind::Fixed64,
            7 => Kind::Fixed32,
            8 => Kind::Bool,
            9 => Kind::String,
            12 => Kind::Bytes,
            13 => Kind::Uint32,
            14 => {
                // SAFETY: a valid enum-typed field.
                let e = unsafe { ffi::upb_FieldDef_EnumSubDef(self.raw()) };
                Kind::Enum(
                    self.wrap(e)
                        .map(Into::into)
                        .expect("an enum field has its enum"),
                )
            }
            15 => Kind::Sfixed32,
            16 => Kind::Sfixed64,
            17 => Kind::Sint32,
            18 => Kind::Sint64,
            // 10 (group) and 11 (message), and nothing else: upb only
            // builds the eighteen types.
            _ => {
                // SAFETY: a valid message- or group-typed field.
                let m = unsafe { ffi::upb_FieldDef_MessageSubDef(self.raw()) };
                Kind::Message(
                    self.wrap(m)
                        .map(Into::into)
                        .expect("a message field has its type"),
                )
            }
        }
    }

    pub fn cardinality(&self) -> Cardinality {
        // SAFETY: a valid definition.
        unsafe {
            if ffi::upb_FieldDef_IsRepeated(self.raw()) {
                Cardinality::Repeated
            } else if ffi::upb_FieldDef_IsRequired(self.raw()) {
                Cardinality::Required
            } else {
                Cardinality::Optional
            }
        }
    }

    /// A proto2 group, or an editions field with delimited encoding: its
    /// value is framed by group tags, not a length.
    pub fn is_group(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_FieldDef_Type(self.raw()) == TYPE_GROUP }
    }

    pub fn is_list(&self) -> bool {
        self.cardinality() == Cardinality::Repeated && !self.is_map()
    }

    pub fn is_map(&self) -> bool {
        self.cardinality() == Cardinality::Repeated
            && matches!(self.kind(), Kind::Message(m) if m.is_map_entry())
    }

    /// Whether the schema declares this field packed, features resolved.
    pub fn is_packed(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_FieldDef_IsPacked(self.raw()) }
    }

    /// Whether the field tracks presence, features resolved.
    pub fn has_presence(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_FieldDef_HasPresence(self.raw()) }
    }

    pub fn is_extension(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_FieldDef_IsExtension(self.raw()) }
    }

    /// The message this field belongs to; for an extension, the message it
    /// extends.
    pub fn containing_message(&self) -> Message {
        // SAFETY: a valid definition; every field has one.
        let p = unsafe { ffi::upb_FieldDef_ContainingType(self.raw()) };
        self.wrap(p)
            .map(Into::into)
            .expect("a field has a containing message")
    }

    /// For an extension declared inside a message, that message.
    /// The file that declares the field (for an extension, the file of
    /// the `extend` block, not the extendee's).
    pub fn parent_file(&self) -> File {
        // SAFETY: a valid definition; every field has a file.
        let p = unsafe { ffi::upb_FieldDef_File(self.raw()) };
        self.wrap(p).map(Into::into).expect("a field has a file")
    }

    /// protoc's `IsGroupLike`: a group-framed field written the way a
    /// proto2 `group` declares one — named as its type lowercased, the type
    /// declared in the same file and the same scope as the field. Text
    /// format names such a field by its type (`MyGroup { … }`) and any
    /// other by its own name. Every proto2 group is group-like; an editions
    /// `DELIMITED` field is only if it was written to look like one.
    pub fn is_group_like(&self) -> bool {
        let Kind::Message(m) = self.kind() else {
            return false;
        };
        let scope = if self.is_extension() {
            self.extension_scope()
        } else {
            Some(self.containing_message())
        };
        self.is_group()
            && self.name() == m.name().to_ascii_lowercase()
            && m.parent_file() == self.parent_file()
            && m.parent_message() == scope
    }

    pub fn extension_scope(&self) -> Option<Message> {
        // SAFETY: a valid definition.
        self.wrap(unsafe { ffi::upb_FieldDef_ExtensionScope(self.raw()) })
            .map(Into::into)
    }

    pub fn containing_oneof(&self) -> Option<Oneof> {
        // SAFETY: a valid definition.
        self.wrap(unsafe { ffi::upb_FieldDef_ContainingOneof(self.raw()) })
            .map(Into::into)
    }

    pub fn json_name(&self) -> String {
        // json_name is the one string upb stores without checking it, so
        // it is converted lossily rather than borrowed.
        // SAFETY: a valid definition; the string lives in the pool.
        let p = unsafe { ffi::upb_FieldDef_JsonName(self.raw()) };
        if p.is_null() {
            return String::new();
        }
        // SAFETY: NUL-terminated, kept alive by `self`.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

impl Oneof {
    fn debug_name(&self) -> String {
        self.name().to_string()
    }

    pub fn name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_OneofDef_Name(self.raw()) })
    }

    /// The oneof protoc synthesizes for a proto3 `optional` field.
    pub fn is_synthetic(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_OneofDef_IsSynthetic(self.raw()) }
    }

    pub fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        children!(self, ffi::upb_OneofDef_FieldCount, ffi::upb_OneofDef_Field)
    }
}

impl Enum {
    fn debug_name(&self) -> String {
        self.full_name().to_string()
    }

    pub fn full_name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_EnumDef_FullName(self.raw()) })
    }

    pub fn name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_EnumDef_Name(self.raw()) })
    }

    pub fn parent_file(&self) -> File {
        // SAFETY: a valid definition; every enum has a file.
        let p = unsafe { ffi::upb_EnumDef_File(self.raw()) };
        self.wrap(p).map(Into::into).expect("an enum has a file")
    }

    /// The message this enum is nested in, if any.
    pub fn parent_message(&self) -> Option<Message> {
        // SAFETY: a valid definition.
        self.wrap(unsafe { ffi::upb_EnumDef_ContainingType(self.raw()) })
            .map(Into::into)
    }

    /// The enum's `SourceCodeInfo.Location.path` in its file: `[5, i]` at
    /// top level, its message's path then `[4, k]` when nested.
    pub fn declaration_path(&self) -> Vec<i32> {
        match self.parent_message() {
            None => vec![
                FILE_ENUM_TYPE,
                sibling_index(self.parent_file().enums(), self),
            ],
            Some(m) => {
                let mut path = m.declaration_path();
                path.extend([MESSAGE_ENUM_TYPE, sibling_index(m.child_enums(), self)]);
                path
            }
        }
    }

    /// A closed enum (proto2, or editions' `CLOSED`) rejects unknown values.
    pub fn is_closed(&self) -> bool {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_EnumDef_IsClosed(self.raw()) }
    }

    /// Values in declaration order.
    pub fn values(&self) -> impl Iterator<Item = EnumValue> + '_ {
        children!(self, ffi::upb_EnumDef_ValueCount, ffi::upb_EnumDef_Value)
    }

    /// The value with this number; with aliases, the first declared.
    pub fn get_value(&self, number: i32) -> Option<EnumValue> {
        // SAFETY: a valid definition.
        self.wrap(unsafe { ffi::upb_EnumDef_FindValueByNumber(self.raw(), number) })
            .map(Into::into)
    }
}

impl EnumValue {
    fn debug_name(&self) -> String {
        self.full_name().to_string()
    }

    /// The enum this value belongs to.
    pub fn parent_enum(&self) -> Enum {
        // SAFETY: a valid definition; every value has its enum.
        let p = unsafe { ffi::upb_EnumValueDef_Enum(self.raw()) };
        self.wrap(p).map(Into::into).expect("a value has its enum")
    }

    /// The value's name in its enum's scope: an enum's values are siblings
    /// of the enum, so `pkg.Color.RED` is `pkg.RED`.
    pub fn full_name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_EnumValueDef_FullName(self.raw()) })
    }

    pub fn name(&self) -> &str {
        // SAFETY: a valid definition.
        upb_str(self, unsafe { ffi::upb_EnumValueDef_Name(self.raw()) })
    }

    pub fn number(&self) -> i32 {
        // SAFETY: a valid definition.
        unsafe { ffi::upb_EnumValueDef_Number(self.raw()) }
    }
}
