#@ prototext: protoc
name: "editions_rendering.proto"  #@ string = 1
package: "reproto.test.rendering"  #@ string = 2
message_type {  #@ repeated DescriptorProto = 4
 name: "Inner"  #@ string = 1
 field {  #@ repeated FieldDescriptorProto = 2
  name: "value"  #@ string = 1
  number: 1  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_INT32  #@ Type(5) = 5
  json_name: "value"  #@ string = 10
 }
}
message_type {  #@ repeated DescriptorProto = 4
 name: "AllFeatures"  #@ string = 1
 field {  #@ repeated FieldDescriptorProto = 2
  name: "implicit_field"  #@ string = 1
  number: 1  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_STRING  #@ Type(9) = 5
  options {  #@ FieldOptions = 8
  }
  json_name: "implicitField"  #@ string = 10
 }
 field {  #@ repeated FieldDescriptorProto = 2
  name: "explicit_field"  #@ string = 1
  number: 2  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_STRING  #@ Type(9) = 5
  options {  #@ FieldOptions = 8
  }
  json_name: "explicitField"  #@ string = 10
 }
 field {  #@ repeated FieldDescriptorProto = 2
  name: "required_field"  #@ string = 1
  number: 3  #@ int32 = 3
  label: LABEL_REQUIRED  #@ Label(2) = 4
  type: TYPE_STRING  #@ Type(9) = 5
  options {  #@ FieldOptions = 8
  }
  json_name: "requiredField"  #@ string = 10
 }
 field {  #@ repeated FieldDescriptorProto = 2
  name: "expanded_ids"  #@ string = 1
  number: 4  #@ int32 = 3
  label: LABEL_REPEATED  #@ Label(3) = 4
  type: TYPE_INT32  #@ Type(5) = 5
  options {  #@ FieldOptions = 8
  }
  json_name: "expandedIds"  #@ string = 10
 }
 field {  #@ repeated FieldDescriptorProto = 2
  name: "delimited_field"  #@ string = 1
  number: 5  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_MESSAGE  #@ Type(11) = 5
  type_name: ".reproto.test.rendering.Inner"  #@ string = 6
  options {  #@ FieldOptions = 8
  }
  json_name: "delimitedField"  #@ string = 10
 }
 field {  #@ repeated FieldDescriptorProto = 2
  name: "with_default"  #@ string = 1
  number: 6  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_INT32  #@ Type(5) = 5
  default_value: "42"  #@ string = 7
  options {  #@ FieldOptions = 8
  }
  json_name: "withDefault"  #@ string = 10
 }
 field {  #@ repeated FieldDescriptorProto = 2
  name: "packed_ids"  #@ string = 1
  number: 7  #@ int32 = 3
  label: LABEL_REPEATED  #@ Label(3) = 4
  type: TYPE_INT32  #@ Type(5) = 5
  options {  #@ FieldOptions = 8
   packed: true  #@ bool = 2
  }
  json_name: "packedIds"  #@ string = 10
 }
}
