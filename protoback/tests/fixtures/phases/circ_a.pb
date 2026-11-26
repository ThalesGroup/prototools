#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "circ_a.proto"  #@ string = 1
 dependency: "circ_b.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "MsgA"  #@ string = 1
 }
 syntax: "proto3"  #@ string = 12
}
