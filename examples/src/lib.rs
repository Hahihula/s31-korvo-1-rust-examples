//! App-level helpers for the demos: start-up (`demo`), Wi-Fi and networking
//! (`net`), SNTP packets (`sntp`), dates (`time`) and the telnet terminal
//! emulator (`terminal`). Kept out of the `esp32-s31-korvo` library so a
//! firmware using the board doesn't pull in the network stack.
//!
//! `sntp`, `time` and the terminal parser have no hardware dependencies and
//! also build for a PC, where they are unit-tested:
//! `cargo test -p esp32-s31-korvo-examples --lib --target <host triple>`.

#![no_std]
#![warn(missing_docs)]

pub mod sntp;
pub mod terminal;
pub mod time;

#[cfg(target_arch = "riscv32")]
pub mod demo;
#[cfg(target_arch = "riscv32")]
pub mod net;
