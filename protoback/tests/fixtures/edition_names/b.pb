#@ prototext: protoc
name: "a/b.proto"  #@ string = 1
package: "a"  #@ string = 2
message_type {  #@ repeated DescriptorProto = 4
 name: "M"  #@ string = 1
}
syntax: "editions"  #@ string = 12
edition: 1003  #@ Edition(1003) = 14; ENUM_UNKNOWN
