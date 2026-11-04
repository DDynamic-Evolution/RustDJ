//! Bounded stereo fan-out from the master bus. The audio callback never waits,
//! allocates, encodes, accesses files, or accesses the network here.
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

const CAPACITY: usize = 1 << 17;

pub struct PcmBuffer {
    samples: Box<[AtomicU32]>,
    write: AtomicUsize,
    read: AtomicUsize,
    // Bit 0: accepting frames; bit 1: producer currently writing one frame.
    gate: AtomicU8,
    dropped: AtomicU64,
}

impl Default for PcmBuffer {
    fn default() -> Self {
        Self {
            samples: (0..CAPACITY * 2).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            gate: AtomicU8::new(0),
            dropped: AtomicU64::new(0),
        }
    }
}

impl PcmBuffer {
    /// Called by the sole consumer before a new job starts.
    pub fn begin(&self) {
        self.stop();
        self.read
            .store(self.write.load(Ordering::Acquire), Ordering::Release);
        self.dropped.store(0, Ordering::Relaxed);
        self.gate.store(1, Ordering::Release);
    }

    /// Worker-side stop, including completion of the last in-flight audio frame.
    pub fn stop(&self) {
        self.gate.fetch_and(!1, Ordering::AcqRel);
        while self.gate.load(Ordering::Acquire) & 2 != 0 {
            std::thread::yield_now();
        }
    }

    pub fn push(&self, frame: [f32; 2]) {
        if self.gate.load(Ordering::Relaxed) == 0
            || self
                .gate
                .compare_exchange(1, 3, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) < CAPACITY {
            let index = (write & (CAPACITY - 1)) * 2;
            for (channel, sample) in frame.into_iter().enumerate() {
                let sample = if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                self.samples[index + channel].store(sample.to_bits(), Ordering::Relaxed);
            }
            self.write.store(write.wrapping_add(1), Ordering::Release);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.gate.fetch_and(!2, Ordering::Release);
    }

    pub fn drain(&self, output: &mut Vec<f32>, max_frames: usize) -> usize {
        output.clear();
        let read = self.read.load(Ordering::Relaxed);
        let available = self.write.load(Ordering::Acquire).wrapping_sub(read);
        let count = available.min(max_frames);
        for frame in 0..count {
            let index = (read.wrapping_add(frame) & (CAPACITY - 1)) * 2;
            for channel in 0..2 {
                output.push(f32::from_bits(
                    self.samples[index + channel].load(Ordering::Relaxed),
                ));
            }
        }
        self.read.store(read.wrapping_add(count), Ordering::Release);
        count
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

pub struct MasterCapture {
    pub sample_rate: u32,
    pub stream: Arc<PcmBuffer>,
    pub recording: Arc<PcmBuffer>,
}

impl MasterCapture {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            stream: Arc::default(),
            recording: Arc::default(),
        }
    }

    pub fn push(&self, frame: [f32; 2]) {
        self.stream.push(frame);
        self.recording.push(frame);
    }
}
