#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "internal/pb/cpp_features.proto"  #@ string = 1
 package: "pb"  #@ string = 2
 dependency: "internal/proto2/descriptor.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "CppFeatures"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "legacy_closed_enum"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_BOOL  #@ Type(8) = 5
   options {  #@ FieldOptions = 8
    retention: RETENTION_RUNTIME  #@ OptionRetention(1) = 17
    targets: TARGET_TYPE_FIELD  #@ repeated OptionTargetType(4) = 19
    targets: TARGET_TYPE_FILE  #@ repeated OptionTargetType(1) = 19
    edition_defaults {  #@ repeated EditionDefault = 20
     value: "true"  #@ string = 2
     edition: EDITION_LEGACY  #@ Edition(900) = 3
    }
    edition_defaults {  #@ repeated EditionDefault = 20
     value: "false"  #@ string = 2
     edition: EDITION_PROTO3  #@ Edition(999) = 3
    }
    feature_support {  #@ FeatureSupport = 22
     edition_introduced: EDITION_2023  #@ Edition(1000) = 1
     edition_deprecated: EDITION_2023  #@ Edition(1000) = 2
     deprecation_warning: "The legacy closed enum behavior in C++ is deprecated and is scheduled to be removed in edition 2025.  See http://protobuf.dev/programming-guides/enum/#cpp for more information"  #@ string = 3
    }
   }
   json_name: "legacyClosedEnum"  #@ string = 10
  }
  field {  #@ repeated FieldDescriptorProto = 2
   name: "string_type"  #@ string = 1
   number: 2  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_ENUM  #@ Type(14) = 5
   type_name: ".pb.CppFeatures.StringType"  #@ string = 6
   options {  #@ FieldOptions = 8
    retention: RETENTION_RUNTIME  #@ OptionRetention(1) = 17
    targets: TARGET_TYPE_FIELD  #@ repeated OptionTargetType(4) = 19
    targets: TARGET_TYPE_FILE  #@ repeated OptionTargetType(1) = 19
    edition_defaults {  #@ repeated EditionDefault = 20
     value: "STRING"  #@ string = 2
     edition: EDITION_LEGACY  #@ Edition(900) = 3
    }
    edition_defaults {  #@ repeated EditionDefault = 20
     value: "VIEW"  #@ string = 2
     edition: EDITION_2024  #@ Edition(1001) = 3
    }
    feature_support {  #@ FeatureSupport = 22
     edition_introduced: EDITION_2023  #@ Edition(1000) = 1
    }
   }
   json_name: "stringType"  #@ string = 10
  }
  field {  #@ repeated FieldDescriptorProto = 2
   name: "enum_name_uses_string_view"  #@ string = 1
   number: 3  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_BOOL  #@ Type(8) = 5
   options {  #@ FieldOptions = 8
    retention: RETENTION_RUNTIME  #@ OptionRetention(1) = 17
    targets: TARGET_TYPE_ENUM  #@ repeated OptionTargetType(6) = 19
    targets: TARGET_TYPE_FILE  #@ repeated OptionTargetType(1) = 19
    edition_defaults {  #@ repeated EditionDefault = 20
     value: "false"  #@ string = 2
     edition: EDITION_LEGACY  #@ Edition(900) = 3
    }
    edition_defaults {  #@ repeated EditionDefault = 20
     value: "true"  #@ string = 2
     edition: EDITION_2024  #@ Edition(1001) = 3
    }
    feature_support {  #@ FeatureSupport = 22
     edition_introduced: EDITION_2024  #@ Edition(1001) = 1
    }
   }
   json_name: "enumNameUsesStringView"  #@ string = 10
  }
  field {  #@ repeated FieldDescriptorProto = 2
   name: "repeated_type"  #@ string = 1
   number: 4  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_ENUM  #@ Type(14) = 5
   type_name: ".pb.CppFeatures.RepeatedType"  #@ string = 6
   options {  #@ FieldOptions = 8
    retention: RETENTION_RUNTIME  #@ OptionRetention(1) = 17
    targets: TARGET_TYPE_FIELD  #@ repeated OptionTargetType(4) = 19
    targets: TARGET_TYPE_FILE  #@ repeated OptionTargetType(1) = 19
    edition_defaults {  #@ repeated EditionDefault = 20
     value: "LEGACY"  #@ string = 2
     edition: EDITION_LEGACY  #@ Edition(900) = 3
    }
    feature_support {  #@ FeatureSupport = 22
     edition_introduced: EDITION_UNSTABLE  #@ Edition(9999) = 1
    }
   }
   json_name: "repeatedType"  #@ string = 10
  }
  enum_type {  #@ repeated EnumDescriptorProto = 4
   name: "StringType"  #@ string = 1
   value {  #@ repeated EnumValueDescriptorProto = 2
    name: "STRING_TYPE_UNKNOWN"  #@ string = 1
    number: 0  #@ int32 = 2
   }
   value {  #@ repeated EnumValueDescriptorProto = 2
    name: "VIEW"  #@ string = 1
    number: 1  #@ int32 = 2
   }
   value {  #@ repeated EnumValueDescriptorProto = 2
    name: "CORD"  #@ string = 1
    number: 2  #@ int32 = 2
   }
   value {  #@ repeated EnumValueDescriptorProto = 2
    name: "STRING"  #@ string = 1
    number: 3  #@ int32 = 2
   }
  }
  enum_type {  #@ repeated EnumDescriptorProto = 4
   name: "RepeatedType"  #@ string = 1
   value {  #@ repeated EnumValueDescriptorProto = 2
    name: "REPEATED_TYPE_UNKNOWN"  #@ string = 1
    number: 0  #@ int32 = 2
   }
   value {  #@ repeated EnumValueDescriptorProto = 2
    name: "LEGACY"  #@ string = 1
    number: 1  #@ int32 = 2
   }
   value {  #@ repeated EnumValueDescriptorProto = 2
    name: "PROXY"  #@ string = 1
    number: 2  #@ int32 = 2
   }
  }
 }
 extension {  #@ repeated FieldDescriptorProto = 7
  name: "cpp"  #@ string = 1
  extendee: ".proto2.FeatureSet"  #@ string = 2
  number: 1000  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_MESSAGE  #@ Type(11) = 5
  type_name: ".pb.CppFeatures"  #@ string = 6
  json_name: "cpp"  #@ string = 10
 }
}
