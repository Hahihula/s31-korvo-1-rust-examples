//! Board support for the ESP32-S31-Korvo-1 V1.1: pins, PSRAM, the 800x480
//! RGB LCD, buttons, the RGB LED and the ES8389 microphones.
//!
//! Pin assignment follows the board user guide and schematic:
//! <https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-korvo-1/user_guide.html>
//!
//! # Using it in a firmware
//!
//! - The firmware's binary crate needs a `build.rs` that passes the linker
//!   script; a dependency can't do that for it:
//!
//!   ```ignore
//!   fn main() {
//!       println!("cargo:rustc-link-arg=-Tlinkall.x");
//!   }
//!   ```
//!
//! - The ESP32-S31 support in released esp-hal crates is incomplete, so
//!   this crate is built against an esp-hal git commit. `[patch.crates-io]`
//!   only applies in the root manifest, so copy the `[patch.crates-io]`
//!   section of this repository's `Cargo.toml` into the firmware's.
//! - [`board::init`] starts the HAL; the logger ([`board::init_logger`],
//!   `log` feature) and the heap are the firmware's choice.
//!
//! # Hardware-independent parts
//!
//! [`ladder`] (button decoding and debouncing) and [`gain`] (ES8389 gain
//! steps) touch no peripheral and read no clock, so they also build for a
//! PC, where they are unit-tested:
//! `cargo test -p esp32-s31-korvo --target <host triple>`. The drivers
//! build only for the ESP32-S31.

#![no_std]
#![warn(missing_docs)]

// Hardware-independent, tested on the host.
pub mod gain;
pub mod ladder;

// Drivers.
#[cfg(target_arch = "riscv32")]
pub mod audio;
#[cfg(target_arch = "riscv32")]
pub mod board;
#[cfg(target_arch = "riscv32")]
pub mod buttons;
#[cfg(target_arch = "riscv32")]
pub mod display;
#[cfg(target_arch = "riscv32")]
pub mod es8389;
