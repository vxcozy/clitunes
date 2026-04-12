//! Bridge between librespot's `Sink` trait and clitunes' `PcmWriter` trait.
//!
//! [`SpotifySink`] receives decoded PCM from librespot at 44100 Hz,
//! resamples to 48000 Hz via rubato, and pushes `StereoFrame` slices
//! through an `mpsc::SyncSender` to the blocking source thread.
