#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "b3_main.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "MsgSet"  #@ string = 1
  options {  #@ MessageOptions = 7
   message_set_wire_format: true  #@ bool = 1
  }
 }
 syntax: "proto3"  #@ string = 12
}
