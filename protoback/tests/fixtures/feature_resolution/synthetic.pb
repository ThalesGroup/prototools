#@ prototext: protoc
name: "synthetic_descriptor.proto"  #@ string = 1
message_type {  #@ repeated DescriptorProto = 4
 name: "FeatureSet"  #@ string = 1
 field {  #@ repeated FieldDescriptorProto = 2
  name: "field_presence"  #@ string = 1
  number: 1  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_ENUM  #@ Type(14) = 5
  type_name: ".FeatureSet.FIELD_PRESENCE"  #@ string = 6
  options {  #@ FieldOptions = 8
   edition_defaults {  #@ repeated EditionDefault = 20
    value: "FIRST"  #@ string = 2
    edition: EDITION_LEGACY  #@ Edition(900) = 3
   }
   edition_defaults {  #@ repeated EditionDefault = 20
    value: "SECOND"  #@ string = 2
    edition: EDITION_2023  #@ Edition(1000) = 3
   }
  }
 }
 enum_type {  #@ repeated EnumDescriptorProto = 4
  name: "FIELD_PRESENCE"  #@ string = 1
  value {  #@ repeated EnumValueDescriptorProto = 2
   name: "FIELD_PRESENCE_UNKNOWN"  #@ string = 1
   number: 0  #@ int32 = 2
  }
  value {  #@ repeated EnumValueDescriptorProto = 2
   name: "FIRST"  #@ string = 1
   number: 1  #@ int32 = 2
  }
  value {  #@ repeated EnumValueDescriptorProto = 2
   name: "SECOND"  #@ string = 1
   number: 2  #@ int32 = 2
  }
 }
}
syntax: "proto2"  #@ string = 12
