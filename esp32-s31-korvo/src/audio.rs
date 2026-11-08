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

/// Errors from the audio input.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The I2S configuration was rejected.
    Config(esp_hal::i2s::master::ConfigError),
    /// Starting the transfer failed.
    I2s(I2sError),
    /// The DMA stopped (the ring overflowed). The stream has been restarted;
    /// the next read works again.
    Dma(esp_hal::dma::DmaError),
    /// Restarting after a DMA error failed; the stream is gone.
    Stopped,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Config(e) => write!(f, "I2S configuration rejected: {e:?}"),
            Error::I2s(e) => write!(f, "I2S transfer failed to start: {e:?}"),
            Error::Dma(e) => write!(f, "I2S DMA stopped and was restarted: {e:?}"),
            Error::Stopped => f.write_str("I2S stream stopped"),
        }
    }
}

impl core::error::Error for Error {}

/// The I2S pins for recording; [`i2s_in_pins!`](crate::i2s_in_pins) fills
/// it in from the peripherals.
pub struct I2sInPins<'d> {
    /// Bit clock (SCLK), GPIO3.
    pub bclk: GPIO3<'d>,
    /// Word select (LRCK), GPIO4.
    pub ws: GPIO4<'d>,
    /// Data from the codec (ASDOUT), GPIO6.
    pub din: GPIO6<'d>,
}

/// [`I2sInPins`] taken out of a `Peripherals` variable:
/// `AudioIn::new(p.I2S0, p.DMA_CH1, i2s_in_pins!(p), 16_000)`.
#[macro_export]
macro_rules! i2s_in_pins {
    ($p:ident) => {
        $crate::audio::I2sInPins {
            bclk: $p.GPIO3,
            ws: $p.GPIO4,
            din: $p.GPIO6,
        }
    };
}

/// A running I2S input stream.
pub struct AudioIn<'d> {
    /// `None` only if restarting after an error failed.
    transfer: Option<I2sRxDmaTransfer<'d, Async, DmaRxStreamBuf>>,
    /// Bytes of a frame that was split between two DMA descriptors.
    partial: [u8; FRAME_BYTES],
    partial_len: usize,
}

impl<'d> AudioIn<'d> {
    /// Starts BCLK/WS and the receive stream. Initialise the ES8389 after
    /// this: it needs the clocks.
    ///
    /// # Errors
    ///
    /// If the I2S configuration is rejected or the transfer can't start.
    pub fn new(
        i2s: I2S0<'d>,
        dma: DMA_CH1<'d>,
        pins: I2sInPins<'d>,
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
            transfer: Some(transfer),
            partial: [0; FRAME_BYTES],
            partial_len: 0,
        })
    }

    /// Waits for recorded data and passes each frame (left, right) to
    /// `frame`. Call often enough that the ring (~128 ms) never fills.
    ///
    /// # Errors
    ///
    /// If the ring overflowed, the DMA stops. The stream is then restarted
    /// and [`Error::Dma`] returned, so the caller can note the gap; the next
    /// call records again. [`Error::Stopped`] if restarting failed.
    pub async fn read(&mut self, mut frame: impl FnMut(i16, i16)) -> Result<usize, Error> {
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        if let Err(e) = transfer.wait_for_available_async().await {
            self.restart()?;
            return Err(Error::Dma(e));
        }
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        let emit = |f: &[u8; FRAME_BYTES], frame: &mut dyn FnMut(i16, i16)| {
            frame(
                i16::from_le_bytes([f[0], f[1]]),
                i16::from_le_bytes([f[2], f[3]]),
            );
        };
        let mut frames = 0;
        loop {
            let data = transfer.peek();
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
            let (whole, tail) = rest.as_chunks::<FRAME_BYTES>();
            for f in whole {
                emit(f, &mut frame);
            }
            frames += whole.len();
            self.partial[self.partial_len..self.partial_len + tail.len()].copy_from_slice(tail);
            self.partial_len += tail.len();
            // Always consume everything, or the ring stops draining.
            transfer.consume(len);
        }
        Ok(frames)
    }

    /// Stops the dead transfer and starts a new one on the same ring. A
    /// frame split across the gap is dropped.
    fn restart(&mut self) -> Result<(), Error> {
        let transfer = self.transfer.take().ok_or(Error::Stopped)?;
        let (rx, buf) = transfer.stop();
        self.partial_len = 0;
        self.transfer = Some(rx.read(buf).map_err(|(e, _, _)| Error::I2s(e))?);
        Ok(())
    }
}
