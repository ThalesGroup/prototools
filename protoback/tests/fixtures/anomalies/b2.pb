#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "b2_main.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "WithRange"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "value"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
  }
  extension_range {  #@ repeated ExtensionRange = 5
   start: 100  #@ int32 = 1
   end: 200  #@ int32 = 2
  }
 }
 syntax: "proto3"  #@ string = 12
}
