#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "a5_main.proto"  #@ string = 1
 dependency: "extendable_helper.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "Main"  #@ string = 1
 }
 extension {  #@ repeated FieldDescriptorProto = 7
  name: "my_ext"  #@ string = 1
  extendee: ".Extendable"  #@ string = 2
  number: 100  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_INT32  #@ Type(5) = 5
 }
 syntax: "proto3"  #@ string = 12
}
