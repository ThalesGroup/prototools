#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "weak_dep.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "Dep"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "value"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
  }
 }
 syntax: "proto3"  #@ string = 12
}
