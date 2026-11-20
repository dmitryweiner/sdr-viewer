//! A client of `meshtasticd`'s TCP API (port 4403): the nodes of the mesh
//! and the packets the node hears, as plain types for the interface.

/// Meshtastic's protobuf messages, generated from `proto/` by `build.rs`.
#[allow(clippy::all, clippy::pedantic)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/meshtastic.rs"));
}

pub mod client;
pub mod frame;
pub mod lora;
pub mod session;
pub mod update;
