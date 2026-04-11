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
    pub const STUDIO: Self = Self {
        sample_rate: 48_000,
        channels: 2,
    };

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
    pub const SILENCE: Self = Self { l: 0.0, r: 0.0 };

    #[inline]
    pub fn mono(&self) -> f32 {
        0.5 * (self.l + self.r)
    }
}
