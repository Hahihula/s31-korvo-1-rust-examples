# ESP32-S31-Korvo-1 Rust examples

Bare-metal (`no_std`) Rust demos for the
[ESP32-S31-Korvo-1 V1.1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-korvo-1/user_guide.html)
development board (ESP32-S31-WROOM-3, 16 MB flash, 16 MB PSRAM), built on
[esp-hal](https://github.com/esp-rs/esp-hal) and Embassy.

## Requirements

- Rust stable with the `rust-src` component (pinned in `rust-toolchain.toml`).
- [`espflash`](https://github.com/esp-rs/espflash) 4.x.
- One USB-C cable into the **UART** port of the board, with the power switch set to ON.

## Running a demo

Every demo is its own binary in `src/bin/`. Build in release mode, which the PSRAM
driver requires:

```bash
cargo run --release --bin 00_bringup
```

The cargo runner (`.cargo/config.toml`) flashes the board through the onboard CP2102
USB-UART bridge and then opens the serial monitor. Press `Ctrl+R` to reset the board and
`Ctrl+C` to quit. If flashing fails with a "Broken pipe" error, run the command again. If
it keeps failing, hold **Boot**, tap **Reset**, then release **Boot** to enter download
mode by hand.

Log verbosity follows `ESP_LOG` (default `info`), for example
`ESP_LOG=debug cargo run --release --bin 00_bringup`.

## Project layout

| Path | Contents |
|------|----------|
| `src/board.rs` | Board pin map, I2C device addresses, common `init()`, shared I2C bus and RGB LED setup |
| `src/buttons.rs` | Debounced decoder for the four ADC-ladder buttons, plus the BOOT button (GPIO61) |
| `src/bin/NN_*.rs` | The demos |
| `src/bin/hello_wifi.rs` | The original project template (Wi-Fi controller init plus a log loop) |

## Demos

### 00 – Board bring-up

```bash
cargo run --release --bin 00_bringup
```

Checks that the basic hardware is alive:

1. Prints the chip revision and the Wi-Fi station MAC address.
2. Detects the PSRAM, writes a pattern over all 16 MB, reads it back and reports errors
   and throughput.
3. Adds the PSRAM to the heap and allocates 1 MiB from it.
4. Scans the shared I2C bus (SDA GPIO0, SCL GPIO1) and names the devices it knows.
5. Reads the product ID of the touch controller on the LCD subboard.

Expected output (verified on a board with the LCD subboard attached):

```
INFO - === ESP32-S31-Korvo-1 bring-up ===
INFO - Chip revision: v0.0
INFO - Wi-Fi STA MAC: 30:ed:a0:..:..:..
INFO - PSRAM: 16384 KiB mapped at 0x50000000
INFO - PSRAM test: 0 bad words, write 220 ms (~72 MiB/s), read 223 ms (~71 MiB/s)
INFO - Heap: 1 MiB allocation at 0x50000000 OK
INFO - Scanning I2C bus (SDA=GPIO0, SCL=GPIO1)...
INFO -   0x10: ES8389 audio codec
INFO -   0x14: touch controller (Goodix)
INFO - I2C scan: 2 device(s)
WARN -   expected OV3660 camera (needs XCLK) at 0x3C did not answer
INFO - Touch at 0x14: Goodix GT1158
INFO - Bring-up done.
INFO - alive, uptime 10 s
```

The warning about the OV3660 camera is expected. The sensor only answers on I2C while it
receives a master clock on GPIO55, which this demo does not generate. Without the LCD
subboard, the touch controller at 0x14 is missing too.

### 01 – Function buttons

```bash
cargo run --release --bin 01_buttons
```

> **Known issue:** the button presses are not detected yet. On this board the esp-hal
> ESP32-S31 ADC driver returns 0 for every conversion after the first, in esp-hal 1.2.2
> and on the current `main` branch alike. The wiring is fine: GPIO42 reads high (3.3 V)
> as a digital input. The driver is under investigation.

The PLAY, SET, VOL- and VOL+ buttons share a single ADC pin (GPIO42, ADC1 channel 0).
Each button pulls a 10 kΩ pull-up down to its own voltage:

| Button | Voltage |
|--------|---------|
| VOL+   | 0.38 V  |
| VOL-   | 0.82 V  |
| PLAY   | 1.34 V  |
| SET    | 1.87 V  |
| none   | 3.30 V  |

The demo works like this:

1. At start-up it samples the idle level and prints the ADC calibration. Keep the
   buttons released while it starts.
2. It polls every 10 ms and accepts a button once three readings in a row agree.
3. For each press it prints the button, a per-button counter, the raw ADC code and the
   estimated voltage.
4. For each release it prints how long the button was held and the counts for all
   buttons.

Expected output once the ADC works:

```
INFO - === Demo 01: buttons (ADC ladder on GPIO42) ===
INFO - ADC calibration: zero=2198 idle=4393 full_scale=4393
INFO - Press PLAY, SET, VOL- or VOL+.
INFO - PLAY pressed  #1   (raw 3090, ~1340 mV)
INFO - PLAY released after 182 ms
INFO - counts: PLAY=1 SET=0 VOL-=0 VOL+=0
```

The raw and millivolt values above are illustrative. The S31 ADC is not calibrated in
esp-hal yet, so the voltage is estimated by scaling linearly between the grounded-input
code (2198) and the idle level.

The Reset and Boot buttons are not part of the ladder. Reset restarts the chip. Boot
(GPIO61) is a strapping pin used to enter download mode.

### 02 – RGB LED

```bash
cargo run --release --bin 02_rgb_led
```

Drives the onboard WS2812B addressable LED. GPIO37 feeds it through a BSS138 level
shifter (3.3 V to 5 V). The signal is generated by RMT channel 0 using the
[`esp-hal-smartled`](https://crates.io/crates/esp-hal-smartled) crate, with gamma
correction from [`smart-leds`](https://crates.io/crates/smart-leds).

It runs four effects:

| Effect | What you see |
|--------|--------------|
| rainbow | Smooth hue cycle, about 4 s per turn |
| breathing | Fades in and out every 3 s, with a different colour each breath |
| color test | Red, green, blue, white and off, one second each. If these don't match the log, the colour order is wrong |
| candle | Warm orange flicker |

The ladder buttons can't be read yet (see demo 01), so the **BOOT** button controls
the demo instead:

- **Short press:** next effect. The first press also turns off auto-advance.
- **Long press (1 s or more):** next brightness level (8, 32, 96, 255; the default is 32).

Until BOOT is pressed for the first time, the effects advance on their own every
10 seconds. Don't hold BOOT while pressing Reset, because that puts the chip into
download mode.

Expected output:

```
INFO - === Demo 02: RGB LED (WS2812B on GPIO37 via RMT) ===
INFO - BOOT: short press = next effect, long press (>= 1 s) = brightness
INFO - effect: rainbow (brightness 32)
INFO - effect: breathing
INFO - effect: color test (red, green, blue, white, off)
INFO -   color test: red
INFO -   color test: green
INFO -   color test: blue
INFO -   color test: white
INFO -   color test: off
INFO - effect: candle
INFO - auto-advance off
INFO - effect: rainbow
INFO - brightness 96
```
