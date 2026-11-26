#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "user.proto"  #@ string = 1
 package: "user"  #@ string = 2
 dependency: "google/protobuf/descriptor.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "U"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "x"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
   json_name: "x"  #@ string = 10
  }
 }
 extension {  #@ repeated FieldDescriptorProto = 7
  name: "level"  #@ string = 1
  extendee: ".google.protobuf.FileOptions"  #@ string = 2
  number: 50001  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_INT32  #@ Type(5) = 5
  json_name: "level"  #@ string = 10
 }
}
