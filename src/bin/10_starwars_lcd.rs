//! Demo 10: the ASCII Star Wars movie from towel.blinkenlights.nl, over
//! telnet, rendered on the 800x480 LCD.
//!
//! A telnet task (Wi-Fi credentials as in demo 04) feeds the stream through
//! the `libmudtelnet-rs` parser into an 80x24 terminal emulator. The main task
//! draws the changed characters with the 10x20 font, which fills the panel
//! exactly. Status messages are shown on the panel as well as on the console.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_net::{Stack, dns::DnsQueryType, tcp::TcpSocket};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{pixelcolor::Rgb565, prelude::RgbColor};
use embedded_io_async::Write;
use esp_backtrace as _;
use esp_hal::{rng::Rng, timer::timg::TimerGroup};
use libmudtelnet_rs::{Parser, events::TelnetEvents};
use log::{error, info, warn};
use s31_rust_examples::{
    board,
    display::{self, Display},
    lcd_pins, net,
    terminal::{Grid, Terminal, TerminalView},
};

esp_bootloader_esp_idf::esp_app_desc!();

const HOST: &str = "towel.blinkenlights.nl";
const PORT: u16 = 23;
/// The server sends frames continuously; silence this long means a dead link.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(10);
const FOREGROUND: Rgb565 = Rgb565::new(26, 54, 26);
const BACKGROUND: Rgb565 = Rgb565::BLACK;

static TERMINAL: Mutex<CriticalSectionRawMutex, RefCell<Terminal>> =
    Mutex::new(RefCell::new(Terminal::new()));

/// Replaces the screen with a status message, logs it too.
fn status(text: &str) {
    info!("{}", text.trim_end());
    TERMINAL.lock(|t| {
        let mut t = t.borrow_mut();
        t.reset();
        t.write_str("\n  ESP32-S31-Korvo-1 | Rust no_std | telnet ");
        t.write_str(HOST);
        t.write_str("\n\n  ");
        t.write_str(text);
    });
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = board::init();
    // The Wi-Fi driver needs about 60 KiB of internal RAM.
    esp_alloc::heap_allocator!(size: 96 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    info!("=== Demo 10: Star Wars over telnet on the LCD ===");
    // Only the frame buffer comes from PSRAM: keeping the Wi-Fi heap in
    // internal RAM keeps Wi-Fi traffic off the PSRAM bus the LCD depends on.
    let framebuffer = board::psram_reserve(peripherals.PSRAM, display::FRAME_BYTES);
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
                Timer::after_secs(60).await;
            }
        }
    };
    let mut view = TerminalView::new(FOREGROUND, BACKGROUND);
    let mut grid: Grid = [[b' '; 80]; 24];

    match net::credentials() {
        Some((ssid, password)) => {
            status("Connecting to Wi-Fi...");
            // Show the status before the (blocking) Wi-Fi start-up.
            draw(&mut view, &mut grid, &mut display).await;
            let config = net::station_config(ssid, password).expect("SSID or password too long");
            let (controller, interface) =
                net::init_wifi(peripherals.WIFI, Some(config)).expect("Wi-Fi init");
            let rng = Rng::new();
            let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
            let stack = net::start(&spawner, controller, interface, seed);
            spawner.spawn(telnet_task(stack).expect("spawn telnet task"));
        }
        None => status(
            "WIFI_SSID was not set at build time.\n  See .cargo/wifi.toml.example and rebuild.",
        ),
    }

    let mut frames: u32 = 0;
    let mut window = Instant::now();
    loop {
        draw(&mut view, &mut grid, &mut display).await;
        frames += 1;
        if window.elapsed() >= Duration::from_secs(30) {
            info!(
                "display: {} fps, {} resyncs so far",
                frames / 30,
                display.resyncs()
            );
            frames = 0;
            window = Instant::now();
        }
    }
}

/// Draws the changed part of the terminal and presents one frame.
async fn draw(view: &mut TerminalView, grid: &mut Grid, display: &mut Display<'_>) {
    // Copy under the lock, draw without it.
    TERMINAL.lock(|t| grid.copy_from_slice(t.borrow().grid()));
    view.draw(grid, &mut display.canvas());
    if let Err(e) = display.present().await {
        error!("display frame failed: {e:?}");
    }
}

#[embassy_executor::task]
#[allow(
    clippy::large_stack_frames,
    reason = "the socket buffers live in the task's statically allocated future"
)]
async fn telnet_task(stack: Stack<'static>) {
    let mut rx_buffer = [0u8; 4096];
    let mut tx_buffer = [0u8; 512];

    status("Waiting for the network...");
    stack.wait_config_up().await;
    loop {
        match play(stack, &mut rx_buffer, &mut tx_buffer).await {
            Ok(seconds) => {
                info!("Connection closed by server after {seconds} s");
                status("The server closed the connection.");
            }
            Err(e) => {
                warn!("Telnet session failed: {e}");
                status(e);
            }
        }
        TERMINAL.lock(|t| t.borrow_mut().write_str("\n\n  Reconnecting in 10 s..."));
        Timer::after(RETRY_DELAY).await;
    }
}

/// Runs one telnet session; returns its length in seconds.
#[allow(
    clippy::large_stack_frames,
    reason = "the TCP connect/read futures are large; awaited from one task"
)]
async fn play(
    stack: Stack<'_>,
    rx_buffer: &mut [u8],
    tx_buffer: &mut [u8],
) -> Result<u64, &'static str> {
    status("Resolving host name...");
    let addr = *stack
        .dns_query(HOST, DnsQueryType::A)
        .await
        .map_err(|_| "DNS lookup failed.")?
        .first()
        .ok_or("DNS lookup returned no address.")?;

    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    socket.set_timeout(Some(STALL_TIMEOUT));
    status("Connecting...");
    socket
        .connect((addr, PORT))
        .await
        .map_err(|_| "TCP connect failed.")?;
    info!("Connected to {addr}:{PORT}");
    TERMINAL.lock(|t| t.borrow_mut().reset());

    let mut parser = Parser::new();
    let started = Instant::now();
    let mut buf = [0u8; 1024];
    loop {
        let n = socket
            .read(&mut buf)
            .await
            .map_err(|_| "The telnet stream broke off.")?;
        if n == 0 {
            return Ok(started.elapsed().as_secs());
        }
        for event in parser.receive(&buf[..n]) {
            match event {
                TelnetEvents::DataReceive(data) => TERMINAL.lock(|t| t.borrow_mut().feed(&data)),
                // Option replies generated by the parser (it refuses everything).
                TelnetEvents::DataSend(data) => {
                    socket
                        .write_all(&data)
                        .await
                        .map_err(|_| "Sending a telnet reply failed.")?;
                }
                _ => {}
            }
        }
    }
}
