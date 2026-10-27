//! Demo 00: board bring-up.
//!
//! Prints chip revision and MAC, tests and maps PSRAM into the heap, and scans
//! the shared I2C bus (GPIO0/GPIO1) for the codec, camera and touch controller.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::vec::Vec;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{
    Async,
    efuse::{self, InterfaceMacAddress},
    i2c::master::{AcknowledgeCheckFailedReason, Error as I2cError, I2c},
    psram::Psram,
    timer::timg::TimerGroup,
};
use log::{info, warn};
use s31_rust_examples::board::{self, i2c_addr};

esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let peripherals = board::init();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("=== ESP32-S31-Korvo-1 bring-up ===");
    let rev = efuse::chip_revision();
    info!("Chip revision: v{}.{}", rev.major, rev.minor);
    info!(
        "Wi-Fi STA MAC: {}",
        efuse::interface_mac_address(InterfaceMacAddress::Station)
    );

    let psram = Psram::new(peripherals.PSRAM, Default::default());
    test_psram(&psram);
    esp_alloc::psram_allocator!(&psram);
    check_heap();

    match board::i2c(peripherals.I2C0, peripherals.GPIO0, peripherals.GPIO1) {
        Ok(mut i2c) => {
            scan_i2c(&mut i2c).await;
            identify_touch(&mut i2c).await;
        }
        Err(e) => warn!("I2C init failed: {e:?}"),
    }

    info!("Bring-up done.");
    loop {
        Timer::after(Duration::from_secs(10)).await;
        info!("alive, uptime {} s", Instant::now().as_secs());
    }
}

/// Writes an address-derived pattern over the whole PSRAM and reads it back.
fn test_psram(psram: &Psram) {
    let (start, size) = psram.raw_parts();
    info!("PSRAM: {} KiB mapped at {:p}", size / 1024, start);
    if size == 0 {
        warn!("PSRAM not detected");
        return;
    }

    let words = size / 4;
    let base = start.cast::<u32>();
    let pattern = |i: usize| (i as u32).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_5A5A;

    let t0 = Instant::now();
    for i in 0..words {
        // SAFETY: `i` stays inside the mapped PSRAM region, which nothing else uses yet.
        unsafe { base.add(i).write_volatile(pattern(i)) };
    }
    let write_ms = t0.elapsed().as_millis();

    let t0 = Instant::now();
    let errors = (0..words)
        // SAFETY: as above.
        .filter(|&i| unsafe { base.add(i).read_volatile() } != pattern(i))
        .count();
    let read_ms = t0.elapsed().as_millis();

    let mib = size as u64 / (1024 * 1024);
    info!(
        "PSRAM test: {errors} bad words, write {write_ms} ms (~{} MiB/s), read {read_ms} ms (~{} MiB/s)",
        mib * 1000 / write_ms.max(1),
        mib * 1000 / read_ms.max(1),
    );
}

fn check_heap() {
    let buf: Vec<u8> = alloc::vec![0x5A; 1024 * 1024];
    let ok = buf.iter().all(|&b| b == 0x5A);
    info!(
        "Heap: 1 MiB allocation at {:p} {}",
        buf.as_ptr(),
        if ok { "OK" } else { "CORRUPT" }
    );
    drop(buf);
    info!("{}", esp_alloc::HEAP.stats());
}

#[allow(
    clippy::large_stack_frames,
    reason = "the async I2C transfer future is ~1 KiB; awaited once from main"
)]
async fn scan_i2c(i2c: &mut I2c<'_, Async>) {
    info!("Scanning I2C bus (SDA=GPIO0, SCL=GPIO1)...");
    let mut found: u128 = 0;
    for addr in 0x08u8..0x78 {
        let mut byte = [0u8; 1];
        match i2c.read_async(addr, &mut byte).await {
            Ok(()) => found |= 1 << addr,
            Err(I2cError::AcknowledgeCheckFailed(AcknowledgeCheckFailedReason::Address)) => {}
            // The address was acknowledged, only the read was refused.
            Err(I2cError::AcknowledgeCheckFailed(_)) => found |= 1 << addr,
            Err(e) => warn!("  0x{addr:02X}: error {e:?}"),
        }
    }

    for addr in (0x08u8..0x78).filter(|&a| found & (1 << a) != 0) {
        info!("  0x{addr:02X}: {}", device_name(addr));
    }
    info!("I2C scan: {} device(s)", found.count_ones());

    for (addr, name) in [
        (i2c_addr::ES8389, "ES8389 codec"),
        (i2c_addr::OV3660, "OV3660 camera (needs XCLK)"),
    ] {
        if found & (1 << addr) == 0 {
            warn!("  expected {name} at 0x{addr:02X} did not answer");
        }
    }
}

fn device_name(addr: u8) -> &'static str {
    match addr {
        i2c_addr::ES8389 => "ES8389 audio codec",
        i2c_addr::OV3660 => "OV3660 camera",
        i2c_addr::TOUCH | i2c_addr::TOUCH_ALT => "touch controller (Goodix)",
        _ => "unknown",
    }
}

/// Goodix controllers report an ASCII product ID at register 0x8140.
async fn identify_touch(i2c: &mut I2c<'_, Async>) {
    for addr in [i2c_addr::TOUCH, i2c_addr::TOUCH_ALT] {
        let mut id = [0u8; 4];
        if i2c
            .write_read_async(addr, &[0x81, 0x40], &mut id)
            .await
            .is_ok()
        {
            let len = id.iter().position(|&b| b == 0).unwrap_or(id.len());
            match core::str::from_utf8(&id[..len]) {
                Ok(s) => info!("Touch at 0x{addr:02X}: Goodix GT{s}"),
                Err(_) => info!("Touch at 0x{addr:02X}: product ID bytes {id:02X?}"),
            }
            return;
        }
    }
    warn!("Touch controller did not answer (is the LCD subboard attached?)");
}
