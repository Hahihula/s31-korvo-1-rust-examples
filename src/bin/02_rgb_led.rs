//! Demo 02: the onboard WS2812B RGB LED (GPIO37) driven by RMT.
//!
//! Cycles through four effects. The BOOT button switches them: a short press
//! selects the next effect, a long press (>= 1 s) steps the brightness. Until
//! the first press the effects advance on their own every 10 seconds.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{peripherals::GPIO61, timer::timg::TimerGroup};
use log::{info, warn};
use s31_rust_examples::{board, buttons::BootButton};
use smart_leds::{
    RGB8, SmartLedsWriteAsync, brightness, gamma,
    hsv::{Hsv, hsv2rgb},
};

esp_bootloader_esp_idf::esp_app_desc!();

const FRAME: Duration = Duration::from_millis(20);
const AUTO_ADVANCE: Duration = Duration::from_secs(10);
const LONG_PRESS: Duration = Duration::from_secs(1);
/// Brightness steps (0-255) applied after gamma correction.
const BRIGHTNESS: [u8; 4] = [8, 32, 96, 255];

#[derive(Clone, Copy)]
enum Press {
    Short,
    Long,
}

static PRESSES: Channel<CriticalSectionRawMutex, Press, 4> = Channel::new();

#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    Rainbow,
    Breathing,
    ColorTest,
    Candle,
}

impl Effect {
    const ALL: [Effect; 4] = [
        Effect::Rainbow,
        Effect::Breathing,
        Effect::ColorTest,
        Effect::Candle,
    ];

    fn name(self) -> &'static str {
        match self {
            Effect::Rainbow => "rainbow",
            Effect::Breathing => "breathing",
            Effect::ColorTest => "color test (red, green, blue, white, off)",
            Effect::Candle => "candle",
        }
    }

    fn next(self) -> Effect {
        let i = Effect::ALL.iter().position(|&e| e == self).unwrap_or(0);
        Effect::ALL[(i + 1) % Effect::ALL.len()]
    }
}

/// Frame generator state shared by the effects.
struct Renderer {
    rng: u32,
    candle: u8,
    color_step: Option<usize>,
}

const COLOR_TEST: [(&str, RGB8); 5] = [
    ("red", RGB8::new(255, 0, 0)),
    ("green", RGB8::new(0, 255, 0)),
    ("blue", RGB8::new(0, 0, 255)),
    ("white", RGB8::new(255, 255, 255)),
    ("off", RGB8::new(0, 0, 0)),
];

impl Renderer {
    fn random(&mut self) -> u32 {
        // xorshift32
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    /// Color of `effect` at `t_ms` milliseconds after the effect started.
    fn color(&mut self, effect: Effect, t_ms: u64) -> RGB8 {
        match effect {
            Effect::Rainbow => hsv2rgb(Hsv {
                hue: (t_ms / 16) as u8,
                sat: 255,
                val: 255,
            }),
            Effect::Breathing => {
                // 3 s triangle wave; the hue moves on with every breath.
                let phase = (t_ms % 3000) as u32;
                let val = if phase < 1500 { phase } else { 3000 - phase } * 255 / 1500;
                hsv2rgb(Hsv {
                    hue: ((t_ms / 3000) * 40) as u8,
                    sat: 255,
                    val: val as u8,
                })
            }
            Effect::ColorTest => {
                let step = (t_ms / 1000) as usize % COLOR_TEST.len();
                let (name, color) = COLOR_TEST[step];
                if self.color_step != Some(step) {
                    self.color_step = Some(step);
                    info!("  color test: {name}");
                }
                color
            }
            Effect::Candle => {
                // Move towards a random target to get a soft flicker.
                let target = 120 + (self.random() % 136) as u8;
                self.candle = ((u16::from(self.candle) * 3 + u16::from(target)) / 4) as u8;
                let v = u16::from(self.candle);
                RGB8::new(self.candle, (v * 90 / 255) as u8, (v * 12 / 255) as u8)
            }
        }
    }
}

#[embassy_executor::task]
async fn boot_button_task(gpio61: GPIO61<'static>) {
    let mut button = BootButton::new(gpio61);
    loop {
        let held = button.next_press().await;
        let press = if held >= LONG_PRESS {
            Press::Long
        } else {
            Press::Short
        };
        PRESSES.send(press).await;
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = board::init();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("=== Demo 02: RGB LED (WS2812B on GPIO37 via RMT) ===");
    let mut led = board::rgb_led(peripherals.RMT, peripherals.GPIO37).expect("RMT LED init");
    spawner.spawn(boot_button_task(peripherals.GPIO61).expect("spawn BOOT button task"));
    info!("BOOT: short press = next effect, long press (>= 1 s) = brightness");

    let mut renderer = Renderer {
        rng: 0x1234_5678,
        candle: 180,
        color_step: None,
    };
    let mut effect = Effect::Rainbow;
    let mut level = 1;
    let mut auto_advance = true;
    let mut started = Instant::now();
    info!(
        "effect: {} (brightness {})",
        effect.name(),
        BRIGHTNESS[level]
    );

    loop {
        let t_ms = started.elapsed().as_millis();
        let color = renderer.color(effect, t_ms);
        if let Err(e) = led
            .write(brightness(gamma([color].into_iter()), BRIGHTNESS[level]))
            .await
        {
            warn!("LED write failed: {e:?}");
        }

        let mut switch = auto_advance && started.elapsed() >= AUTO_ADVANCE;
        match select(Timer::after(FRAME), PRESSES.receive()).await {
            Either::First(()) => {}
            Either::Second(Press::Short) => {
                if auto_advance {
                    auto_advance = false;
                    info!("auto-advance off");
                }
                switch = true;
            }
            Either::Second(Press::Long) => {
                level = (level + 1) % BRIGHTNESS.len();
                info!("brightness {}", BRIGHTNESS[level]);
            }
        }

        if switch {
            effect = effect.next();
            renderer.color_step = None;
            started = Instant::now();
            info!("effect: {}", effect.name());
        }
    }
}
