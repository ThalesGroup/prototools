// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Hand-written declarations of the upb functions this crate calls (spec
//! 0415 S2). Only public functions, never `_upb_…` or `UPB_PRIVATE`
//! symbols; each prototype is copied from the vendored `upb.h`. The
//! `every_declaration_links` test takes the address of each one, so an
//! upgrade that turns one inline fails the build of the test.

use std::ffi::{c_char, c_int, c_void};

macro_rules! opaque {
    ($($name:ident),* $(,)?) => {
        $(
            #[repr(C)]
            pub struct $name {
                _private: [u8; 0],
            }
        )*
    };
}

opaque!(
    DefPool,
    FileDef,
    MessageDef,
    FieldDef,
    OneofDef,
    EnumDef,
    EnumValueDef,
    Arena,
    RawMessage,
    MiniTable,
    ExtensionRegistry,
    Array,
    Map,
    ServiceDef,
    MethodDef,
    ExtensionRange,
);

/// C `upb_FieldType`: the values of `FieldDescriptorProto.Type`.
pub type FieldType = c_int;
/// C `google_protobuf_Edition`.
pub type EditionNumber = c_int;

/// C `upb_StringView`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StringView {
    pub data: *const c_char,
    pub size: usize,
}

/// C `upb_MessageValue`: one value of any field type. upb's own union also
/// has a `struct upb_Message` member, private, of the size of a pointer,
/// which changes neither its size nor its alignment (checked below and in
/// the shim).
#[repr(C)]
#[derive(Clone, Copy)]
pub union MessageValue {
    pub bool_val: bool,
    pub float_val: f32,
    pub double_val: f64,
    pub int32_val: i32,
    pub int64_val: i64,
    pub uint32_val: u32,
    pub uint64_val: u64,
    pub array_val: *const Array,
    pub map_val: *const Map,
    pub msg_val: *const RawMessage,
    pub str_val: StringView,
}

const _: () = assert!(std::mem::size_of::<MessageValue>() == 16);
const _: () = assert!(std::mem::align_of::<MessageValue>() == 8);

/// C `upb_MutableMessageValue`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union MutableMessageValue {
    pub array: *mut Array,
    pub map: *mut Map,
    pub msg: *mut RawMessage,
}

/// `kUpb_Message_Begin` and `kUpb_Map_Begin`: `(size_t)-1`.
pub const ITER_BEGIN: usize = usize::MAX;
/// `kUpb_DecodeStatus_Ok`, `kUpb_EncodeStatus_Ok`.
pub const STATUS_OK: c_int = 0;

extern "C" {
    // ── Pool ──────────────────────────────────────────────────────────────
    pub fn upb_DefPool_New() -> *mut DefPool;
    pub fn upb_DefPool_Free(s: *mut DefPool);
    pub fn upb_DefPool_FindMessageByNameWithSize(
        s: *const DefPool,
        sym: *const c_char,
        len: usize,
    ) -> *const MessageDef;
    pub fn upb_DefPool_FindEnumByNameWithSize(
        s: *const DefPool,
        sym: *const c_char,
        len: usize,
    ) -> *const EnumDef;
    pub fn upb_DefPool_FindFileByNameWithSize(
        s: *const DefPool,
        name: *const c_char,
        len: usize,
    ) -> *const FileDef;
    pub fn upb_DefPool_FindExtensionByNumber(
        s: *const DefPool,
        m: *const MessageDef,
        fieldnum: i32,
    ) -> *const FieldDef;
    pub fn upb_DefPool_FindExtensionByNameWithSize(
        s: *const DefPool,
        name: *const c_char,
        size: usize,
    ) -> *const FieldDef;
    pub fn upb_DefPool_FindServiceByNameWithSize(
        s: *const DefPool,
        name: *const c_char,
        size: usize,
    ) -> *const ServiceDef;
    pub fn upb_DefPool_GetAllExtensions(
        s: *const DefPool,
        m: *const MessageDef,
        count: *mut usize,
    ) -> *mut *const FieldDef;

    // ── File ──────────────────────────────────────────────────────────────
    pub fn upb_FileDef_Name(f: *const FileDef) -> *const c_char;
    pub fn upb_FileDef_Package(f: *const FileDef) -> *const c_char;
    pub fn upb_FileDef_Edition(f: *const FileDef) -> EditionNumber;
    pub fn upb_FileDef_DependencyCount(f: *const FileDef) -> c_int;
    pub fn upb_FileDef_Dependency(f: *const FileDef, i: c_int) -> *const FileDef;
    pub fn upb_FileDef_TopLevelMessageCount(f: *const FileDef) -> c_int;
    pub fn upb_FileDef_TopLevelMessage(f: *const FileDef, i: c_int) -> *const MessageDef;
    pub fn upb_FileDef_TopLevelEnumCount(f: *const FileDef) -> c_int;
    pub fn upb_FileDef_TopLevelEnum(f: *const FileDef, i: c_int) -> *const EnumDef;
    pub fn upb_FileDef_TopLevelExtensionCount(f: *const FileDef) -> c_int;
    pub fn upb_FileDef_TopLevelExtension(f: *const FileDef, i: c_int) -> *const FieldDef;
    pub fn upb_FileDef_ServiceCount(f: *const FileDef) -> c_int;
    pub fn upb_FileDef_Service(f: *const FileDef, i: c_int) -> *const ServiceDef;

    // ── Message ───────────────────────────────────────────────────────────
    pub fn upb_MessageDef_FullName(m: *const MessageDef) -> *const c_char;
    pub fn upb_MessageDef_Name(m: *const MessageDef) -> *const c_char;
    pub fn upb_MessageDef_File(m: *const MessageDef) -> *const FileDef;
    pub fn upb_MessageDef_ContainingType(m: *const MessageDef) -> *const MessageDef;
    pub fn upb_MessageDef_FieldCount(m: *const MessageDef) -> c_int;
    pub fn upb_MessageDef_Field(m: *const MessageDef, i: c_int) -> *const FieldDef;
    pub fn upb_MessageDef_FindFieldByNumber(m: *const MessageDef, i: u32) -> *const FieldDef;
    pub fn upb_MessageDef_FindFieldByNameWithSize(
        m: *const MessageDef,
        name: *const c_char,
        size: usize,
    ) -> *const FieldDef;
    pub fn upb_MessageDef_NestedMessageCount(m: *const MessageDef) -> c_int;
    pub fn upb_MessageDef_NestedMessage(m: *const MessageDef, i: c_int) -> *const MessageDef;
    pub fn upb_MessageDef_NestedEnumCount(m: *const MessageDef) -> c_int;
    pub fn upb_MessageDef_NestedEnum(m: *const MessageDef, i: c_int) -> *const EnumDef;
    pub fn upb_MessageDef_NestedExtensionCount(m: *const MessageDef) -> c_int;
    pub fn upb_MessageDef_NestedExtension(m: *const MessageDef, i: c_int) -> *const FieldDef;
    pub fn upb_MessageDef_OneofCount(m: *const MessageDef) -> c_int;
    pub fn upb_MessageDef_Oneof(m: *const MessageDef, i: c_int) -> *const OneofDef;
    pub fn upb_MessageDef_IsMapEntry(m: *const MessageDef) -> bool;
    pub fn upb_MessageDef_ExtensionRangeCount(m: *const MessageDef) -> c_int;
    pub fn upb_MessageDef_ExtensionRange(m: *const MessageDef, i: c_int) -> *const ExtensionRange;
    pub fn upb_ExtensionRange_Start(r: *const ExtensionRange) -> i32;
    pub fn upb_ExtensionRange_End(r: *const ExtensionRange) -> i32;

    // ── Field ─────────────────────────────────────────────────────────────
    pub fn upb_FieldDef_Name(f: *const FieldDef) -> *const c_char;
    pub fn upb_FieldDef_FullName(f: *const FieldDef) -> *const c_char;
    pub fn upb_FieldDef_File(f: *const FieldDef) -> *const FileDef;
    pub fn upb_FieldDef_Number(f: *const FieldDef) -> u32;
    pub fn upb_FieldDef_Type(f: *const FieldDef) -> FieldType;
    pub fn upb_FieldDef_IsRepeated(f: *const FieldDef) -> bool;
    pub fn upb_FieldDef_IsRequired(f: *const FieldDef) -> bool;
    pub fn upb_FieldDef_IsPacked(f: *const FieldDef) -> bool;
    pub fn upb_FieldDef_HasPresence(f: *const FieldDef) -> bool;
    pub fn upb_FieldDef_IsExtension(f: *const FieldDef) -> bool;
    pub fn upb_FieldDef_ContainingType(f: *const FieldDef) -> *const MessageDef;
    pub fn upb_FieldDef_ExtensionScope(f: *const FieldDef) -> *const MessageDef;
    pub fn upb_FieldDef_ContainingOneof(f: *const FieldDef) -> *const OneofDef;
    pub fn upb_FieldDef_MessageSubDef(f: *const FieldDef) -> *const MessageDef;
    pub fn upb_FieldDef_EnumSubDef(f: *const FieldDef) -> *const EnumDef;
    pub fn upb_FieldDef_JsonName(f: *const FieldDef) -> *const c_char;

    // ── Oneof ─────────────────────────────────────────────────────────────
    pub fn upb_OneofDef_Name(o: *const OneofDef) -> *const c_char;
    pub fn upb_OneofDef_IsSynthetic(o: *const OneofDef) -> bool;
    pub fn upb_OneofDef_FieldCount(o: *const OneofDef) -> c_int;
    pub fn upb_OneofDef_Field(o: *const OneofDef, i: c_int) -> *const FieldDef;

    // ── Enum ──────────────────────────────────────────────────────────────
    pub fn upb_EnumDef_FullName(e: *const EnumDef) -> *const c_char;
    pub fn upb_EnumDef_Name(e: *const EnumDef) -> *const c_char;
    pub fn upb_EnumDef_File(e: *const EnumDef) -> *const FileDef;
    pub fn upb_EnumDef_ContainingType(e: *const EnumDef) -> *const MessageDef;
    pub fn upb_EnumDef_IsClosed(e: *const EnumDef) -> bool;
    pub fn upb_EnumDef_ValueCount(e: *const EnumDef) -> c_int;
    pub fn upb_EnumDef_Value(e: *const EnumDef, i: c_int) -> *const EnumValueDef;
    pub fn upb_EnumDef_FindValueByNumber(e: *const EnumDef, num: i32) -> *const EnumValueDef;
    pub fn upb_EnumValueDef_Enum(v: *const EnumValueDef) -> *const EnumDef;
    pub fn upb_EnumValueDef_FullName(v: *const EnumValueDef) -> *const c_char;
    pub fn upb_EnumValueDef_Name(v: *const EnumValueDef) -> *const c_char;
    pub fn upb_EnumValueDef_Number(v: *const EnumValueDef) -> i32;

    // ── Service and method ────────────────────────────────────────────────
    pub fn upb_ServiceDef_FullName(s: *const ServiceDef) -> *const c_char;
    pub fn upb_ServiceDef_Name(s: *const ServiceDef) -> *const c_char;
    pub fn upb_ServiceDef_File(s: *const ServiceDef) -> *const FileDef;
    pub fn upb_ServiceDef_Index(s: *const ServiceDef) -> c_int;
    pub fn upb_ServiceDef_MethodCount(s: *const ServiceDef) -> c_int;
    pub fn upb_ServiceDef_Method(s: *const ServiceDef, i: c_int) -> *const MethodDef;
    pub fn upb_ServiceDef_FindMethodByNameWithSize(
        s: *const ServiceDef,
        name: *const c_char,
        len: usize,
    ) -> *const MethodDef;
    pub fn upb_MethodDef_FullName(m: *const MethodDef) -> *const c_char;
    pub fn upb_MethodDef_Name(m: *const MethodDef) -> *const c_char;
    pub fn upb_MethodDef_Index(m: *const MethodDef) -> c_int;
    pub fn upb_MethodDef_Service(m: *const MethodDef) -> *const ServiceDef;
    pub fn upb_MethodDef_InputType(m: *const MethodDef) -> *const MessageDef;
    pub fn upb_MethodDef_OutputType(m: *const MethodDef) -> *const MessageDef;
    pub fn upb_MethodDef_ClientStreaming(m: *const MethodDef) -> bool;
    pub fn upb_MethodDef_ServerStreaming(m: *const MethodDef) -> bool;

    // ── Arena ─────────────────────────────────────────────────────────────
    pub fn upb_Arena_New() -> *mut Arena;
    pub fn upb_Arena_Free(a: *mut Arena);
    pub fn upb_Arena_Malloc(a: *mut Arena, size: usize) -> *mut c_void;

    // ── Messages ──────────────────────────────────────────────────────────
    pub fn upb_MessageDef_MiniTable(m: *const MessageDef) -> *const MiniTable;
    pub fn upb_DefPool_ExtensionRegistry(s: *const DefPool) -> *const ExtensionRegistry;
    pub fn upb_Message_New(m: *const MiniTable, arena: *mut Arena) -> *mut RawMessage;
    pub fn upb_Decode(
        buf: *const c_char,
        size: usize,
        msg: *mut RawMessage,
        mt: *const MiniTable,
        extreg: *const ExtensionRegistry,
        options: c_int,
        arena: *mut Arena,
    ) -> c_int;
    pub fn upb_Encode(
        msg: *const RawMessage,
        l: *const MiniTable,
        options: c_int,
        arena: *mut Arena,
        buf: *mut *mut c_char,
        size: *mut usize,
    ) -> c_int;
    pub fn upb_Message_HasFieldByDef(msg: *const RawMessage, f: *const FieldDef) -> bool;
    pub fn upb_Message_GetFieldByDef(msg: *const RawMessage, f: *const FieldDef) -> MessageValue;
    pub fn upb_Message_SetFieldByDef(
        msg: *mut RawMessage,
        f: *const FieldDef,
        val: MessageValue,
        a: *mut Arena,
    ) -> bool;
    pub fn upb_Message_ClearFieldByDef(msg: *mut RawMessage, f: *const FieldDef);
    pub fn upb_Message_Mutable(
        msg: *mut RawMessage,
        f: *const FieldDef,
        a: *mut Arena,
    ) -> MutableMessageValue;
    pub fn upb_Message_Next(
        msg: *const RawMessage,
        m: *const MessageDef,
        ext_pool: *const DefPool,
        f: *mut *const FieldDef,
        val: *mut MessageValue,
        iter: *mut usize,
    ) -> bool;
    pub fn upb_Message_DeepClone(
        msg: *const RawMessage,
        m: *const MiniTable,
        arena: *mut Arena,
    ) -> *mut RawMessage;

    // ── Arrays and maps ───────────────────────────────────────────────────
    pub fn upb_Array_Size(arr: *const Array) -> usize;
    pub fn upb_Array_Get(arr: *const Array, i: usize) -> MessageValue;
    pub fn upb_Array_Append(array: *mut Array, val: MessageValue, arena: *mut Arena) -> bool;
    pub fn upb_Array_Delete(array: *mut Array, i: usize, count: usize);
    pub fn upb_Array_GetMutable(arr: *mut Array, i: usize) -> *mut RawMessage;
    pub fn upb_Map_Size(map: *const Map) -> usize;
    pub fn upb_Map_Next(
        map: *const Map,
        key: *mut MessageValue,
        val: *mut MessageValue,
        iter: *mut usize,
    ) -> bool;
    pub fn upb_Map_Set(
        map: *mut Map,
        key: MessageValue,
        val: MessageValue,
        arena: *mut Arena,
    ) -> bool;

    // ── Shim (src/shim.c) ─────────────────────────────────────────────────
    pub fn prototext_upb_add_file(
        pool: *mut DefPool,
        buf: *const c_char,
        len: usize,
        err: *mut c_char,
        errlen: usize,
    ) -> *const FileDef;
    pub fn prototext_upb_message_set_wire_format(m: *const MessageDef) -> bool;
    pub fn prototext_upb_free_extensions(exts: *mut *const FieldDef);
}

#[cfg(test)]
mod tests {
    /// Every declaration above names a symbol the linker can find (spec
    /// 0415 S2). Taking each function's address forces it to be linked, so
    /// an upb upgrade that leaves a declared function inline-only fails
    /// this test's build instead of a caller's at an unlucky moment.
    #[test]
    fn every_declaration_links() {
        let addresses: &[*const ()] = &[
            super::upb_DefPool_New as *const (),
            super::upb_DefPool_Free as *const (),
            super::upb_DefPool_FindMessageByNameWithSize as *const (),
            super::upb_DefPool_FindEnumByNameWithSize as *const (),
            super::upb_DefPool_FindFileByNameWithSize as *const (),
            super::upb_DefPool_FindExtensionByNumber as *const (),
            super::upb_DefPool_FindExtensionByNameWithSize as *const (),
            super::upb_FileDef_Name as *const (),
            super::upb_FileDef_Package as *const (),
            super::upb_FileDef_Edition as *const (),
            super::upb_FileDef_DependencyCount as *const (),
            super::upb_FileDef_Dependency as *const (),
            super::upb_FileDef_TopLevelMessageCount as *const (),
            super::upb_FileDef_TopLevelMessage as *const (),
            super::upb_FileDef_TopLevelEnumCount as *const (),
            super::upb_FileDef_TopLevelEnum as *const (),
            super::upb_FileDef_TopLevelExtensionCount as *const (),
            super::upb_FileDef_TopLevelExtension as *const (),
            super::upb_MessageDef_FullName as *const (),
            super::upb_MessageDef_Name as *const (),
            super::upb_MessageDef_File as *const (),
            super::upb_MessageDef_ContainingType as *const (),
            super::upb_MessageDef_FieldCount as *const (),
            super::upb_MessageDef_Field as *const (),
            super::upb_MessageDef_FindFieldByNumber as *const (),
            super::upb_MessageDef_FindFieldByNameWithSize as *const (),
            super::upb_MessageDef_NestedMessageCount as *const (),
            super::upb_MessageDef_NestedMessage as *const (),
            super::upb_MessageDef_NestedEnumCount as *const (),
            super::upb_MessageDef_NestedEnum as *const (),
            super::upb_MessageDef_NestedExtensionCount as *const (),
            super::upb_MessageDef_NestedExtension as *const (),
            super::upb_MessageDef_OneofCount as *const (),
            super::upb_MessageDef_Oneof as *const (),
            super::upb_MessageDef_IsMapEntry as *const (),
            super::upb_FieldDef_Name as *const (),
            super::upb_FieldDef_FullName as *const (),
            super::upb_FieldDef_File as *const (),
            super::upb_FieldDef_Number as *const (),
            super::upb_FieldDef_Type as *const (),
            super::upb_FieldDef_IsRepeated as *const (),
            super::upb_FieldDef_IsRequired as *const (),
            super::upb_FieldDef_IsPacked as *const (),
            super::upb_FieldDef_HasPresence as *const (),
            super::upb_FieldDef_IsExtension as *const (),
            super::upb_FieldDef_ContainingType as *const (),
            super::upb_FieldDef_ExtensionScope as *const (),
            super::upb_FieldDef_ContainingOneof as *const (),
            super::upb_FieldDef_MessageSubDef as *const (),
            super::upb_FieldDef_EnumSubDef as *const (),
            super::upb_FieldDef_JsonName as *const (),
            super::upb_OneofDef_Name as *const (),
            super::upb_OneofDef_IsSynthetic as *const (),
            super::upb_OneofDef_FieldCount as *const (),
            super::upb_OneofDef_Field as *const (),
            super::upb_EnumDef_FullName as *const (),
            super::upb_EnumDef_Name as *const (),
            super::upb_EnumDef_File as *const (),
            super::upb_EnumDef_ContainingType as *const (),
            super::upb_EnumDef_IsClosed as *const (),
            super::upb_EnumDef_ValueCount as *const (),
            super::upb_EnumDef_Value as *const (),
            super::upb_EnumDef_FindValueByNumber as *const (),
            super::upb_EnumValueDef_Enum as *const (),
            super::upb_EnumValueDef_FullName as *const (),
            super::upb_EnumValueDef_Name as *const (),
            super::upb_EnumValueDef_Number as *const (),
            super::upb_Arena_New as *const (),
            super::upb_Arena_Free as *const (),
            super::upb_Arena_Malloc as *const (),
            super::upb_MessageDef_MiniTable as *const (),
            super::upb_DefPool_ExtensionRegistry as *const (),
            super::upb_Message_New as *const (),
            super::upb_Decode as *const (),
            super::upb_Encode as *const (),
            super::upb_Message_HasFieldByDef as *const (),
            super::upb_Message_GetFieldByDef as *const (),
            super::upb_Message_SetFieldByDef as *const (),
            super::upb_Message_ClearFieldByDef as *const (),
            super::upb_Message_Mutable as *const (),
            super::upb_Message_Next as *const (),
            super::upb_Message_DeepClone as *const (),
            super::upb_Array_Size as *const (),
            super::upb_Array_Get as *const (),
            super::upb_Array_Append as *const (),
            super::upb_Array_Delete as *const (),
            super::upb_Array_GetMutable as *const (),
            super::upb_Map_Size as *const (),
            super::upb_Map_Next as *const (),
            super::upb_Map_Set as *const (),
            super::upb_DefPool_FindServiceByNameWithSize as *const (),
            super::upb_DefPool_GetAllExtensions as *const (),
            super::upb_FileDef_ServiceCount as *const (),
            super::upb_FileDef_Service as *const (),
            super::upb_MessageDef_ExtensionRangeCount as *const (),
            super::upb_MessageDef_ExtensionRange as *const (),
            super::upb_ExtensionRange_Start as *const (),
            super::upb_ExtensionRange_End as *const (),
            super::upb_ServiceDef_FullName as *const (),
            super::upb_ServiceDef_Name as *const (),
            super::upb_ServiceDef_File as *const (),
            super::upb_ServiceDef_Index as *const (),
            super::upb_ServiceDef_MethodCount as *const (),
            super::upb_ServiceDef_Method as *const (),
            super::upb_ServiceDef_FindMethodByNameWithSize as *const (),
            super::upb_MethodDef_FullName as *const (),
            super::upb_MethodDef_Name as *const (),
            super::upb_MethodDef_Index as *const (),
            super::upb_MethodDef_Service as *const (),
            super::upb_MethodDef_InputType as *const (),
            super::upb_MethodDef_OutputType as *const (),
            super::upb_MethodDef_ClientStreaming as *const (),
            super::upb_MethodDef_ServerStreaming as *const (),
            super::prototext_upb_add_file as *const (),
            super::prototext_upb_message_set_wire_format as *const (),
            super::prototext_upb_free_extensions as *const (),
        ];
        assert!(addresses.iter().all(|a| !a.is_null()));
    }
}
