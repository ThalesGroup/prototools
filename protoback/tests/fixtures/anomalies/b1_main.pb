#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "b1_main.proto"  #@ string = 1
 dependency: "b1_helper.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "Outer"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "value"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
  }
  extension {  #@ repeated FieldDescriptorProto = 6
   name: "nested_ext"  #@ string = 1
   extendee: ".Extendable"  #@ string = 2
   number: 101  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
  }
 }
 syntax: "proto3"  #@ string = 12
}
