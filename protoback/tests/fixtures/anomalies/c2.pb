#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "c2_main.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "Outer"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "mygroup"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_GROUP  #@ Type(10) = 5
   type_name: ".Mygroup"  #@ string = 6
  }
 }
 message_type {  #@ repeated DescriptorProto = 4
  name: "Mygroup"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "val"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
  }
 }
 syntax: "proto3"  #@ string = 12
}
