#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "a5_regression.proto"  #@ string = 1
 extension {  #@ repeated FieldDescriptorProto = 7
  name: "my_method_opt"  #@ string = 1
  extendee: ".proto2.MethodOptions"  #@ string = 2
  number: 50000  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_BOOL  #@ Type(8) = 5
 }
 syntax: "proto3"  #@ string = 12
}
