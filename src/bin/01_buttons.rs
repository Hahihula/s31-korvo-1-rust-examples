//! Demo 01: the four ADC ladder buttons.
//!
//! Prints the idle calibration, then every debounced press with its raw ADC
//! code, estimated voltage and a per-button counter, and every release with
//! the hold time. A summary line is printed after each release.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{
    analog::adc::{FULL_SCALE, ZERO_DIFF_CODE},
    timer::timg::TimerGroup,
};
use log::info;
use s31_rust_examples::{
    board,
    buttons::{Button, Buttons, Event},
};

esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let peripherals = board::init();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("=== Demo 01: buttons (ADC ladder on GPIO42) ===");
    let mut buttons = Buttons::new(peripherals.ADC1, peripherals.GPIO42).expect("ADC init");
    info!(
        "ADC calibration: zero={} idle={} full_scale={}",
        ZERO_DIFF_CODE,
        buttons.idle_raw(),
        FULL_SCALE
    );
    info!("Press PLAY, SET, VOL- or VOL+.");

    let mut counts = [0u32; Button::ALL.len()];
    loop {
        match buttons.next_event().await {
            Event::Pressed { button, raw, mv } => {
                counts[button.index()] += 1;
                info!(
                    "{:<4} pressed  #{:<3} (raw {raw}, ~{mv} mV)",
                    button.name(),
                    counts[button.index()]
                );
            }
            Event::Released { button, held } => {
                info!(
                    "{:<4} released after {} ms",
                    button.name(),
                    held.as_millis()
                );
                info!(
                    "counts: PLAY={} SET={} VOL-={} VOL+={}",
                    counts[Button::Play.index()],
                    counts[Button::Set.index()],
                    counts[Button::VolDown.index()],
                    counts[Button::VolUp.index()],
                );
            }
        }
    }
}
