//! Board-level constants and common initialisation.
//!
//! | Function            | GPIO            |
//! |---------------------|-----------------|
//! | Shared I2C SDA/SCL  | 0 / 1           |
//! | I2S MCLK/SCLK/LRCK  | 2 / 3 / 4       |
//! | I2S DOUT/DIN        | 5 / 6           |
//! | Speaker PA enable   | 7               |
//! | RGB LED (WS2812B)   | 37              |
//! | BOOT button         | 61 (active low) |
//! | Button ladder (ADC) | 42 (ADC1_CH0)   |
//! | microSD D0-3/CLK/CMD| 20-23 / 24 / 25 |
//! | microSD power       | 39              |
//! | UART0 TX/RX         | 58 / 59         |

use esp_hal::{
    Async,
    clock::CpuClock,
    i2c::master::{Config as I2cConfig, ConfigError, I2c, Instance as I2cInstance},
    peripherals::{GPIO0, GPIO1, GPIO37, PSRAM, Peripherals, RMT},
    psram::Psram,
    rmt::Rmt,
    time::Rate,
};
use esp_hal_smartled::{RmtSmartLeds, WS2812B_TIMING, buffer_size, color_order};
use smart_leds::RGB8;

/// 7-bit addresses of the devices on the shared I2C bus (GPIO0/GPIO1).
pub mod i2c_addr {
    /// ES8389 audio codec (schematic lists 8-bit address 0x20).
    pub const ES8389: u8 = 0x10;
    /// OV3660 camera SCCB (schematic lists 8-bit address 0x78).
    /// Only answers while the camera receives XCLK on GPIO55.
    pub const OV3660: u8 = 0x3C;
    /// Touch controller on the LCD subboard (Goodix GT-series).
    pub const TOUCH: u8 = 0x14;
    /// Alternative Goodix address, selected by the INT level at reset.
    pub const TOUCH_ALT: u8 = 0x5D;
}

/// Initialises logging, the HAL with maximum CPU clock and a small internal heap.
///
/// The caller still has to start `esp_rtos` with a `TIMG0` timer.
pub fn init() -> Peripherals {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Plain RAM instead of the template's `#[esp_hal::ram(reclaimed)]`: on the
    // ESP32-S31 the reclaimed bootloader RAM starts at 0x2F06CFB0, where the ROM
    // symbol table places `g_shared_buffers`. Nothing has been seen to break
    // there, but ROM code may still use that RAM after boot.
    esp_alloc::heap_allocator!(size: 16 * 1024);

    peripherals
}

/// Creates the async I2C master for the shared bus (codec, camera, touch).
pub fn i2c<'d>(
    i2c: impl I2cInstance + 'd,
    sda: GPIO0<'d>,
    scl: GPIO1<'d>,
) -> Result<I2c<'d, Async>, ConfigError> {
    let config = I2cConfig::default().with_frequency(Rate::from_khz(100));
    Ok(I2c::new(i2c, config)?
        .with_sda(sda)
        .with_scl(scl)
        .into_async())
}

/// RMT source clock used for the WS2812B timing.
const RMT_FREQ: Rate = Rate::from_mhz(80);

/// The single WS2812B RGB LED on GPIO37, driven by RMT channel 0.
pub type RgbLed<'d> = RmtSmartLeds<'d, { buffer_size::<RGB8>(1) }, Async, RGB8, color_order::Grb>;

/// Creates the async driver for the onboard RGB LED.
///
/// GPIO37 reaches the LED through a BSS138 level shifter (3.3 V to 5 V), which
/// does not invert the signal.
pub fn rgb_led<'d>(rmt: RMT<'d>, pin: GPIO37<'d>) -> Result<RgbLed<'d>, esp_hal_smartled::Error> {
    let rmt = Rmt::new(rmt, RMT_FREQ)?.into_async();
    RmtSmartLeds::new(WS2812B_TIMING, rmt.channel0, pin, RMT_FREQ)
}

/// Maps the 16 MB PSRAM and hands the first `reserve` bytes (rounded up to a
/// 64-byte cache line) to the caller, e.g. as a DMA frame buffer. The rest of
/// PSRAM stays unused.
///
/// Call at most once (and not together with [`psram_split`]).
pub fn psram_reserve(psram: PSRAM<'static>, reserve: usize) -> &'static mut [u8] {
    psram_map(psram, reserve, false)
}

/// Like [`psram_reserve`], but adds the rest of PSRAM to the heap.
///
/// Call at most once (and not together with [`psram_reserve`]).
pub fn psram_split(psram: PSRAM<'static>, reserve: usize) -> &'static mut [u8] {
    psram_map(psram, reserve, true)
}

fn psram_map(psram: PSRAM<'static>, reserve: usize, rest_to_heap: bool) -> &'static mut [u8] {
    let psram = Psram::new(psram, Default::default());
    let (start, size) = psram.raw_parts();
    let reserve = reserve.next_multiple_of(64).min(size);

    // SAFETY: the PSRAM mapping is valid for `size` bytes for the rest of the
    // program, nothing else uses it, and the heap only gets the part after
    // `reserve`.
    unsafe {
        if rest_to_heap && size > reserve {
            esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
                start.add(reserve),
                size - reserve,
                esp_alloc::MemoryCapability::External.into(),
            ));
        }
        core::slice::from_raw_parts_mut(start, reserve)
    }
}
