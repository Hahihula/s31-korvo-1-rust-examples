//! Demo 05: the ASCII Star Wars movie from towel.blinkenlights.nl, over telnet,
//! printed to the serial console.
//!
//! Connects to Wi-Fi (credentials as in demo 04), opens a TCP connection to
//! port 23, runs the stream through the `libmudtelnet-rs` parser and writes
//! the payload, including its ANSI cursor codes, straight to the UART. The
//! terminal running `espflash monitor` therefore plays the animation. Telnet
//! option requests are refused by the parser's default reply.

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
use embassy_net::{Stack, dns::DnsQueryType, tcp::TcpSocket};
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::Write;
use esp_backtrace as _;
use esp_hal::{rng::Rng, timer::timg::TimerGroup};
use esp_println::Printer;
use libmudtelnet_rs::{Parser, events::TelnetEvents};
use log::{error, info, warn};
use s31_rust_examples::{board, net};

esp_bootloader_esp_idf::esp_app_desc!();

const HOST: &str = "towel.blinkenlights.nl";
const PORT: u16 = 23;
/// The server sends frames continuously; silence this long means a dead link.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_DELAY: Duration = Duration::from_secs(10);

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

    info!("=== Demo 05: Star Wars over telnet ({HOST}:{PORT}) ===");
    let Some((ssid, password)) = net::credentials() else {
        error!("WIFI_SSID was not set at build time; see .cargo/wifi.toml.example");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    };

    let config = net::station_config(ssid, password).expect("SSID or password too long");
    let (controller, interface) =
        net::init_wifi(peripherals.WIFI, Some(config)).expect("Wi-Fi init");
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let stack = net::start(&spawner, controller, interface, seed);

    info!("Waiting for network...");
    stack.wait_config_up().await;
    if let Some(config) = stack.config_v4() {
        info!("IP address {}", config.address);
    }
    info!("Make the terminal at least 80x24. Press Ctrl+C to quit the monitor.");

    let mut rx_buffer = [0u8; 4096];
    let mut tx_buffer = [0u8; 512];
    loop {
        match play(stack, &mut rx_buffer, &mut tx_buffer).await {
            Ok(stats) => info!(
                "Connection closed by server after {} s, {} bytes",
                stats.seconds, stats.bytes
            ),
            Err(SessionError::Dns) => warn!("DNS lookup of {HOST} failed"),
            Err(SessionError::Connect(e)) => warn!("TCP connect failed: {e:?}"),
            Err(SessionError::Io(e)) => warn!("Telnet stream ended: {e:?}"),
        }
        info!("Reconnecting in {} s...", RETRY_DELAY.as_secs());
        Timer::after(RETRY_DELAY).await;
    }
}

enum SessionError {
    Dns,
    Connect(embassy_net::tcp::ConnectError),
    Io(embassy_net::tcp::Error),
}

struct SessionStats {
    seconds: u64,
    bytes: usize,
}

/// Runs one telnet session until the server closes it or the link stalls.
#[allow(
    clippy::large_stack_frames,
    reason = "the TCP connect/read futures are large; awaited once from main"
)]
async fn play(
    stack: Stack<'_>,
    rx_buffer: &mut [u8],
    tx_buffer: &mut [u8],
) -> Result<SessionStats, SessionError> {
    let addr = *stack
        .dns_query(HOST, DnsQueryType::A)
        .await
        .map_err(|_| SessionError::Dns)?
        .first()
        .ok_or(SessionError::Dns)?;

    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    socket.set_timeout(Some(STALL_TIMEOUT));
    info!("Connecting to {addr}:{PORT}...");
    socket
        .connect((addr, PORT))
        .await
        .map_err(SessionError::Connect)?;
    info!("Connected, the movie starts in a moment.");

    let mut parser = Parser::new();
    let started = Instant::now();
    let mut bytes = 0;
    let mut buf = [0u8; 1024];
    loop {
        let n = socket.read(&mut buf).await.map_err(SessionError::Io)?;
        if n == 0 {
            return Ok(SessionStats {
                seconds: started.elapsed().as_secs(),
                bytes,
            });
        }
        bytes += n;

        for event in parser.receive(&buf[..n]) {
            match event {
                TelnetEvents::DataReceive(data) => Printer::write_bytes(&data),
                // Option replies generated by the parser (it refuses everything).
                TelnetEvents::DataSend(data) => {
                    socket.write_all(&data).await.map_err(SessionError::Io)?;
                }
                _ => {}
            }
        }
    }
}
