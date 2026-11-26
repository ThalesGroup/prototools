#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "inst.proto"  #@ string = 1
 package: "inst"  #@ string = 2
 message_type {  #@ repeated DescriptorProto = 4
  name: "M"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "labels"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_REPEATED  #@ Label(3) = 4
   type: TYPE_MESSAGE  #@ Type(11) = 5
   type_name: ".inst.M.LabelsEntry"  #@ string = 6
  }
  field {  #@ repeated FieldDescriptorProto = 2
   name: "single"  #@ string = 1
   number: 2  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_GROUP  #@ Type(10) = 5
   type_name: ".inst.M.G"  #@ string = 6
  }
  field {  #@ repeated FieldDescriptorProto = 2
   name: "many"  #@ string = 1
   number: 3  #@ int32 = 3
   label: LABEL_REPEATED  #@ Label(3) = 4
   type: TYPE_GROUP  #@ Type(10) = 5
   type_name: ".inst.M.G"  #@ string = 6
  }
  nested_type {  #@ repeated DescriptorProto = 3
   name: "LabelsEntry"  #@ string = 1
   field {  #@ repeated FieldDescriptorProto = 2
    name: "key"  #@ string = 1
    number: 1  #@ int32 = 3
    label: LABEL_OPTIONAL  #@ Label(1) = 4
    type: TYPE_STRING  #@ Type(9) = 5
   }
   field {  #@ repeated FieldDescriptorProto = 2
    name: "value"  #@ string = 1
    number: 2  #@ int32 = 3
    label: LABEL_OPTIONAL  #@ Label(1) = 4
    type: TYPE_STRING  #@ Type(9) = 5
   }
   options {  #@ MessageOptions = 7
    map_entry: true  #@ bool = 7
   }
  }
  nested_type {  #@ repeated DescriptorProto = 3
   name: "G"  #@ string = 1
   field {  #@ repeated FieldDescriptorProto = 2
    name: "x"  #@ string = 1
    number: 1  #@ int32 = 3
    label: LABEL_OPTIONAL  #@ Label(1) = 4
    type: TYPE_INT32  #@ Type(5) = 5
   }
  }
 }
 syntax: "proto2"  #@ string = 12
}
