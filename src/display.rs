//! 4.3" 800x480 RGB565 LCD subboard (ESP32-S3-LCD-EV-Board-SUB3) on the
//! LCD_CAM DPI interface.
//!
//! The frame buffer lives in PSRAM. Its DMA descriptors form a ring and the
//! LCD runs with `next_frame_en`, so the panel is refreshed continuously at a
//! constant ~35 Hz without any CPU involvement. Drawing goes straight into the
//! buffer while it is being scanned out; [`Display::present`] writes the
//! touched rows back from the CPU cache and waits for the next VSYNC.
//!
//! Two panel/PSRAM quirks shape this design:
//!
//! - Stopping the LCD after every frame and drawing in the gap does not work:
//!   the gap varies with the drawing work and with uneven gaps the panel loses
//!   sync and fades to black.
//! - PSRAM bandwidth has little headroom. When heavy CPU traffic starves the
//!   DMA, the LCD keeps its timing while the data falls behind, and the picture
//!   stays shifted from then on. Restarting the stream every frame to realign
//!   it makes the panel flicker, so instead the stream only starts with the
//!   first [`Display::present`]: draw the initial screen before that call. After
//!   a large redraw, [`Display::resync`] realigns the stream (one short blink).
//!
//! Timing follows `SUB_BOARD3_800_480_PANEL_35HZ_RGB_TIMING` from Espressif's
//! esp-bsp: 18 MHz pixel clock, HSYNC 40/40/48, VSYNC 23/32/13
//! (pulse/back porch/front porch), data shifted on the falling edge.

use core::convert::Infallible;

use embassy_time::{Duration, with_timeout};
use embedded_graphics::{
    Pixel,
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
    primitives::Rectangle,
};
use esp_hal::{
    Async,
    dma::{
        BurstConfig, DmaDescriptor, DmaError, DmaTxBuffer, ExternalBurstConfig,
        InternalBurstConfig, Owner, Preparation,
        aligned::{DmaAlignedMut, DmaAlignedRef, InternalMemory},
        descriptor_count,
    },
    gpio::Level,
    lcd_cam::{
        LcdCam,
        lcd::{
            ClockMode, Phase, Polarity,
            dpi::{Config, ConfigError, Dpi, DpiTransfer, Format, FrameTiming},
        },
    },
    peripherals::{
        DMA_AXI_CH0, GPIO8, GPIO9, GPIO10, GPIO11, GPIO12, GPIO13, GPIO14, GPIO15, GPIO16, GPIO17,
        GPIO18, GPIO19, GPIO33, GPIO34, GPIO35, GPIO36, GPIO40, GPIO43, GPIO44, GPIO45, LCD_CAM,
    },
    time::Rate,
};
use static_cell::ConstStaticCell;

pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 480;
/// Size of one RGB565 frame in bytes.
pub const FRAME_BYTES: usize = WIDTH * HEIGHT * 2;
/// One row in bytes: 1600 = 25 cache lines, so whole rows are cache-aligned.
const ROW_BYTES: usize = WIDTH * 2;

/// Burst configuration for the frame buffer DMA: 64-byte PSRAM bursts, and
/// burst mode on for internal RAM, where the descriptors live.
const BURST: BurstConfig = BurstConfig {
    external_memory: ExternalBurstConfig::Size64,
    internal_memory: InternalBurstConfig::Enabled,
};
/// Upper bound for waiting on a VSYNC (a frame takes ~28 ms).
const VSYNC_TIMEOUT: Duration = Duration::from_millis(100);
/// Largest descriptor chunk that keeps every chunk burst-aligned.
const CHUNK: usize = 4096 - 64;
const DESCRIPTORS: usize = descriptor_count(FRAME_BYTES, CHUNK);

/// The LCD connector pins (DB0-4 blue, DB5-10 green, DB11-15 red).
pub struct LcdPins<'d> {
    pub vsync: GPIO45<'d>,
    pub hsync: GPIO44<'d>,
    pub de: GPIO43<'d>,
    pub pclk: GPIO40<'d>,
    pub b: (GPIO8<'d>, GPIO9<'d>, GPIO10<'d>, GPIO11<'d>, GPIO12<'d>),
    pub g: (
        GPIO13<'d>,
        GPIO14<'d>,
        GPIO15<'d>,
        GPIO16<'d>,
        GPIO17<'d>,
        GPIO18<'d>,
    ),
    pub r: (GPIO19<'d>, GPIO33<'d>, GPIO34<'d>, GPIO35<'d>, GPIO36<'d>),
}

/// Builds [`LcdPins`] from the HAL peripherals struct.
#[macro_export]
macro_rules! lcd_pins {
    ($p:expr) => {
        $crate::display::LcdPins {
            vsync: $p.GPIO45,
            hsync: $p.GPIO44,
            de: $p.GPIO43,
            pclk: $p.GPIO40,
            b: ($p.GPIO8, $p.GPIO9, $p.GPIO10, $p.GPIO11, $p.GPIO12),
            g: (
                $p.GPIO13, $p.GPIO14, $p.GPIO15, $p.GPIO16, $p.GPIO17, $p.GPIO18,
            ),
            r: ($p.GPIO19, $p.GPIO33, $p.GPIO34, $p.GPIO35, $p.GPIO36),
        }
    };
}

/// DPI configuration for the SUB3 panel.
pub fn panel_config() -> Config {
    Config::default()
        .with_clock_mode(ClockMode {
            polarity: Polarity::IdleLow,
            phase: Phase::ShiftLow,
        })
        .with_frequency(Rate::from_mhz(18))
        .with_format(Format {
            enable_2byte_mode: true,
            ..Default::default()
        })
        .with_timing(FrameTiming {
            horizontal_active_width: WIDTH,
            // pulse 40 + back porch 40 + active 800 + front porch 48
            horizontal_total_width: 928,
            // pulse + back porch
            horizontal_blank_front_porch: 80,
            vertical_active_height: HEIGHT,
            // pulse 23 + back porch 32 + active 480 + front porch 13
            vertical_total_height: 548,
            vertical_blank_front_porch: 55,
            hsync_width: 40,
            vsync_width: 23,
            hsync_position: 0,
        })
        .with_vsync_idle_level(Level::High)
        .with_hsync_idle_level(Level::High)
        .with_de_idle_level(Level::Low)
        .with_disable_black_region(false)
}

#[derive(Debug)]
pub enum DisplayError {
    /// The frame buffer is too small or not aligned for DMA.
    Buffer,
    /// The DPI configuration was rejected.
    Config(ConfigError),
    /// The DMA transfer could not be started.
    Dma(DmaError),
}

/// Writes CPU cache lines covering `bytes` back to memory so the DMA sees them.
fn writeback(bytes: &[u8]) {
    if let Ok(region) = DmaAlignedRef::new(bytes) {
        region.writeback();
    }
}

/// A frame buffer whose DMA descriptors are linked into a ring, so the DMA
/// never finishes and the LCD can scan it out over and over.
pub struct RingFrameBuf {
    descriptors: DmaAlignedMut<'static, [DmaDescriptor]>,
    framebuffer: *mut u8,
    /// The first transfer writes back the whole buffer; later restarts rely
    /// on [`Display::present`] having written back the changed rows.
    started: bool,
}

// SAFETY: the descriptors cover exactly `FRAME_BYTES` of a buffer that stays
// valid for the rest of the program, every chunk is burst-aligned, and the
// owner check is disabled because the ring never hands descriptors back.
unsafe impl DmaTxBuffer for RingFrameBuf {
    type View = RingFrameBuf;
    type Final = RingFrameBuf;

    fn prepare(&mut self) -> Preparation {
        // Descriptors live in cached internal RAM, the pixels in cached PSRAM.
        self.descriptors.writeback();
        if !self.started {
            self.started = true;
            // SAFETY: the frame buffer is `FRAME_BYTES` long (checked in `new`).
            writeback(unsafe { core::slice::from_raw_parts(self.framebuffer, FRAME_BYTES) });
        }
        Preparation {
            start: self.descriptors.as_mut_ptr(),
            accesses_psram: true,
            burst_transfer: BURST,
            check_owner: Some(false),
            auto_write_back: false,
        }
    }

    fn into_view(self) -> Self::View {
        self
    }

    fn from_view(view: Self::View) -> Self::Final {
        view
    }
}

enum Scanout<'d> {
    Running(DpiTransfer<'d, RingFrameBuf, Async>),
    /// Not started yet, or a restart failed; the next `present` starts it.
    Idle(Dpi<'d, Async>, RingFrameBuf),
    /// Only while switching between the two.
    Switching,
}

pub struct Display<'d> {
    scanout: Scanout<'d>,
    framebuffer: *mut u8,
    /// Rows `start..end` written since the last `present`.
    dirty: Option<(usize, usize)>,
}

impl<'d> Display<'d> {
    /// Sets up the panel. The refresh starts with the first [`Self::present`].
    /// `framebuffer` must
    /// hold at least [`FRAME_BYTES`] bytes, start 64-byte aligned and stay
    /// valid forever; normally it is the start of PSRAM.
    ///
    /// Can only be called once: the DMA descriptors are statically allocated.
    pub fn new(
        lcd_cam: LCD_CAM<'d>,
        dma: DMA_AXI_CH0<'d>,
        pins: LcdPins<'d>,
        framebuffer: &'static mut [u8],
    ) -> Result<Self, DisplayError> {
        static DESC: ConstStaticCell<InternalMemory<[DmaDescriptor; DESCRIPTORS]>> =
            ConstStaticCell::new(InternalMemory::new([DmaDescriptor::EMPTY; DESCRIPTORS]));

        let framebuffer = framebuffer
            .get_mut(..FRAME_BYTES)
            .ok_or(DisplayError::Buffer)?;
        if !(framebuffer.as_ptr() as usize).is_multiple_of(64) {
            return Err(DisplayError::Buffer);
        }
        framebuffer.fill(0);
        let framebuffer = framebuffer.as_mut_ptr();

        let mut descriptors = DESC.take().get_mut().unsize();
        let first = descriptors.as_mut_ptr();
        for (i, desc) in descriptors.iter_mut().enumerate() {
            let offset = i * CHUNK;
            let len = CHUNK.min(FRAME_BYTES - offset);
            desc.set_owner(Owner::Dma);
            desc.set_suc_eof(false);
            desc.set_size(len);
            desc.set_length(len);
            // SAFETY: `offset + len <= FRAME_BYTES`.
            desc.buffer = unsafe { framebuffer.add(offset) };
            desc.next = if i + 1 == DESCRIPTORS {
                first
            } else {
                // SAFETY: `i + 1 < DESCRIPTORS`, still inside the array.
                unsafe { first.add(i + 1) }
            };
        }
        let ring = RingFrameBuf {
            descriptors,
            framebuffer,
            started: false,
        };

        let lcd_cam = LcdCam::new(lcd_cam).into_async();
        let dpi = Dpi::new(lcd_cam.lcd, dma, panel_config())
            .map_err(DisplayError::Config)?
            .with_vsync(pins.vsync)
            .with_hsync(pins.hsync)
            .with_de(pins.de)
            .with_pclk(pins.pclk)
            .with_data0(pins.b.0)
            .with_data1(pins.b.1)
            .with_data2(pins.b.2)
            .with_data3(pins.b.3)
            .with_data4(pins.b.4)
            .with_data5(pins.g.0)
            .with_data6(pins.g.1)
            .with_data7(pins.g.2)
            .with_data8(pins.g.3)
            .with_data9(pins.g.4)
            .with_data10(pins.g.5)
            .with_data11(pins.r.0)
            .with_data12(pins.r.1)
            .with_data13(pins.r.2)
            .with_data14(pins.r.3)
            .with_data15(pins.r.4);

        Ok(Self {
            scanout: Scanout::Idle(dpi, ring),
            framebuffer,
            dirty: None,
        })
    }

    fn pixels(&mut self) -> &mut [u8] {
        // SAFETY: the buffer is `FRAME_BYTES` long and lives forever. The DMA
        // only reads it, so concurrent CPU writes can at worst show up as
        // tearing on screen.
        unsafe { core::slice::from_raw_parts_mut(self.framebuffer, FRAME_BYTES) }
    }

    /// Raw RGB565 little-endian frame buffer. Marks the whole frame as changed.
    pub fn framebuffer(&mut self) -> &mut [u8] {
        self.dirty = Some((0, HEIGHT));
        self.pixels()
    }

    /// Drawing surface over the frame buffer. Changes become visible after the
    /// next [`Self::present`].
    pub fn canvas(&mut self) -> Canvas<'_> {
        // SAFETY: see `pixels`.
        let buf = unsafe { core::slice::from_raw_parts_mut(self.framebuffer, FRAME_BYTES) };
        Canvas {
            buf,
            dirty: &mut self.dirty,
        }
    }

    /// Makes the changes since the last call visible (cache write-back of the
    /// touched rows) and waits for the start of the next frame. The first call
    /// starts the continuous refresh.
    pub async fn present(&mut self) -> Result<(), DisplayError> {
        if let Some((start, end)) = self.dirty.take() {
            let rows = &self.pixels()[start * ROW_BYTES..end * ROW_BYTES];
            writeback(rows);
        }
        match &mut self.scanout {
            Scanout::Running(transfer) => {
                // A frame takes ~28 ms; without VSYNC the LCD has stopped.
                if with_timeout(VSYNC_TIMEOUT, transfer.wait_for_vsync())
                    .await
                    .is_err()
                {
                    log::warn!(
                        "display: no VSYNC within {} ms, restarting",
                        VSYNC_TIMEOUT.as_millis()
                    );
                    return self.restart();
                }
                Ok(())
            }
            _ => self.restart(),
        }
    }

    /// Realigns the stream with the screen after a large redraw (see the
    /// module docs). The panel blinks once.
    pub async fn resync(&mut self) -> Result<(), DisplayError> {
        if let Scanout::Running(transfer) = &mut self.scanout {
            let _ = with_timeout(VSYNC_TIMEOUT, transfer.wait_for_vsync()).await;
        }
        self.restart()
    }

    /// (Re)starts the ring transfer from the start of the frame buffer.
    fn restart(&mut self) -> Result<(), DisplayError> {
        let (dpi, ring) = match core::mem::replace(&mut self.scanout, Scanout::Switching) {
            Scanout::Running(transfer) => transfer.stop(),
            Scanout::Idle(dpi, ring) => (dpi, ring),
            Scanout::Switching => unreachable!("scan-out state is restored below"),
        };
        // `next_frame_en`: start the next frame as soon as one ends, forever.
        match dpi.send(true, ring) {
            Ok(transfer) => {
                self.scanout = Scanout::Running(transfer);
                Ok(())
            }
            Err((e, dpi, ring)) => {
                self.scanout = Scanout::Idle(dpi, ring);
                Err(DisplayError::Dma(e))
            }
        }
    }
}

/// [`DrawTarget`] over an RGB565 little-endian frame buffer that records
/// which rows it touched.
pub struct Canvas<'a> {
    buf: &'a mut [u8],
    dirty: &'a mut Option<(usize, usize)>,
}

impl Canvas<'_> {
    fn mark(&mut self, start: usize, end: usize) {
        *self.dirty = Some(match *self.dirty {
            Some((s, e)) => (s.min(start), e.max(end)),
            None => (start, end),
        });
    }

    #[inline]
    fn put(&mut self, x: usize, y: usize, raw: u16) {
        let i = (y * WIDTH + x) * 2;
        self.buf[i..i + 2].copy_from_slice(&raw.to_le_bytes());
        self.mark(y, y + 1);
    }
}

impl OriginDimensions for Canvas<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for Canvas<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if let (Ok(x), Ok(y)) = (usize::try_from(point.x), usize::try_from(point.y))
                && x < WIDTH
                && y < HEIGHT
            {
                self.put(x, y, RawU16::from(color).into_inner());
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        let Some(bottom_right) = area.bottom_right() else {
            return Ok(());
        };
        let bytes = RawU16::from(color).into_inner().to_le_bytes();
        let (x0, x1) = (area.top_left.x as usize, bottom_right.x as usize);
        self.mark(area.top_left.y as usize, bottom_right.y as usize + 1);
        for y in area.top_left.y as usize..=bottom_right.y as usize {
            let row = &mut self.buf[(y * WIDTH + x0) * 2..(y * WIDTH + x1 + 1) * 2];
            for px in row.as_chunks_mut::<2>().0 {
                *px = bytes;
            }
        }
        Ok(())
    }
}
