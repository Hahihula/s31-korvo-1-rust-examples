//! Demo 04: Wi-Fi station, DHCP, DNS and NTP.
//!
//! Scans for access points, connects to the network given at build time in
//! `WIFI_SSID` / `WIFI_PASSWORD`, obtains an address over DHCP, resolves a host
//! name and fetches the current time over NTP. Afterwards it prints the time
//! and address every 10 seconds and reconnects automatically if the link drops.

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
use embassy_net::{Stack, dns::DnsQueryType};
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{rng::Rng, timer::timg::TimerGroup};
use esp_radio::wifi::{WifiController, scan::ScanConfig};
use log::{error, info, warn};
use s31_rust_examples::{
    board,
    net::{self, DateTime},
};

esp_bootloader_esp_idf::esp_app_desc!();

const NTP_SERVER: &str = "pool.ntp.org";
const DNS_TEST_HOST: &str = "towel.blinkenlights.nl";

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

    info!("=== Demo 04: Wi-Fi, DHCP, DNS, NTP ===");

    let Some((ssid, password)) = net::credentials() else {
        // Without credentials only the scan is useful.
        let (mut controller, _interface) =
            net::init_wifi(peripherals.WIFI, None).expect("Wi-Fi init");
        scan(&mut controller, None).await;
        error!("WIFI_SSID was not set at build time; rebuild with:");
        error!("  WIFI_SSID=<name> WIFI_PASSWORD=<password> cargo run --release --bin 04_wifi");
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    };

    let config = net::station_config(ssid, password).expect("SSID or password too long");
    let (mut controller, interface) =
        net::init_wifi(peripherals.WIFI, Some(config)).expect("Wi-Fi init");
    info!(
        "Station configured for '{ssid}' ({})",
        if password.is_empty() {
            "open"
        } else {
            "WPA2-Personal"
        }
    );
    scan(&mut controller, Some(ssid)).await;

    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let stack = net::start(&spawner, controller, interface, seed);

    info!("Waiting for DHCP...");
    stack.wait_config_up().await;
    print_config(stack);

    match stack.dns_query(DNS_TEST_HOST, DnsQueryType::A).await {
        Ok(addrs) => info!("DNS: {DNS_TEST_HOST} -> {addrs:?}"),
        Err(e) => warn!("DNS lookup of {DNS_TEST_HOST} failed: {e:?}"),
    }

    // Unix time at the moment `synced_at` was taken.
    let mut clock: Option<(u64, Instant)> = None;
    loop {
        if clock.is_none() {
            match net::ntp_time(stack, NTP_SERVER).await {
                Ok(unix) => {
                    info!("NTP: {} ({unix})", DateTime::from_unix(unix));
                    clock = Some((unix, Instant::now()));
                }
                Err(e) => warn!("NTP query to {NTP_SERVER} failed: {e:?}"),
            }
        }

        Timer::after(Duration::from_secs(10)).await;

        if !stack.is_config_up() {
            warn!("network down, waiting for DHCP...");
            stack.wait_config_up().await;
            print_config(stack);
        }
        if let Some((unix, synced_at)) = clock {
            let now = unix + synced_at.elapsed().as_secs();
            match stack.config_v4() {
                Some(config) => info!("{} | IP {}", DateTime::from_unix(now), config.address),
                None => info!("{} | no IP", DateTime::from_unix(now)),
            }
        }
    }
}

/// Lists nearby access points and warns when `wanted` is not among them.
#[allow(
    clippy::large_stack_frames,
    reason = "the esp-radio scan future is ~1 KiB; awaited once from main"
)]
async fn scan(controller: &mut WifiController<'_>, wanted: Option<&str>) {
    info!("Scanning...");
    match controller
        .scan_async(&ScanConfig::default().with_max(15))
        .await
    {
        Ok(aps) => {
            info!("Found {} access point(s):", aps.len());
            for ap in &aps {
                info!(
                    "  {:>4} dBm  ch {:>2}  {:<32} {:?}",
                    ap.signal_strength,
                    ap.channel,
                    ap.ssid.as_str(),
                    ap.auth_method
                );
            }
            if let Some(wanted) = wanted
                && !aps.iter().any(|ap| ap.ssid.as_str() == wanted)
            {
                warn!("'{wanted}' was not found in the scan (SSIDs are case-sensitive)");
            }
        }
        Err(e) => warn!("scan failed: {e:?}"),
    }
}

fn print_config(stack: Stack<'_>) {
    if let Some(config) = stack.config_v4() {
        info!(
            "DHCP: address {}, gateway {:?}, DNS {:?}",
            config.address, config.gateway, config.dns_servers
        );
    }
}
