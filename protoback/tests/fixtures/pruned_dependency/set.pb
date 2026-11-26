#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "weak.proto"  #@ string = 1
 package: "strip_test"  #@ string = 2
 message_type {  #@ repeated DescriptorProto = 4
  name: "Weak"  #@ string = 1
 }
 syntax: "proto2"  #@ string = 12
}
file {  #@ repeated FileDescriptorProto = 1
 name: "public.proto"  #@ string = 1
 package: "strip_test"  #@ string = 2
 message_type {  #@ repeated DescriptorProto = 4
  name: "Public"  #@ string = 1
 }
 syntax: "proto2"  #@ string = 12
}
file {  #@ repeated FileDescriptorProto = 1
 name: "importer.proto"  #@ string = 1
 package: "strip_test"  #@ string = 2
 dependency: "missing.proto"  #@ repeated string = 3
 dependency: "weak.proto"  #@ repeated string = 3
 dependency: "public.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "Importer"  #@ string = 1
 }
 public_dependency: 2  #@ repeated int32 = 10
 weak_dependency: 1  #@ repeated int32 = 11
 syntax: "proto2"  #@ string = 12
}
