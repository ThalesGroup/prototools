#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "c3_main.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "Msg"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "mandatory"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_REQUIRED  #@ Label(2) = 4
   type: TYPE_STRING  #@ Type(9) = 5
  }
 }
 syntax: "proto3"  #@ string = 12
}
