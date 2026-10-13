// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

// What upb.h declares only `static inline`, re-exported as real symbols
// for the Rust side (spec 0415 S2). Everything else is linked directly.

#include <string.h>

#include "upb.h"

// Parse one serialized FileDescriptorProto and add it to `pool`.
//
// upb_DefPool_AddFile takes a parsed proto, and parsing needs an arena and
// the generated (inline) parser; doing both here keeps upb's arenas and
// status struct on this side of the boundary. upb copies everything it
// keeps (names, options) into the pool's own arena, so the parse arena is
// freed before returning.
//
// Returns the new file, or NULL with a NUL-terminated message in `err`
// (truncated to `errlen` bytes).
const upb_FileDef* prototext_upb_add_file(upb_DefPool* pool, const char* buf,
                                          size_t len, char* err,
                                          size_t errlen) {
  if (errlen > 0) err[0] = '\0';
  upb_Arena* arena = upb_Arena_New();
  if (!arena) {
    if (errlen > 0) {
      strncpy(err, "out of memory", errlen - 1);
      err[errlen - 1] = '\0';
    }
    return NULL;
  }
  const google_protobuf_FileDescriptorProto* proto =
      google_protobuf_FileDescriptorProto_parse(buf, len, arena);
  const upb_FileDef* file = NULL;
  if (!proto) {
    if (errlen > 0) {
      strncpy(err, "not a valid FileDescriptorProto", errlen - 1);
      err[errlen - 1] = '\0';
    }
  } else {
    upb_Status status;
    upb_Status_Clear(&status);
    file = upb_DefPool_AddFile(pool, proto, &status);
    if (!file && errlen > 0) {
      strncpy(err, upb_Status_ErrorMessage(&status), errlen - 1);
      err[errlen - 1] = '\0';
    }
  }
  upb_Arena_Free(arena);
  return file;
}

// `option message_set_wire_format = true` on a message.
bool prototext_upb_message_set_wire_format(const upb_MessageDef* m) {
  const google_protobuf_MessageOptions* opts = upb_MessageDef_Options(m);
  return opts && google_protobuf_MessageOptions_message_set_wire_format(opts);
}
