use std::io;
use std::sync::atomic::{fence, AtomicU64, Ordering};

use clitunes_core::StereoFrame;

const MAGIC: u32 = 0x434C_4952; // "CLIR"
const VERSION: u8 = 1;
const HEADER_SIZE: usize = 128;
const WRITE_SEQ_OFFSET: usize = 64;
const FRAME_SIZE: usize = std::mem::size_of::<StereoFrame>();

const _: () = assert!(FRAME_SIZE == 8);
const _: () = assert!(WRITE_SEQ_OFFSET.is_multiple_of(8));

pub fn region_size(capacity_frames: u32) -> usize {
    HEADER_SIZE + (capacity_frames as usize) * FRAME_SIZE
}

#[derive(Debug, Clone, Copy)]
pub struct Overrun {
    pub lost_frames: u64,
}

impl std::fmt::Display for Overrun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "overrun: {} frames lost", self.lost_frames)
    }
}

impl std::error::Error for Overrun {}

// ---------------------------------------------------------------------------
// Producer
// ---------------------------------------------------------------------------

pub struct Producer {
    base: *mut u8,
    mask: u64,
    written: u64,
}

unsafe impl Send for Producer {}

impl Producer {
    /// Initialise a ring in the given memory region.
    ///
    /// # Safety
    ///
    /// `base` must point to a writable, zero-initialised region of at least
    /// `region_size(capacity)` bytes. `capacity` must be a power of two.
    pub unsafe fn init(base: *mut u8, capacity: u32, sample_rate: u32) -> Self {
        assert!(capacity.is_power_of_two());

        // Header fields (all native-endian — same-arch cross-process only).
        (base as *mut u32).write(MAGIC);
        base.add(4).write(VERSION);
        base.add(5).write(2); // channels = stereo
        (base.add(8) as *mut u32).write(sample_rate);
        (base.add(12) as *mut u32).write(capacity);

        let seq_ptr = base.add(WRITE_SEQ_OFFSET) as *mut AtomicU64;
        seq_ptr.write(AtomicU64::new(0));

        Producer {
            base,
            mask: (capacity - 1) as u64,
            written: 0,
        }
    }

    pub fn write_frames(&mut self, frames: &[StereoFrame]) -> usize {
        let data_base = unsafe { self.base.add(HEADER_SIZE) };

        for (i, frame) in frames.iter().enumerate() {
            let idx = ((self.written + i as u64) & self.mask) as usize;
            unsafe {
                let dst = data_base.add(idx * FRAME_SIZE) as *mut StereoFrame;
                dst.write(*frame);
            }
        }

        self.written += frames.len() as u64;

        let seq = unsafe { &*(self.base.add(WRITE_SEQ_OFFSET) as *const AtomicU64) };
        seq.store(self.written, Ordering::Release);

        frames.len()
    }

    pub fn written(&self) -> u64 {
        self.written
    }
}

// ---------------------------------------------------------------------------
// Consumer
// ---------------------------------------------------------------------------

pub struct Consumer {
    base: *const u8,
    capacity: u32,
    mask: u64,
    cursor: u64,
}

unsafe impl Send for Consumer {}

impl Consumer {
    /// Attach to an existing ring, starting at the current write position.
    ///
    /// # Safety
    ///
    /// `base` must point to a valid, readable ring region that was initialised
    /// by a [`Producer`].
    pub unsafe fn attach(base: *const u8) -> io::Result<Self> {
        let magic = (base as *const u32).read();
        if magic != MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad ring magic"));
        }
        let version = base.add(4).read();
        if version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported ring version",
            ));
        }
        let capacity = (base.add(12) as *const u32).read();
        if !capacity.is_power_of_two() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "capacity not power of 2",
            ));
        }

        let seq = &*(base.add(WRITE_SEQ_OFFSET) as *const AtomicU64);
        let ws = seq.load(Ordering::Acquire);

        Ok(Consumer {
            base,
            capacity,
            mask: (capacity - 1) as u64,
            cursor: ws,
        })
    }

    /// Attach at cursor position zero (for tests that want to read from the
    /// beginning of a freshly initialised ring).
    ///
    /// # Safety
    ///
    /// Same requirements as [`attach`](Self::attach).
    pub unsafe fn attach_from_start(base: *const u8) -> io::Result<Self> {
        let mut c = Self::attach(base)?;
        c.cursor = 0;
        Ok(c)
    }

    pub fn read_frames(&mut self, buf: &mut [StereoFrame]) -> Result<usize, Overrun> {
        let seq = unsafe { &*(self.base.add(WRITE_SEQ_OFFSET) as *const AtomicU64) };
        let ws = seq.load(Ordering::Acquire);

        let behind = ws.wrapping_sub(self.cursor);
        if behind > self.capacity as u64 {
            let lost = behind - self.capacity as u64;
            self.cursor = ws;
            return Err(Overrun { lost_frames: lost });
        }

        let available = behind as usize;
        if available == 0 {
            return Ok(0);
        }

        let to_read = available.min(buf.len());
        let data_base = unsafe { self.base.add(HEADER_SIZE) };

        for (i, slot) in buf.iter_mut().enumerate().take(to_read) {
            let idx = ((self.cursor + i as u64) & self.mask) as usize;
            unsafe {
                let src = data_base.add(idx * FRAME_SIZE) as *const StereoFrame;
                *slot = src.read();
            }
        }

        // Full barrier: all frame reads must complete before the re-check.
        // On aarch64 this emits DMB ISH; on x86 it compiles to nothing
        // (TSO already orders load→load) but the compiler fence still
        // prevents reordering.
        fence(Ordering::SeqCst);

        let ws2 = seq.load(Ordering::Relaxed);
        let behind2 = ws2.wrapping_sub(self.cursor);
        if behind2 > self.capacity as u64 {
            let lost = behind2 - self.capacity as u64;
            self.cursor = ws2;
            return Err(Overrun { lost_frames: lost });
        }

        self.cursor += to_read as u64;
        Ok(to_read)
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

// ---------------------------------------------------------------------------
// Heap-backed ring (single-process tests)
// ---------------------------------------------------------------------------

pub struct HeapRing {
    buf: Vec<u8>,
}

impl HeapRing {
    pub fn new(capacity_frames: u32, sample_rate: u32) -> (Self, Producer) {
        assert!(capacity_frames.is_power_of_two());
        let size = region_size(capacity_frames);
        let buf = vec![0u8; size];
        let mut ring = HeapRing { buf };
        let producer =
            unsafe { Producer::init(ring.buf.as_mut_ptr(), capacity_frames, sample_rate) };
        (ring, producer)
    }

    pub fn consumer_from_start(&self) -> io::Result<Consumer> {
        unsafe { Consumer::attach_from_start(self.buf.as_ptr()) }
    }

    pub fn consumer(&self) -> io::Result<Consumer> {
        unsafe { Consumer::attach(self.buf.as_ptr()) }
    }
}

// ---------------------------------------------------------------------------
// SHM-backed ring (cross-process)
// ---------------------------------------------------------------------------

pub struct ShmRegion {
    name: std::ffi::CString,
    ptr: *mut u8,
    len: usize,
    owner: bool,
}

unsafe impl Send for ShmRegion {}

impl ShmRegion {
    pub fn create(
        name: &str,
        capacity_frames: u32,
        sample_rate: u32,
    ) -> io::Result<(Self, Producer)> {
        use std::ffi::CString;

        assert!(capacity_frames.is_power_of_two());
        let len = region_size(capacity_frames);
        let c_name =
            CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        unsafe { libc::shm_unlink(c_name.as_ptr()) };

        let fd = unsafe {
            libc::shm_open(
                c_name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        if unsafe { libc::ftruncate(fd, len as libc::off_t) } < 0 {
            let err = io::Error::last_os_error();
            unsafe {
                libc::close(fd);
                libc::shm_unlink(c_name.as_ptr());
            }
            return Err(err);
        }

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        unsafe { libc::close(fd) };

        if ptr == libc::MAP_FAILED {
            unsafe { libc::shm_unlink(c_name.as_ptr()) };
            return Err(io::Error::last_os_error());
        }

        let producer = unsafe { Producer::init(ptr as *mut u8, capacity_frames, sample_rate) };

        Ok((
            ShmRegion {
                name: c_name,
                ptr: ptr as *mut u8,
                len,
                owner: true,
            },
            producer,
        ))
    }

    pub fn open_consumer(name: &str) -> io::Result<(Self, Consumer)> {
        use std::ffi::CString;

        let c_name =
            CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let fd = unsafe { libc::shm_open(c_name.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } < 0 {
            let err = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err);
        }
        let len = stat.st_size as usize;

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        unsafe { libc::close(fd) };

        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        let consumer = unsafe { Consumer::attach(ptr as *const u8)? };

        Ok((
            ShmRegion {
                name: c_name,
                ptr: ptr as *mut u8,
                len,
                owner: false,
            },
            consumer,
        ))
    }
}

impl Drop for ShmRegion {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
            if self.owner {
                libc::shm_unlink(self.name.as_ptr());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(i: u64) -> StereoFrame {
        StereoFrame {
            l: i as f32,
            r: -(i as f32),
        }
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (ring, mut producer) = HeapRing::new(16, 48_000);
        let mut consumer = ring.consumer_from_start().unwrap();

        let frames: Vec<_> = (0..8).map(make_frame).collect();
        producer.write_frames(&frames);

        let mut buf = [StereoFrame::SILENCE; 16];
        let n = consumer.read_frames(&mut buf).unwrap();
        assert_eq!(n, 8);
        for (i, frame) in buf.iter().enumerate().take(8) {
            assert_eq!(frame.l, i as f32);
        }
    }

    #[test]
    fn read_returns_zero_when_empty() {
        let (ring, _producer) = HeapRing::new(16, 48_000);
        let mut consumer = ring.consumer_from_start().unwrap();
        let mut buf = [StereoFrame::SILENCE; 4];
        let n = consumer.read_frames(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn overrun_detected() {
        let (ring, mut producer) = HeapRing::new(4, 48_000);
        let mut consumer = ring.consumer_from_start().unwrap();

        let frames: Vec<_> = (0..8).map(make_frame).collect();
        producer.write_frames(&frames);

        let mut buf = [StereoFrame::SILENCE; 4];
        let err = consumer.read_frames(&mut buf).unwrap_err();
        assert_eq!(err.lost_frames, 4);
        assert_eq!(consumer.cursor(), 8);
    }

    #[test]
    fn consumer_attach_at_current_position() {
        let (ring, mut producer) = HeapRing::new(16, 48_000);

        let frames: Vec<_> = (0..4).map(make_frame).collect();
        producer.write_frames(&frames);

        let mut consumer = ring.consumer().unwrap();
        assert_eq!(consumer.cursor(), 4);

        let more: Vec<_> = (4..8).map(make_frame).collect();
        producer.write_frames(&more);

        let mut buf = [StereoFrame::SILENCE; 8];
        let n = consumer.read_frames(&mut buf).unwrap();
        assert_eq!(n, 4);
        assert_eq!(buf[0].l, 4.0);
        assert_eq!(buf[3].l, 7.0);
    }

    #[test]
    fn wrapping_read_across_boundary() {
        let (ring, mut producer) = HeapRing::new(4, 48_000);
        let mut consumer = ring.consumer_from_start().unwrap();

        let batch1: Vec<_> = (0..3).map(make_frame).collect();
        producer.write_frames(&batch1);
        let mut buf = [StereoFrame::SILENCE; 4];
        let n = consumer.read_frames(&mut buf).unwrap();
        assert_eq!(n, 3);

        let batch2: Vec<_> = (3..6).map(make_frame).collect();
        producer.write_frames(&batch2);
        let n = consumer.read_frames(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(buf[0].l, 3.0);
        assert_eq!(buf[1].l, 4.0);
        assert_eq!(buf[2].l, 5.0);
    }

    #[test]
    fn two_consumers_independent_cursors() {
        let (ring, mut producer) = HeapRing::new(16, 48_000);
        let mut c1 = ring.consumer_from_start().unwrap();
        let mut c2 = ring.consumer_from_start().unwrap();

        let frames: Vec<_> = (0..4).map(make_frame).collect();
        producer.write_frames(&frames);

        let mut buf1 = [StereoFrame::SILENCE; 2];
        let mut buf2 = [StereoFrame::SILENCE; 4];
        let n1 = c1.read_frames(&mut buf1).unwrap();
        let n2 = c2.read_frames(&mut buf2).unwrap();

        assert_eq!(n1, 2);
        assert_eq!(n2, 4);
        assert_eq!(c1.cursor(), 2);
        assert_eq!(c2.cursor(), 4);
    }

    #[test]
    fn shm_create_and_open() {
        let name = format!("/clitunes-test-{}", std::process::id());
        let (_region, mut producer) = ShmRegion::create(&name, 16, 48_000).unwrap();
        let (_region2, mut consumer) = ShmRegion::open_consumer(&name).unwrap();

        let frames: Vec<_> = (0..4).map(make_frame).collect();
        producer.write_frames(&frames);

        let mut buf = [StereoFrame::SILENCE; 8];
        let n = consumer.read_frames(&mut buf).unwrap();
        assert_eq!(n, 4);
        assert_eq!(buf[0].l, 0.0);
        assert_eq!(buf[3].l, 3.0);
    }

    #[test]
    fn region_size_matches_expected() {
        assert_eq!(region_size(16), HEADER_SIZE + 16 * FRAME_SIZE);
        assert_eq!(region_size(1024), HEADER_SIZE + 1024 * 8);
    }
}
