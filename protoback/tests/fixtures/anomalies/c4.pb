#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "c4_main.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "Msg"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "greeting"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_STRING  #@ Type(9) = 5
   default_value: "hello"  #@ string = 7
  }
 }
 syntax: "proto3"  #@ string = 12
}
