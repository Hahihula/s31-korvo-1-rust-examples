//! ES8389 audio codec (the ESP32-S31-Korvo-1's microphone ADC), recording
//! side only.
//!
//! A port of Espressif's `esp_codec_dev` driver
//! (esp-adf `components/esp_codec_dev/device/es8389/es8389.c`, Apache-2.0),
//! configured as Espressif's Korvo-1 BSP does (`bsp_audio.c`): codec as I2S
//! slave, no MCLK (the codec clocks itself from SCLK), 16-bit Philips I2S.
//! One difference: `no_dac_ref = true`. With the BSP default the right ADC
//! channel carries a DAC loopback for echo cancellation and the codec expects
//! SCLK = 64 x fs; here both channels are microphones (left MIC1, right MIC2)
//! and SCLK = 32 x fs, which is what 16-bit stereo I2S provides.

use embassy_time::{Duration, Timer};
use embedded_hal_async::i2c::I2c;

/// 7-bit I2C address (schematic lists 8-bit 0x20).
pub const ADDR: u8 = 0x10;

/// PGA input selection (REG0x72/0x73 bits 6:4): differential MIC1P-MIC1N on
/// the left ADC, MIC2N-MIC2P on the right.
const ADCL_INSEL: u8 = 1;
const ADCR_INSEL: u8 = 1;

/// Microphone PGA gain, 0 to 36.5 dB in 3 dB steps (driver's enum values).
#[derive(Clone, Copy, Debug)]
pub struct Gain(u8);

impl Gain {
    pub fn from_db(db: f32) -> Self {
        // 0 dB = 0, then 3.5 dB, 6.5 dB, ... 36.5 dB = 14 (es8389_reg.h).
        if db < 3.5 {
            Gain(0)
        } else {
            Gain((((db - 3.5) / 3.0) as u8 + 1).min(14))
        }
    }
    pub fn db(self) -> f32 {
        if self.0 == 0 {
            0.0
        } else {
            3.5 + 3.0 * (self.0 - 1) as f32
        }
    }
}

pub struct Es8389<I> {
    i2c: I,
}

impl<I: I2c> Es8389<I> {
    pub fn new(i2c: I) -> Self {
        Es8389 { i2c }
    }

    async fn w(&mut self, reg: u8, v: u8) -> Result<(), I::Error> {
        self.i2c.write(ADDR, &[reg, v]).await
    }

    async fn r(&mut self, reg: u8) -> Result<u8, I::Error> {
        let mut b = [0u8];
        self.i2c.write_read(ADDR, &[reg], &mut b).await?;
        Ok(b[0])
    }

    async fn update(&mut self, reg: u8, mask: u8, v: u8) -> Result<(), I::Error> {
        let old = self.r(reg).await?;
        self.w(reg, (old & !mask) | (v & mask)).await
    }

    /// Chip ID registers (0xFD, 0xFE).
    pub async fn chip_id(&mut self) -> Result<[u8; 2], I::Error> {
        Ok([self.r(0xFD).await?, self.r(0xFE).await?])
    }

    /// Open (es8389_open), set 16 kHz / 16 bit (es8389_set_fs), start the ADC
    /// (es8389_enable) and set the microphone gain. SCLK and LRCK must already
    /// be running.
    pub async fn init(&mut self, gain: Gain) -> Result<(), I::Error> {
        // es8389_open, register by register.
        for &(reg, v) in &[
            (0xF3, 0x00),
            (0x00, 0x7E),
            (0xF3, 0x38),
            (0x24, 0x64),
            (0x25, 0x04),
            (0x45, 0x03),
            (0x60, 0x2A),
            (0x61, 0xC9),
            (0x62, 0x4F),
            (0x63, 0x06),
            (0x6B, 0x00),
            (0x6D, 0x16),
            (0x6E, 0xAA),
            (0x6F, 0x66),
            (0x70, 0x99),
            (0x23, 0x00),
            (0x72, ADCL_INSEL << 4),
            (0x73, ADCR_INSEL << 4),
            (0x10, 0xC4),
            (0x01, 0x08),
            (0xF1, 0x00),
            (0x12, 0x01),
            (0x13, 0x01),
            (0x14, 0x01),
            (0x15, 0x01),
            (0x16, 0x35),
            (0x17, 0x09),
            (0x18, 0x91),
            (0x19, 0x28),
            (0x1A, 0x01),
            (0x1B, 0x01),
            (0x1C, 0x11),
            (0x2A, 0x00),
            (0x20, 0x60), // S16_LE, I2S
            (0x40, 0x60),
            (0xF0, 0x01),
            (0x02, 0x00),
            (0x04, 0x00),
            (0x05, 0x10),
            (0x06, 0x00),
            (0x07, 0xC0),
            (0x08, 0x00),
            (0x09, 0xC0),
            (0x0A, 0x80),
            (0x0B, 4),
            (0x0C, 256u16.to_be_bytes()[0]),
            (0x0D, 256u16.to_be_bytes()[1]),
            (0x0F, 0x10),
            (0x21, 0x1F),
            (0x22, 0x7F),
            (0x2F, 0xC0),
            (0x30, 0xF4),
            (0x31, 0x00),
            (0x44, 0x00),
            (0x41, 0x7F),
            (0x42, 0x7F),
            (0x43, 0x10),
            (0x49, 0x0F),
            (0x4C, 0xC0),
            (0x00, 0x00),
            (0x03, 0xC1),
            (0x00, 0x01),
            (0x4D, 0x00),
            (0x26, 191),
            (0x27, 191),
            (0x28, 191),
            (0x46, 191),
            (0x47, 191),
            (0x48, 95 << 1),
        ] {
            self.w(reg, v).await?;
        }
        self.update(0x01, 0x01, 0x00).await?; // slave
        self.update(0x02, 0xC0, 1 << 6).await?; // internal clock from SCLK (no MCLK)
        self.update(0x02, 0x02, 0x00).await?; // MCLK not inverted
        self.w(0xF0, 0x12).await?; // no DAC reference: both ADC channels are mics
        self.update(0x02, 0x01, 0x00).await?; // SCLK not inverted

        // es8389_set_fs: 16 kHz, 16 bit, SCLK = 32 x fs (coeff_div row
        // {32, 512000, 16000}).
        let c: [u8; 20] = [
            0x00, 0x55, 0x84, 0xD0, 0x01, 0xC1, 0x90, 0x00, 0x00, 0x1F, 0x7F, 0xBF, 0xC0, 0xFF,
            0x7F, 0x00, 0x12, 0x00, 0x12, 0x31,
        ];
        let reg19 = 0x0E;
        self.w(0x04, c[0]).await?;
        self.w(0x05, c[1]).await?;
        self.w(0x06, c[2]).await?;
        self.w(0x07, c[3]).await?;
        self.w(0x08, c[4]).await?;
        self.w(0x09, c[5]).await?;
        self.w(0x0A, c[6]).await?;
        self.update(0x0F, 0xC0, c[7]).await?;
        self.w(0x11, c[8]).await?;
        self.w(0x21, c[9]).await?;
        self.w(0x22, c[10]).await?;
        self.w(0x26, c[11]).await?;
        self.update(0x30, 0xC0, c[12]).await?;
        self.w(0x41, c[13]).await?;
        self.w(0x42, c[14]).await?;
        self.update(0x43, 0x81, c[15]).await?;
        self.update(0xF0, 0x73, c[16]).await?;
        self.w(0xF1, c[17]).await?;
        self.w(0x16, c[18]).await?;
        self.w(0x18, c[19]).await?;
        self.w(0x19, reg19).await?;
        self.update(0x20, 0xE0, 0x60).await?; // S16_LE
        self.update(0x40, 0xE0, 0x60).await?;
        self.update(0x0C, 0xE0, 0x00).await?; // I2S format
        self.update(0x20, 0x1C, 0x00).await?;
        self.update(0x40, 0x1C, 0x00).await?;

        // es8389_enable -> es8389_start -> es8389_set_bias_on, then unmute.
        self.w(0x4D, 0x00).await?;
        self.update(0x69, 0x20, 0x20).await?;
        self.w(0x61, 0xD9).await?;
        self.w(0x64, 0x8F).await?;
        self.w(0x10, 0xE4).await?;
        self.w(0x00, 0x01).await?;
        self.w(0x03, 0xC3).await?;
        self.w(0x24, 0x6A).await?;
        self.w(0x25, 0x0A).await?;
        self.update(0x20, 0x03, 0x00).await?; // ADC unmute
        Timer::after(Duration::from_millis(10)).await;
        self.set_gain(gain).await
    }

    /// Microphone PGA gain on both channels (es8389_apply_mic_pga_gain).
    pub async fn set_gain(&mut self, g: Gain) -> Result<(), I::Error> {
        self.w(0x72, (ADCL_INSEL << 4) | g.0).await?;
        self.w(0x73, (ADCR_INSEL << 4) | g.0).await
    }
}
