#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "ext.proto"  #@ string = 1
 package: "ext"  #@ string = 2
 dependency: "internal/proto2/descriptor.proto"  #@ repeated string = 3
 extension {  #@ repeated FieldDescriptorProto = 7
  name: "level"  #@ string = 1
  extendee: ".proto2.FieldOptions"  #@ string = 2
  number: 50001  #@ int32 = 3
  label: LABEL_OPTIONAL  #@ Label(1) = 4
  type: TYPE_INT32  #@ Type(5) = 5
  json_name: "level"  #@ string = 10
 }
}
