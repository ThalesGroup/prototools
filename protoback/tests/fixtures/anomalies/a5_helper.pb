#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "extendable_helper.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "Extendable"  #@ string = 1
  extension_range {  #@ repeated ExtensionRange = 5
   start: 100  #@ int32 = 1
   end: 200  #@ int32 = 2
  }
 }
 syntax: "proto2"  #@ string = 12
}
