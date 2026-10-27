//! Demo 08: the 4.3" 800x480 RGB LCD subboard.
//!
//! Draws a static test pattern (colour bars, grey and RGB ramps, a one-pixel
//! border and a title) into a PSRAM frame buffer, then animates a bouncing
//! square and a frame counter on top of it. The frame rate is logged every
//! five seconds.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant};
use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};
use esp_backtrace as _;
use esp_hal::timer::timg::TimerGroup;
use log::{error, info};
use s31_rust_examples::{
    board,
    display::{self, Canvas, Display, HEIGHT, WIDTH},
    lcd_pins,
};

esp_bootloader_esp_idf::esp_app_desc!();

const BAR_COLORS: [Rgb565; 8] = [
    Rgb565::WHITE,
    Rgb565::YELLOW,
    Rgb565::CYAN,
    Rgb565::GREEN,
    Rgb565::MAGENTA,
    Rgb565::RED,
    Rgb565::BLUE,
    Rgb565::BLACK,
];
const BACKGROUND: Rgb565 = Rgb565::new(2, 4, 6);
/// Area the square bounces in (below the static pattern).
const ARENA: Rectangle = Rectangle::new(Point::new(8, 300), Size::new(784, 172));
const SQUARE: u32 = 40;

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let peripherals = board::init();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("=== Demo 08: RGB LCD 800x480 ===");
    let framebuffer = board::psram_split(peripherals.PSRAM, display::FRAME_BYTES);
    let mut display = match Display::new(
        peripherals.LCD_CAM,
        peripherals.DMA_AXI_CH0,
        lcd_pins!(peripherals),
        framebuffer,
    ) {
        Ok(display) => display,
        Err(e) => {
            error!("display init failed: {e:?}");
            loop {
                embassy_time::Timer::after_secs(60).await;
            }
        }
    };

    draw_test_pattern(&mut display.canvas());
    info!("Test pattern drawn, animating");

    let mut pos = Point::new(ARENA.top_left.x + 1, ARENA.top_left.y + 1);
    let mut velocity = Point::new(5, 3);
    let mut frames: u32 = 0;
    let mut total_frames: u32 = 0;
    let mut window_start = Instant::now();

    loop {
        let mut canvas = display.canvas();
        // Erase the old square, move it, draw it again.
        let _ = Rectangle::new(pos, Size::new(SQUARE, SQUARE))
            .into_styled(PrimitiveStyle::with_fill(BACKGROUND))
            .draw(&mut canvas);
        pos += velocity;
        let max =
            ARENA.bottom_right().unwrap_or_default() - Point::new(SQUARE as i32, SQUARE as i32);
        if pos.x <= ARENA.top_left.x || pos.x >= max.x {
            velocity.x = -velocity.x;
            pos.x = pos.x.clamp(ARENA.top_left.x, max.x);
        }
        if pos.y <= ARENA.top_left.y || pos.y >= max.y {
            velocity.y = -velocity.y;
            pos.y = pos.y.clamp(ARENA.top_left.y, max.y);
        }
        let hue = BAR_COLORS[(total_frames / 30) as usize % 7];
        let _ = Rectangle::new(pos, Size::new(SQUARE, SQUARE))
            .into_styled(PrimitiveStyle::with_fill(hue))
            .draw(&mut canvas);
        draw_counter(&mut canvas, total_frames);

        if let Err(e) = display.present().await {
            error!("frame failed: {e:?}");
        }
        frames += 1;
        total_frames = total_frames.wrapping_add(1);

        let elapsed = window_start.elapsed();
        if elapsed >= Duration::from_secs(5) {
            let fps_x10 = u64::from(frames) * 10_000 / elapsed.as_millis().max(1);
            info!("{}.{} fps", fps_x10 / 10, fps_x10 % 10);
            frames = 0;
            window_start = Instant::now();
        }
    }
}

fn draw_test_pattern(canvas: &mut Canvas<'_>) {
    let _ = canvas.clear(BACKGROUND);

    // Eight vertical colour bars across the top.
    let bar_width = (WIDTH / BAR_COLORS.len()) as u32;
    for (i, color) in BAR_COLORS.iter().enumerate() {
        let _ = Rectangle::new(
            Point::new(i as i32 * bar_width as i32, 0),
            Size::new(bar_width, 140),
        )
        .into_styled(PrimitiveStyle::with_fill(*color))
        .draw(canvas);
    }

    // Grey ramp plus red, green and blue ramps, each 32 levels.
    let ramps: [fn(u8) -> Rgb565; 4] = [
        |v| Rgb565::new(v, v * 2, v),
        |v| Rgb565::new(v, 0, 0),
        |v| Rgb565::new(0, v * 2, 0),
        |v| Rgb565::new(0, 0, v),
    ];
    let step = (WIDTH / 32) as u32;
    for (row, ramp) in ramps.iter().enumerate() {
        for level in 0..32u8 {
            let _ = Rectangle::new(
                Point::new(i32::from(level) * step as i32, 150 + row as i32 * 24),
                Size::new(step, 22),
            )
            .into_styled(PrimitiveStyle::with_fill(ramp(level)))
            .draw(canvas);
        }
    }

    // A one-pixel border shows whether the edges line up with the panel.
    let _ = Rectangle::new(Point::zero(), Size::new(WIDTH as u32, HEIGHT as u32))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
        .draw(canvas);

    let title = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    let _ = Text::with_alignment(
        "ESP32-S31-Korvo-1 | Rust no_std | 800x480 RGB565",
        Point::new(WIDTH as i32 / 2, 270),
        title,
        Alignment::Center,
    )
    .draw(canvas);
    let _ = ARENA
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::new(10, 20, 10), 1))
        .draw(canvas);
}

fn draw_counter(canvas: &mut Canvas<'_>, frame: u32) {
    let text = alloc::format!("frame {frame:>8}");
    let origin = Point::new(WIDTH as i32 - 200, 290);
    let _ = Rectangle::new(origin - Point::new(0, 16), Size::new(190, 20))
        .into_styled(PrimitiveStyle::with_fill(BACKGROUND))
        .draw(canvas);
    let _ = Text::new(
        &text,
        origin,
        MonoTextStyle::new(&FONT_10X20, Rgb565::YELLOW),
    )
    .draw(canvas);
}
