#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "ord_use.proto"  #@ string = 1
 package: "ord"  #@ string = 2
 message_type {  #@ repeated DescriptorProto = 4
  name: "M"  #@ string = 1
 }
 options {  #@ FileOptions = 8
  50101: "set"  #@ string
 }
}
