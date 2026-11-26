#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "u.proto"  #@ string = 1
 package: "u"  #@ string = 2
 dependency: "ext.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "M"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "x"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
   options {  #@ FieldOptions = 8
    50001: 7  #@ varint
   }
   json_name: "x"  #@ string = 10
  }
 }
}
