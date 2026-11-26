#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "base.proto"  #@ string = 1
 package: "p"  #@ string = 2
 message_type {  #@ repeated DescriptorProto = 4
  name: "Base"  #@ string = 1
  extension_range {  #@ repeated ExtensionRange = 5
   start: 100  #@ int32 = 1
   end: 200  #@ int32 = 2
  }
 }
 syntax: "proto2"  #@ string = 12
}
file {  #@ repeated FileDescriptorProto = 1
 name: "m.proto"  #@ string = 1
 package: "p"  #@ string = 2
 dependency: "base.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "A"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "i"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_INT32  #@ Type(5) = 5
  }
  nested_type {  #@ repeated DescriptorProto = 3
   name: "B"  #@ string = 1
   nested_type {  #@ repeated DescriptorProto = 3
    name: "C"  #@ string = 1
    extension {  #@ repeated FieldDescriptorProto = 6
     name: "x"  #@ string = 1
     extendee: ".p.Base"  #@ string = 2
     number: 100  #@ int32 = 3
     label: LABEL_OPTIONAL  #@ Label(1) = 4
     type: TYPE_INT32  #@ Type(5) = 5
    }
   }
  }
 }
 syntax: "proto3"  #@ string = 12
}
