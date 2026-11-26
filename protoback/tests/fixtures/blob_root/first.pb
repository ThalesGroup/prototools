#@ prototext: protoc
name: "first.proto"  #@ string = 1
package: "one"  #@ string = 2
message_type {  #@ repeated DescriptorProto = 4
 name: "First"  #@ string = 1
 field {  #@ repeated FieldDescriptorProto = 2
  name: "value"  #@ string = 1
  number: 1  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_STRING  #@ Type(9) = 5
 }
}
syntax: "proto3"  #@ string = 12
