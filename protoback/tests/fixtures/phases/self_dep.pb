#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "prune_base.proto"  #@ string = 1
 package: "prune_test"  #@ string = 2
 dependency: "prune_base.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "Base"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "value"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
   json_name: "value"  #@ string = 10
  }
 }
 syntax: "proto3"  #@ string = 12
}
