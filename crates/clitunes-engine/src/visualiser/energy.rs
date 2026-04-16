use crate::audio::FftSnapshot;

pub struct EnergyTracker {
    energy: f32,
    attack: f32,
    release: f32,
    norm_divisor: f32,
}

impl EnergyTracker {
    pub fn new(attack: f32, release: f32, norm_divisor: f32) -> Self {
        Self {
            energy: 0.0,
            attack,
            release,
            norm_divisor,
        }
    }

    pub fn update(&mut self, fft: &FftSnapshot) -> f32 {
        let sum: f32 = fft.magnitudes.iter().sum();
        let norm = (sum / fft.magnitudes.len().max(1) as f32 / self.norm_divisor).min(1.0);
        if norm > self.energy {
            self.energy = self.attack * self.energy + (1.0 - self.attack) * norm;
        } else {
            self.energy = self.release * self.energy + (1.0 - self.release) * norm;
        }
        self.energy
    }

    pub fn energy(&self) -> f32 {
        self.energy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_fft() -> FftSnapshot {
        FftSnapshot::new(vec![5000.0; 64], 48_000, 128)
    }

    fn silent_fft() -> FftSnapshot {
        FftSnapshot::new(vec![0.0; 64], 48_000, 128)
    }

    #[test]
    fn loud_input_drives_energy_up() {
        let mut tracker = EnergyTracker::new(0.5, 0.88, 500.0);
        assert_eq!(tracker.energy(), 0.0);
        let fft = loud_fft();
        for _ in 0..20 {
            tracker.update(&fft);
        }
        assert!(
            tracker.energy() > 0.5,
            "energy should rise, got {}",
            tracker.energy()
        );
    }

    #[test]
    fn silent_input_decays_energy() {
        let mut tracker = EnergyTracker::new(0.5, 0.88, 500.0);
        let loud = loud_fft();
        for _ in 0..20 {
            tracker.update(&loud);
        }
        let peak = tracker.energy();
        let silent = silent_fft();
        for _ in 0..50 {
            tracker.update(&silent);
        }
        assert!(
            tracker.energy() < peak * 0.1,
            "energy should decay, got {}",
            tracker.energy()
        );
    }

    #[test]
    fn empty_magnitudes_no_panic() {
        let mut tracker = EnergyTracker::new(0.5, 0.88, 500.0);
        let fft = FftSnapshot::new(vec![], 48_000, 0);
        tracker.update(&fft);
        assert_eq!(tracker.energy(), 0.0);
    }

    #[test]
    fn higher_attack_means_slower_rise() {
        let mut fast = EnergyTracker::new(0.3, 0.88, 500.0);
        let mut slow = EnergyTracker::new(0.7, 0.88, 500.0);
        let fft = loud_fft();
        for _ in 0..5 {
            fast.update(&fft);
            slow.update(&fft);
        }
        assert!(
            fast.energy() > slow.energy(),
            "lower attack coeff should rise faster: fast={} slow={}",
            fast.energy(),
            slow.energy()
        );
    }

    #[test]
    fn higher_release_means_slower_decay() {
        let mut fast_decay = EnergyTracker::new(0.5, 0.7, 500.0);
        let mut slow_decay = EnergyTracker::new(0.5, 0.95, 500.0);
        let loud = loud_fft();
        for _ in 0..20 {
            fast_decay.update(&loud);
            slow_decay.update(&loud);
        }
        let silent = silent_fft();
        for _ in 0..20 {
            fast_decay.update(&silent);
            slow_decay.update(&silent);
        }
        assert!(
            slow_decay.energy() > fast_decay.energy(),
            "higher release should decay slower: slow={} fast={}",
            slow_decay.energy(),
            fast_decay.energy()
        );
    }

    #[test]
    fn different_norm_divisor_changes_energy_level() {
        let mut low_div = EnergyTracker::new(0.5, 0.88, 500.0);
        let mut high_div = EnergyTracker::new(0.5, 0.88, 600.0);
        let fft = FftSnapshot::new(vec![300.0; 64], 48_000, 128);
        for _ in 0..20 {
            low_div.update(&fft);
            high_div.update(&fft);
        }
        assert!(
            low_div.energy() > high_div.energy(),
            "lower divisor should give higher energy: low={} high={}",
            low_div.energy(),
            high_div.energy()
        );
    }
}
