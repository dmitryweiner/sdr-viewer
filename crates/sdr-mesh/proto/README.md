# Meshtastic's protobuf files

`meshtastic/*.proto` are copied unchanged from
[meshtastic/protobufs](https://github.com/meshtastic/protobufs) at tag
`v2.7.26`, the firmware `meshtasticd` runs on the user's device: only
`mesh.proto` and what it imports. They are GPL-3.0, like this app.

`build.rs` compiles them with `protox` (no `protoc` needed) into the types
of `sdr_mesh::proto`. A newer firmware: copy the same files from its tag
and run the tests.
