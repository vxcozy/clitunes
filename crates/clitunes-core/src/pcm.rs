use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl PcmFormat {
    pub const CD_QUALITY: Self = Self {
        sample_rate: 44_100,
        channels: 2,
    };

    /// 48 kHz stereo — the standard studio/broadcast sample rate.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_core::PcmFormat;
    ///
    /// let fmt = PcmFormat::STUDIO;
    /// assert_eq!(fmt.sample_rate, 48_000);
    /// assert_eq!(fmt.channels, 2);
    /// ```
    pub const STUDIO: Self = Self {
        sample_rate: 48_000,
        channels: 2,
    };

    /// Returns `true` when the format has exactly two channels.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_core::PcmFormat;
    ///
    /// assert!(PcmFormat::STUDIO.is_stereo());
    ///
    /// let mono = PcmFormat { sample_rate: 44_100, channels: 1 };
    /// assert!(!mono.is_stereo());
    /// ```
    pub const fn is_stereo(&self) -> bool {
        self.channels == 2
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct StereoFrame {
    pub l: f32,
    pub r: f32,
}

impl StereoFrame {
    /// A silent frame (both channels zero).
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_core::StereoFrame;
    ///
    /// let s = StereoFrame::SILENCE;
    /// assert_eq!(s.l, 0.0);
    /// assert_eq!(s.r, 0.0);
    /// ```
    pub const SILENCE: Self = Self { l: 0.0, r: 0.0 };

    /// Downmix to mono by averaging both channels.
    ///
    /// # Examples
    ///
    /// ```
    /// use clitunes_core::StereoFrame;
    ///
    /// let frame = StereoFrame { l: 0.6, r: 0.4 };
    /// assert!((frame.mono() - 0.5).abs() < f32::EPSILON);
    ///
    /// assert_eq!(StereoFrame::SILENCE.mono(), 0.0);
    /// ```
    #[inline]
    pub fn mono(&self) -> f32 {
        0.5 * (self.l + self.r)
    }
}
