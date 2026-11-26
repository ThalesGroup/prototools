#@ prototext: protoc
file {  #@ repeated FileDescriptorProto = 1
 name: "f/feat.proto"  #@ string = 1
 package: "feat"  #@ string = 2
 dependency: "internal/pb/cpp_features.proto"  #@ repeated string = 3
 message_type {  #@ repeated DescriptorProto = 4
  name: "M"  #@ string = 1
  field {  #@ repeated FieldDescriptorProto = 2
   name: "s"  #@ string = 1
   number: 1  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_STRING  #@ Type(9) = 5
   options {  #@ FieldOptions = 8
    features {  #@ FeatureSet = 21
     1000 {  #@ message
      2: 1  #@ varint
     }
    }
   }
   json_name: "s"  #@ string = 10
  }
  field {  #@ repeated FieldDescriptorProto = 2
   name: "t"  #@ string = 1
   number: 2  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_STRING  #@ Type(9) = 5
   options {  #@ FieldOptions = 8
    features {  #@ FeatureSet = 21
     field_presence: EXPLICIT  #@ FieldPresence(1) = 1
     1000 {  #@ message
      2: 3  #@ varint
     }
    }
   }
   json_name: "t"  #@ string = 10
  }
  field {  #@ repeated FieldDescriptorProto = 2
   name: "u"  #@ string = 1
   number: 3  #@ int32 = 3
   label: LABEL_OPTIONAL  #@ Label(1) = 4
   type: TYPE_STRING  #@ Type(9) = 5
   json_name: "u"  #@ string = 10
  }
 }
 options {  #@ FileOptions = 8
  features {  #@ FeatureSet = 50
   field_presence: IMPLICIT  #@ FieldPresence(2) = 1
   1000 {  #@ message
    2: 2  #@ varint
   }
  }
 }
 syntax: "editions"  #@ string = 12
 edition: EDITION_2023  #@ Edition(1000) = 14
}
