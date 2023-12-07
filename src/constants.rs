pub const LOG_BYTES_IN_BYTE: u8 = 0;
pub const BYTES_IN_BYTE: usize = 1;
pub const LOG_BITS_IN_BYTE: u8 = 3;
pub const BITS_IN_BYTE: usize = 1 << LOG_BITS_IN_BYTE;

pub const LOG_BYTES_IN_GBYTE: u8 = 30;
pub const BYTES_IN_GBYTE: usize = 1 << LOG_BYTES_IN_GBYTE;

pub const LOG_BYTES_IN_MBYTE: u8 = 20;
pub const BYTES_IN_MBYTE: usize = 1 << LOG_BYTES_IN_MBYTE;

pub const LOG_BYTES_IN_KBYTE: u8 = 10;
pub const BYTES_IN_KBYTE: usize = 1 << LOG_BYTES_IN_KBYTE;

#[cfg(target_pointer_width = "32")]
pub const LOG_BYTES_IN_ADDRESS: u8 = 2;
#[cfg(target_pointer_width = "64")]
pub const LOG_BYTES_IN_ADDRESS: u8 = 3;
pub const BYTES_IN_ADDRESS: usize = 1 << LOG_BYTES_IN_ADDRESS;
pub const LOG_BITS_IN_ADDRESS: usize = LOG_BITS_IN_BYTE as usize + LOG_BYTES_IN_ADDRESS as usize;
pub const BITS_IN_ADDRESS: usize = 1 << LOG_BITS_IN_ADDRESS;

pub const LOG_BYTES_IN_WORD: u8 = LOG_BYTES_IN_ADDRESS;
pub const BYTES_IN_WORD: usize = 1 << LOG_BYTES_IN_WORD;
pub const LOG_BITS_IN_WORD: usize = LOG_BITS_IN_BYTE as usize + LOG_BYTES_IN_WORD as usize;
pub const BITS_IN_WORD: usize = 1 << LOG_BITS_IN_WORD;
