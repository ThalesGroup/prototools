# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from typing import Final

from google.protobuf.descriptor_pb2 import FieldDescriptorProto

# --- Configuration constants --------------------------------------------------

FIELD_NUM_MAX: Final[int] = 536870911

#                    Descriptor type       Fqdn Tag 
ENUM = 'enum'       # Enum                  'enum'
                    # Extension Descriptor  'exte'
FIELD = 'fdsc'      # Field Descriptor      'fdsc'
FILE = 'file'       # File                  'file'
                    # Map Descriptor        'mapd'
MESSAGE = 'desc'    # Message Descriptor    'desc'
METHOD = 'meth'     # Method                'meth'
                    # Oneof Descriptor      'onof'
                    # Option Descriptor     'optn'
                    # Package Descriptor    'pack'
SERVICE = 'serv'    # Service               'serv'


# --- Protobuf constants -------------------------------------------------------


LABEL_OPTIONAL: int = FieldDescriptorProto.LABEL_OPTIONAL
LABEL_REQUIRED = FieldDescriptorProto.LABEL_REQUIRED
LABEL_REPEATED = FieldDescriptorProto.LABEL_REPEATED


label_names: dict[int, str] = {
    # NOTE: Ideally label_names should be typed dict[ValueType, str], but it is
    # impossible to import ValueType. Anyways ValueType really is just int.
    LABEL_OPTIONAL: 'optional',  # 1
    LABEL_REQUIRED: 'required',  # 2
    LABEL_REPEATED: 'repeated',  # 3
}

TYPE_DOUBLE = FieldDescriptorProto.TYPE_DOUBLE
TYPE_FLOAT = FieldDescriptorProto.TYPE_FLOAT
TYPE_INT64 = FieldDescriptorProto.TYPE_INT64
TYPE_UINT64 = FieldDescriptorProto.TYPE_UINT64
TYPE_INT32 = FieldDescriptorProto.TYPE_INT32
TYPE_FIXED64 = FieldDescriptorProto.TYPE_FIXED64
TYPE_FIXED32 = FieldDescriptorProto.TYPE_FIXED32
TYPE_BOOL = FieldDescriptorProto.TYPE_BOOL
TYPE_STRING = FieldDescriptorProto.TYPE_STRING
TYPE_GROUP = FieldDescriptorProto.TYPE_GROUP
TYPE_MESSAGE = FieldDescriptorProto.TYPE_MESSAGE
TYPE_BYTES = FieldDescriptorProto.TYPE_BYTES
TYPE_UINT32 = FieldDescriptorProto.TYPE_UINT32
TYPE_ENUM = FieldDescriptorProto.TYPE_ENUM
TYPE_SFIXED32 = FieldDescriptorProto.TYPE_SFIXED32
TYPE_SFIXED64 = FieldDescriptorProto.TYPE_SFIXED64
TYPE_SINT32 = FieldDescriptorProto.TYPE_SINT32
TYPE_SINT64 = FieldDescriptorProto.TYPE_SINT64

type_names: dict[int, str] = {
    # NOTE: Ideally type_names should be typed dict[ValueType, str], but it is
    # impossible to import ValueType. Anyways ValueType really is just int.
    TYPE_DOUBLE: 'double',  #  1
    TYPE_FLOAT: 'float',  #  2
    TYPE_INT64: 'int64',  #  3
    TYPE_UINT64: 'uint64',  #  4
    TYPE_INT32: 'int32',  #  5
    TYPE_FIXED64: 'fixed64',  #  6
    TYPE_FIXED32: 'fixed32',  #  7
    TYPE_BOOL: 'bool',  #  8
    TYPE_STRING: 'string',  #  9
    TYPE_GROUP: 'group',  # 10
    TYPE_MESSAGE: 'message',  # 11
    TYPE_BYTES: 'bytes',  # 12
    TYPE_UINT32: 'uint32',  # 13
    TYPE_ENUM: 'enum',  # 14
    TYPE_SFIXED32: 'sfixed32',  # 15
    TYPE_SFIXED64: 'sfixed64',  # 16
    TYPE_SINT32: 'sint32',  # 17
    TYPE_SINT64: 'sint64',  # 18
}
