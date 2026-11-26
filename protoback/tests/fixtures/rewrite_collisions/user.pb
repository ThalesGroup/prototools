#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "user.proto"  #@ string = 1
 package: "user"  #@ string = 2
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
 options {  #@ FileOptions = 8
  java_package: "com.example.user"  #@ string = 1
  99: 5  #@ varint
 }
}
