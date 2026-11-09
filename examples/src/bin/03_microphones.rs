//! Demo 03: the two microphones (ES8389 codec over I2S, 16 kHz).
//!
//! Starts the I2S clocks, checks the codec's chip ID, configures it and
//! prints the level of both microphones (RMS and peak, dBFS) twice a second.
//! VOL+ / VOL- change the microphone gain in 3 dB steps.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp32_s31_korvo::{
    audio::{AudioIn, Error},
    board,
    buttons::{Button, Buttons, Event},
    es8389::{Clock, Config, Es8389, Format, WordLength},
    gain::Gain,
    i2s_in_pins,
};
use log::{error, info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

const SAMPLE_RATE: u32 = 16_000;
/// Samples per level report (0.5 s).
const BLOCK: usize = SAMPLE_RATE as usize / 2;
/// Gain at start: speech peaks around -13 dBFS at arm's length.
const START_GAIN_DB: f32 = 33.5;

/// RMS and peak of both channels, in dBFS.
#[derive(Clone, Copy)]
struct Levels {
    rms: [f32; 2],
    peak: [f32; 2],
}

static LEVELS: Signal<CriticalSectionRawMutex, Levels> = Signal::new();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp32_s31_korvo_examples::start!(peripherals, "Demo 03: microphones (ES8389, 16 kHz)");

    // I2S first: the codec is a slave clocked from BCLK.
    match AudioIn::new(
        peripherals.I2S0,
        peripherals.DMA_CH1,
        i2s_in_pins!(peripherals),
        SAMPLE_RATE,
    ) {
        Ok(input) => spawner.spawn(mic_task(input).unwrap()),
        Err(e) => error!("I2S init failed: {e}"),
    }
    Timer::after(Duration::from_millis(20)).await;

    let mut i2c = board::i2c(peripherals.I2C0, peripherals.GPIO0, peripherals.GPIO1)
        .expect("I2C config is valid");
    // Borrowed, so the bus stays available for the camera and touch.
    let mut codec = Es8389::new(&mut i2c, board::i2c_addr::ES8389);
    match codec.chip_id().await {
        Ok(id) => info!("ES8389 chip ID {:02X} {:02X}", id[0], id[1]),
        Err(e) => error!("ES8389 did not answer: {e:?}"),
    }
    let mut gain = Gain::from_db(START_GAIN_DB);
    // 16-bit stereo I2S: BCLK = 32 x fs.
    let config = Config {
        clock: Clock::Bclk(32 * SAMPLE_RATE),
        sample_rate: SAMPLE_RATE,
        format: Format::I2s,
        word_length: WordLength::Bits16,
        dac_reference: false,
    };
    let started = match codec.init(config).await {
        Ok(()) => codec.set_mic_gain(gain.mic_gain()).await.map_err(Into::into),
        Err(e) => Err(e),
    };
    match started {
        Ok(()) => info!("ES8389 running, mic gain {} dB", gain.db()),
        Err(e) => error!("ES8389 init failed: {e:?}"),
    }
    info!("VOL+ / VOL- = mic gain");

    let mut buttons = Buttons::new(peripherals.ADC1, peripherals.GPIO42).expect("ADC init");
    loop {
        match embassy_futures::select::select(LEVELS.wait(), buttons.next_event()).await {
            embassy_futures::select::Either::First(l) => info!(
                "MIC1 {:6.1} dBFS (peak {:6.1})  MIC2 {:6.1} dBFS (peak {:6.1})",
                l.rms[0], l.peak[0], l.rms[1], l.peak[1]
            ),
            embassy_futures::select::Either::Second(Event::Pressed { button, .. }) => {
                gain = match button {
                    Button::VolUp => gain.up(),
                    Button::VolDown => gain.down(),
                    _ => continue,
                };
                match codec.set_mic_gain(gain.mic_gain()).await {
                    Ok(()) => info!("mic gain {} dB", gain.db()),
                    Err(e) => warn!("gain change failed: {e:?}"),
                }
            }
            embassy_futures::select::Either::Second(_) => {}
        }
    }
}

/// Drains the I2S ring (it holds ~128 ms) and reports levels per block.
#[embassy_executor::task]
async fn mic_task(mut input: AudioIn<'static>) {
    let mut sum_sq = [0f32; 2];
    let mut peak = [0i32; 2];
    let mut count = 0usize;
    loop {
        let result = input
            .read(|l, r| {
                for (ch, s) in [l, r].into_iter().enumerate() {
                    let v = f32::from(s);
                    sum_sq[ch] += v * v;
                    peak[ch] = peak[ch].max(i32::from(s).abs());
                }
                count += 1;
                if count == BLOCK {
                    LEVELS.signal(Levels {
                        rms: sum_sq.map(|e| dbfs(libm::sqrtf(e / BLOCK as f32))),
                        peak: peak.map(|p| dbfs(p as f32)),
                    });
                    sum_sq = [0.0; 2];
                    peak = [0; 2];
                    count = 0;
                }
            })
            .await;
        match result {
            Ok(_) => {}
            // AudioIn restarted the stream; read on at once.
            Err(Error::Dma(e)) => warn!("mic ring overflowed, restarted: {e:?}"),
            Err(e) => {
                error!("mic stream stopped: {e}");
                return;
            }
        }
    }
}

fn dbfs(v: f32) -> f32 {
    20.0 * libm::log10f(v.max(1.0) / 32768.0)
}
