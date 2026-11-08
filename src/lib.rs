//! Carry Reticulum packets directly over Ethernet, without IP.

pub mod frame;
pub mod hdlc;
#[cfg(target_os = "linux")]
pub mod link;
