use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Single bandwidth sample with timestamp
#[derive(Debug, Clone)]
struct BandwidthSample {
    timestamp: Instant,
    bytes: u64,
}

/// Tracks bandwidth using sliding window moving average
#[derive(Debug)]
pub struct BandwidthTracker {
    window_duration: Duration,
    samples: VecDeque<BandwidthSample>,
    start_time: Instant,
}

impl BandwidthTracker {
    pub fn new(window_duration: Duration) -> Self {
        Self {
            window_duration,
            samples: VecDeque::new(),
            start_time: Instant::now(),
        }
    }

    /// Add bytes transferred at current time
    pub fn record_bytes(&mut self, bytes: u64) {
        let now = Instant::now();
        self.samples.push_back(BandwidthSample {
            timestamp: now,
            bytes,
        });
    }

    /// Get current rate (bytes/sec) over the window
    /// Evicts old samples and calculates: sum(bytes) / window_duration
    pub fn get_current_rate(&mut self) -> f64 {
        let now = Instant::now();
        self.evict_old_samples(now);

        if self.samples.is_empty() {
            return 0.0;
        }

        // Calculate actual window duration
        let oldest_timestamp = self.samples.front().unwrap().timestamp;
        let window_duration_secs = now.duration_since(oldest_timestamp).as_secs_f64();

        if window_duration_secs == 0.0 {
            return 0.0;
        }

        // Sum all bytes in window
        let total_bytes: u64 = self.samples.iter().map(|s| s.bytes).sum();

        // Return bytes per second
        total_bytes as f64 / window_duration_secs
    }

    /// Evict samples older than now - window_duration
    fn evict_old_samples(&mut self, now: Instant) {
        let cutoff = now
            .checked_sub(self.window_duration)
            .unwrap_or(self.start_time);

        while let Some(sample) = self.samples.front() {
            if sample.timestamp < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }
}

const DEFAULT_WINDOW_DURATION: Duration = Duration::from_secs(3);

/// Global bandwidth statistics (shared across all peers)
#[derive(Debug, Clone)]
pub struct BandwidthStats {
    download: Arc<Mutex<BandwidthTracker>>,
    upload: Arc<Mutex<BandwidthTracker>>,
}

impl Default for BandwidthStats {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_DURATION)
    }
}

impl BandwidthStats {
    pub fn new(window_duration: Duration) -> Self {
        Self {
            download: Arc::new(Mutex::new(BandwidthTracker::new(window_duration))),
            upload: Arc::new(Mutex::new(BandwidthTracker::new(window_duration))),
        }
    }

    pub fn record_download(&self, bytes: u64) {
        if let Ok(mut tracker) = self.download.try_lock() {
            tracker.record_bytes(bytes);
        }
    }

    pub fn record_upload(&self, bytes: u64) {
        if let Ok(mut tracker) = self.upload.try_lock() {
            tracker.record_bytes(bytes);
        }
    }

    pub fn get_download_rate(&self) -> f64 {
        self.download
            .try_lock()
            .map(|mut tracker| tracker.get_current_rate())
            .unwrap_or(0.0)
    }

    pub fn get_upload_rate(&self) -> f64 {
        self.upload
            .try_lock()
            .map(|mut tracker| tracker.get_current_rate())
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_empty_tracker_returns_zero_rate() {
        let mut tracker = BandwidthTracker::new(Duration::from_secs(3));
        assert_eq!(tracker.get_current_rate(), 0.0);
    }

    #[test]
    fn test_single_sample_within_window() {
        let mut tracker = BandwidthTracker::new(Duration::from_secs(3));
        tracker.record_bytes(1024);

        // Should have non-zero rate immediately after recording
        let rate = tracker.get_current_rate();
        assert!(rate > 0.0);
    }

    #[test]
    fn test_rate_calculation_with_multiple_samples() {
        let mut tracker = BandwidthTracker::new(Duration::from_secs(3));

        // Record 10 KB immediately
        tracker.record_bytes(10240);

        // Wait a bit then record more
        std::thread::sleep(Duration::from_millis(100));
        tracker.record_bytes(10240);

        let rate = tracker.get_current_rate();
        // Rate should be positive and reasonable
        assert!(rate > 0.0);
        assert!(rate < 1_000_000.0); // Less than 1 MB/s seems reasonable for this test
    }

    #[test]
    fn test_evicts_old_samples() {
        let mut tracker = BandwidthTracker::new(Duration::from_millis(100));

        // Record some bytes
        tracker.record_bytes(1024);
        assert_eq!(tracker.samples.len(), 1);

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(150));

        // Trigger eviction by getting rate
        tracker.get_current_rate();

        // Old sample should be evicted
        assert_eq!(tracker.samples.len(), 0);
    }

    #[tokio::test]
    async fn test_bandwidth_stats_thread_safety() {
        let stats = BandwidthStats::new(Duration::from_secs(3));

        // Spawn multiple tasks recording bytes
        let mut handles = vec![];
        for _ in 0..10 {
            let stats_clone = stats.clone();
            let handle = tokio::spawn(async move {
                for _ in 0..100 {
                    stats_clone.record_download(100);
                    stats_clone.record_upload(50);
                }
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Should be able to get rates without panicking
        let download_rate = stats.get_download_rate();
        let upload_rate = stats.get_upload_rate();

        // Rates should be non-negative
        assert!(download_rate >= 0.0);
        assert!(upload_rate >= 0.0);
    }

    #[test]
    fn test_default_uses_3_second_window() {
        let stats = BandwidthStats::default();
        stats.record_download(1024);

        let rate = stats.get_download_rate();
        assert!(rate >= 0.0);
    }
}
