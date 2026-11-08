//! Carry Reticulum packets directly over Ethernet, without IP.

pub mod frame;
pub mod hdlc;
pub mod packet;
#[cfg(target_os = "linux")]
pub mod link;
