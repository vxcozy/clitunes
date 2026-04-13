//! Micro-interactions: shimmer, volume overlay, error pulse, quit fade, breathing.

pub mod breathing;
pub mod error_pulse;
pub mod quit_fade;
pub mod shimmer;
pub mod volume_overlay;

pub use breathing::BreathingAnimation;
pub use error_pulse::ErrorPulse;
pub use quit_fade::QuitFade;
pub use shimmer::ShimmerAnimation;
pub use volume_overlay::VolumeOverlay;
