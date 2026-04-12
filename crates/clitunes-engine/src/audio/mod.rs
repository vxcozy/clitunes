//! Audio pipeline: PCM ring, calibration tone, realfft tap, cpal output.

#[cfg(feature = "audio")]
pub mod cpal_output;
pub mod fft_tap;
pub mod ring;
pub mod tone;

#[cfg(feature = "audio")]
pub use cpal_output::{CpalOutput, CpalOutputConfig, NegotiatedFormat};
pub use fft_tap::{FftSnapshot, FftTap};
pub use ring::{PcmRing, PcmRingReader, PcmRingWriter, PcmWriter};
pub use tone::CalibrationTone;
