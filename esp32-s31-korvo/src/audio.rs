//! I2S input from the ES8389 on the ESP32-S31-Korvo-1: 16-bit stereo
//! Philips frames, left = MIC1, right = MIC2. The S31 is master (BCLK GPIO3,
//! WS GPIO4, DIN GPIO6); there is no MCLK line (Espressif's BSP leaves it
//! unconnected), the codec clocks itself from BCLK.
//!
//! Same structure as `AudioIn` in the esp-vocat library.

use esp_hal::{
    Async,
    dma::DmaRxStreamBuf,
    dma_rx_stream_buffer,
    i2s::master::{Channels, DataFormat, Error as I2sError, I2s, I2sRxDmaTransfer, TdmConfig},
    peripherals::{DMA_CH1, GPIO3, GPIO4, GPIO6, I2S0},
    time::Rate,
};

/// Bytes per stereo frame (two 16-bit samples).
const FRAME_BYTES: usize = 4;
/// DMA ring for recording: 8 KiB, about 128 ms at 16 kHz.
const IN_RING_BYTES: usize = 8192;

#[derive(Debug)]
#[allow(dead_code, reason = "logged through Debug")]
pub enum Error {
    Config(esp_hal::i2s::master::ConfigError),
    I2s(I2sError),
    Dma(esp_hal::dma::DmaError),
}

pub struct I2sInPins {
    pub bclk: GPIO3<'static>,
    pub ws: GPIO4<'static>,
    pub din: GPIO6<'static>,
}

/// A running I2S input stream.
pub struct AudioIn {
    transfer: I2sRxDmaTransfer<'static, Async, DmaRxStreamBuf>,
    partial: [u8; FRAME_BYTES],
    partial_len: usize,
}

impl AudioIn {
    /// Starts BCLK/WS and the receive stream. Initialise the ES8389 after
    /// this: it needs the clocks.
    pub fn new(
        i2s: I2S0<'static>,
        dma: DMA_CH1<'static>,
        pins: I2sInPins,
        sample_rate: u32,
    ) -> Result<Self, Error> {
        let config = TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(sample_rate))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO);
        let i2s = I2s::new(i2s, dma, config)
            .map_err(Error::Config)?
            .into_async();
        let rx = i2s
            .i2s_rx
            .with_bclk(pins.bclk)
            .with_ws(pins.ws)
            .with_din(pins.din)
            .build();
        let buf = dma_rx_stream_buffer!(IN_RING_BYTES, 1024);
        let transfer = rx.read(buf).map_err(|(e, _, _)| Error::I2s(e))?;
        Ok(Self {
            transfer,
            partial: [0; FRAME_BYTES],
            partial_len: 0,
        })
    }

    /// Waits for recorded data and passes each frame (left, right) to
    /// `frame`. Call often enough that the ring (~128 ms) never fills.
    pub async fn read(&mut self, mut frame: impl FnMut(i16, i16)) -> Result<usize, Error> {
        self.transfer
            .wait_for_available_async()
            .await
            .map_err(Error::Dma)?;
        let mut frames = 0;
        let emit = |f: &[u8], frame: &mut dyn FnMut(i16, i16)| {
            frame(
                i16::from_le_bytes([f[0], f[1]]),
                i16::from_le_bytes([f[2], f[3]]),
            );
        };
        loop {
            let data = self.transfer.peek();
            if data.is_empty() {
                break;
            }
            let len = data.len();
            let mut rest = data;
            // A descriptor doesn't always end on a frame boundary.
            if self.partial_len > 0 {
                let take = (FRAME_BYTES - self.partial_len).min(rest.len());
                self.partial[self.partial_len..self.partial_len + take]
                    .copy_from_slice(&rest[..take]);
                self.partial_len += take;
                rest = &rest[take..];
                if self.partial_len == FRAME_BYTES {
                    emit(&self.partial, &mut frame);
                    frames += 1;
                    self.partial_len = 0;
                }
            }
            let whole = rest.len() / FRAME_BYTES * FRAME_BYTES;
            for f in rest[..whole].chunks_exact(FRAME_BYTES) {
                emit(f, &mut frame);
            }
            frames += whole / FRAME_BYTES;
            let tail = &rest[whole..];
            self.partial[self.partial_len..self.partial_len + tail.len()].copy_from_slice(tail);
            self.partial_len += tail.len();
            // Always consume everything, or the ring stops draining.
            self.transfer.consume(len);
        }
        Ok(frames)
    }
}
