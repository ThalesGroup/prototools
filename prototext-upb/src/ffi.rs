// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Hand-written declarations of the upb functions this crate calls (spec
//! 0415 S2). Only public functions, never `_upb_…` or `UPB_PRIVATE`
//! symbols; each prototype is copied from the vendored `upb.h`. The
//! `every_declaration_links` test takes the address of each one, so an
//! upgrade that turns one inline fails the build of the test.

use std::ffi::{c_char, c_int};

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
);

/// C `upb_FieldType`: the values of `FieldDescriptorProto.Type`.
pub type FieldType = c_int;
/// C `google_protobuf_Edition`.
pub type EditionNumber = c_int;

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

    // ── Shim (src/shim.c) ─────────────────────────────────────────────────
    pub fn prototext_upb_add_file(
        pool: *mut DefPool,
        buf: *const c_char,
        len: usize,
        err: *mut c_char,
        errlen: usize,
    ) -> *const FileDef;
    pub fn prototext_upb_message_set_wire_format(m: *const MessageDef) -> bool;
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
            super::prototext_upb_add_file as *const (),
            super::prototext_upb_message_set_wire_format as *const (),
        ];
        assert!(addresses.iter().all(|a| !a.is_null()));
    }
}
