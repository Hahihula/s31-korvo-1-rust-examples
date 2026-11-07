//! App-level helpers for the demos: Wi-Fi and networking (`net`) and the
//! telnet terminal emulator (`terminal`). Kept out of the `esp32-s31-korvo`
//! library so a firmware using the board doesn't pull in the network stack.

#![no_std]

pub mod net;
pub mod terminal;
