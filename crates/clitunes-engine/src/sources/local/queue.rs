use std::collections::VecDeque;

use clitunes_core::Track;

pub struct Queue {
    upcoming: VecDeque<Track>,
    played: Vec<Track>,
    current: Option<Track>,
}

impl Queue {
    pub fn new(tracks: Vec<Track>) -> Self {
        Self {
            upcoming: VecDeque::from(tracks),
            played: Vec::new(),
            current: None,
        }
    }

    pub fn current(&self) -> Option<&Track> {
        self.current.as_ref()
    }

    /// Advance to the next track. Returns a reference to it.
    pub fn next(&mut self) -> Option<&Track> {
        if let Some(prev) = self.current.take() {
            self.played.push(prev);
        }
        self.current = self.upcoming.pop_front();
        self.current.as_ref()
    }

    /// Go back to the previous track. Returns a reference to it.
    pub fn prev(&mut self) -> Option<&Track> {
        if let Some(current) = self.current.take() {
            self.upcoming.push_front(current);
        }
        self.current = self.played.pop();
        self.current.as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.upcoming.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.upcoming.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_track(name: &str) -> Track {
        Track {
            path: PathBuf::from(format!("/music/{}.flac", name)),
            title: Some(name.to_string()),
            artist: None,
            album: None,
            album_artist: None,
            track_num: None,
            year: None,
            duration_secs: None,
            embedded_art: None,
        }
    }

    #[test]
    fn next_advances_through_queue() {
        let mut q = Queue::new(vec![make_track("A"), make_track("B"), make_track("C")]);
        assert!(q.current().is_none());

        assert_eq!(q.next().unwrap().display_title(), "A");
        assert_eq!(q.remaining(), 2);
        assert_eq!(q.next().unwrap().display_title(), "B");
        assert_eq!(q.next().unwrap().display_title(), "C");
        assert!(q.next().is_none());
    }

    #[test]
    fn prev_goes_back() {
        let mut q = Queue::new(vec![make_track("A"), make_track("B"), make_track("C")]);
        q.next(); // A
        q.next(); // B

        let prev = q.prev().unwrap();
        assert_eq!(prev.display_title(), "A");

        let next = q.next().unwrap();
        assert_eq!(next.display_title(), "B");
    }

    #[test]
    fn prev_at_start_returns_none() {
        let mut q = Queue::new(vec![make_track("A")]);
        q.next(); // A
                  // A is current, no played history
                  // prev should push A back to upcoming, current = None (no played)
        assert!(q.prev().is_none());
        // But A should be back in upcoming
        assert_eq!(q.next().unwrap().display_title(), "A");
    }

    #[test]
    fn empty_queue() {
        let mut q = Queue::new(vec![]);
        assert!(q.is_empty());
        assert!(q.next().is_none());
    }
}
