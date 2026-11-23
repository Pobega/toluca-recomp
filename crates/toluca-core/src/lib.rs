//! Shared ABI definitions for toluca-portmaster: guest CPU state, memory map
//! constants, AOT code cache format (`toluca_code.bin`), and host-bridge table.

pub mod aot;
pub mod bridge;
pub mod cpu;
pub mod memory;
pub mod pe;
pub mod sha1;

pub use aot::{AOT_FORMAT_VERSION, AOT_MAGIC, AotHeader, AotManifest, AotWriter};
pub use bridge::HostBridge;
pub use cpu::GuestCpuState;
pub use memory::GuestAddr;
