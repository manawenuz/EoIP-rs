//! gRPC service definitions for EoIP-rs management API.
//!
//! Generated from `proto/eoip.proto` by tonic-build.

pub mod eoip {
    pub mod v1 {
        tonic::include_proto!("eoip.v1");
    }
}

pub use eoip::v1::*;
