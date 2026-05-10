#@ prototext: protoc
id: 1  #@ required uint32 = 1
name: "hi"  #@ string = 2
tags: 7  #@ repeated uint32 = 3
child {  #@ Inner = 4
 value: 42  #@ uint32 = 1
}
status: WARN  #@ Status(1) = 5
