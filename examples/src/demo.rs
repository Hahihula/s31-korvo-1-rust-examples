//! Start-up every demo shares.

/// Starts a demo: HAL at full clock, logger, an internal heap of
/// `heap` bytes if given (plain RAM, see `board::init`), esp-rtos on TIMG0,
/// then logs the title. Binds the peripherals to `$p`.
///
/// ```ignore
/// esp32_s31_korvo_examples::start!(p, "Demo 01: buttons");
/// esp32_s31_korvo_examples::start!(p, "Demo 04: Wi-Fi", heap = 96 * 1024);
/// ```
#[macro_export]
macro_rules! start {
    ($p:ident, $title:expr $(, heap = $size:expr)?) => {
        let $p = esp32_s31_korvo::board::init();
        esp32_s31_korvo::board::init_logger();
        $( esp_alloc::heap_allocator!(size: $size); )?
        let timg0 = esp_hal::timer::timg::TimerGroup::new($p.TIMG0);
        esp_rtos::start(timg0.timer0);
        log::info!("=== {} ===", $title);
    };
}
