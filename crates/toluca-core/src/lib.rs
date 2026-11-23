//! Shared ABI definitions for toluca-portmaster: guest CPU state, memory map
//! constants, AOT code cache format (`toluca_code.bin`), and host-bridge table.

pub const AOT_MAGIC: [u8; 4] = *b"TLCA";
pub const AOT_FORMAT_VERSION: u32 = 1;
