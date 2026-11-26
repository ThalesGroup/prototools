#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "c1_main.proto"  #@ string = 1
 message_type {  #@ repeated DescriptorProto = 4
  name: "Outer"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "bad_map"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_REPEATED  #@ Label(3) = 4
   type: TYPE_MESSAGE  #@ Type(11) = 5
   type_name: ".Outer.BadMapEntry"  #@ string = 6
  }
  nested_type {  #@ repeated DescriptorProto = 3
   name: "BadMapEntry"  #@ string = 1
   field {  #@ repeated FieldDescriptorProto = 2
    name: "key"  #@ string = 1
    number: 1  #@ int32 = 3
    label: LABEL_OPTIONAL  #@ Label(1) = 4
    type: TYPE_STRING  #@ Type(9) = 5
   }
   options {  #@ MessageOptions = 7
    map_entry: true  #@ bool = 7
   }
  }
 }
 syntax: "proto3"  #@ string = 12
}
