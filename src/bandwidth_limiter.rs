use anyhow::{Result, anyhow};
use governor::{
    Quota, RateLimiter, clock::DefaultClock, state::InMemoryState, state::direct::NotKeyed,
};
use std::num::NonZeroU32;
use std::sync::Arc;

pub type GovernorRateLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

#[derive(Clone, Debug)]
pub struct BandwidthLimiter {
    pub download: Option<Arc<GovernorRateLimiter>>,
    pub upload: Option<Arc<GovernorRateLimiter>>,
}

impl BandwidthLimiter {
    pub fn new(download_bps: Option<u64>, upload_bps: Option<u64>) -> Result<Self> {
        let download = download_bps
            .map(|bps| {
                let quota = Quota::per_second(
                    NonZeroU32::new(bps as u32)
                        .ok_or_else(|| anyhow!("Download rate must be greater than 0"))?,
                );
                Ok::<_, anyhow::Error>(Arc::new(RateLimiter::direct(quota)))
            })
            .transpose()?;

        let upload = upload_bps
            .map(|bps| {
                let quota = Quota::per_second(
                    NonZeroU32::new(bps as u32)
                        .ok_or_else(|| anyhow!("Upload rate must be greater than 0"))?,
                );
                Ok::<_, anyhow::Error>(Arc::new(RateLimiter::direct(quota)))
            })
            .transpose()?;

        Ok(Self { download, upload })
    }
}

pub fn parse_bandwidth_arg(s: &str) -> Result<u64> {
    let s = s.trim().to_uppercase();

    if s.is_empty() {
        return Err(anyhow!("Bandwidth value cannot be empty"));
    }

    let (num_str, multiplier) = if let Some(stripped) = s.strip_suffix('G') {
        (stripped, 1_073_741_824u64) // 1024^3
    } else if let Some(stripped) = s.strip_suffix('M') {
        (stripped, 1_048_576u64) // 1024^2
    } else if let Some(stripped) = s.strip_suffix('K') {
        (stripped, 1024u64)
    } else {
        (s.as_str(), 1u64)
    };

    let num: u64 = num_str
        .parse()
        .map_err(|_| anyhow!("Invalid bandwidth value: {}", s))?;

    if num == 0 {
        return Err(anyhow!("Bandwidth value must be greater than 0"));
    }

    Ok(num * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bandwidth_arg_megabytes() {
        assert_eq!(parse_bandwidth_arg("5M").unwrap(), 5 * 1_048_576);
        assert_eq!(parse_bandwidth_arg("100M").unwrap(), 100 * 1_048_576);
        assert_eq!(parse_bandwidth_arg("1m").unwrap(), 1_048_576); // lowercase
    }

    #[test]
    fn test_parse_bandwidth_arg_kilobytes() {
        assert_eq!(parse_bandwidth_arg("100K").unwrap(), 100 * 1024);
        assert_eq!(parse_bandwidth_arg("512K").unwrap(), 512 * 1024);
        assert_eq!(parse_bandwidth_arg("1k").unwrap(), 1024); // lowercase
    }

    #[test]
    fn test_parse_bandwidth_arg_gigabytes() {
        assert_eq!(parse_bandwidth_arg("1G").unwrap(), 1_073_741_824);
        assert_eq!(parse_bandwidth_arg("2G").unwrap(), 2 * 1_073_741_824);
        assert_eq!(parse_bandwidth_arg("1g").unwrap(), 1_073_741_824); // lowercase
    }

    #[test]
    fn test_parse_bandwidth_arg_bytes() {
        assert_eq!(parse_bandwidth_arg("512").unwrap(), 512);
        assert_eq!(parse_bandwidth_arg("1000000").unwrap(), 1_000_000);
    }

    #[test]
    fn test_parse_bandwidth_arg_invalid() {
        assert!(parse_bandwidth_arg("abc").is_err());
        assert!(parse_bandwidth_arg("5X").is_err());
        assert!(parse_bandwidth_arg("").is_err());
        assert!(parse_bandwidth_arg("0").is_err());
        assert!(parse_bandwidth_arg("0M").is_err());
    }

    #[test]
    fn test_bandwidth_limiter_creation() {
        let limiter = BandwidthLimiter::new(Some(1_000_000), Some(500_000)).unwrap();
        assert!(limiter.download.is_some());
        assert!(limiter.upload.is_some());

        let limiter = BandwidthLimiter::new(None, None).unwrap();
        assert!(limiter.download.is_none());
        assert!(limiter.upload.is_none());

        let limiter = BandwidthLimiter::new(Some(1_000_000), None).unwrap();
        assert!(limiter.download.is_some());
        assert!(limiter.upload.is_none());
    }

    #[test]
    fn test_bandwidth_limiter_zero_rate() {
        assert!(BandwidthLimiter::new(Some(0), None).is_err());
        assert!(BandwidthLimiter::new(None, Some(0)).is_err());
    }
}
