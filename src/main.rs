use std::fs::{self, OpenOptions};

use bittorrent_from_scratch::{
    BandwidthStats, PeerConnectionStats,
    bandwidth_limiter::{BandwidthLimiter, parse_bandwidth_arg},
    bittorrent_client::BitTorrent,
    cli::Args,
    encoding::{Decoder, Encoder},
    error::AppError,
    peer_manager::PeerManager,
};
use chrono::Utc;
use clap::Parser;
use env_logger::{Builder, Target};
use log::{LevelFilter, debug, info};
use reqwest::Client;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // CLI Arguments (parse early to get log_dir)
    let args = Args::parse();

    // Determine log directory - use macOS standard location if not specified
    let default_log_dir = if cfg!(target_os = "macos") {
        // Use ~/Library/Logs/bittorrent for macOS
        std::env::var("HOME")
            .map(|home| format!("{}/Library/Logs/bittorrent", home))
            .unwrap_or_else(|_| "./logs".to_string())
    } else {
        "./logs".to_string()
    };

    let log_dir = args.log_dir.as_deref().unwrap_or(&default_log_dir);
    fs::create_dir_all(log_dir).unwrap();
    let filename = format!("{}/app-{}.log", log_dir, Utc::now().format("%Y-%m-%d_%H-%M-%S"));

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(filename)
        .unwrap();

    let log_level = std::env::var("LOG_LEVEL")
        .ok()
        .and_then(|level| match level.to_uppercase().as_str() {
            "ERROR" => Some(LevelFilter::Error),
            "WARN" => Some(LevelFilter::Warn),
            "INFO" => Some(LevelFilter::Info),
            "DEBUG" => Some(LevelFilter::Debug),
            "TRACE" => Some(LevelFilter::Trace),
            _ => None,
        })
        .unwrap_or(LevelFilter::Info);

    Builder::new()
        .target(Target::Pipe(Box::new(file)))
        .filter_level(log_level)
        .init();

    // Parse bandwidth limiters
    let bandwidth_limiter = if args.max_download_rate.is_some() || args.max_upload_rate.is_some() {
        let download_bps = args
            .max_download_rate
            .as_ref()
            .map(|r| parse_bandwidth_arg(r))
            .transpose()
            .expect("Invalid download rate format");

        let upload_bps = args
            .max_upload_rate
            .as_ref()
            .map(|r| parse_bandwidth_arg(r))
            .transpose()
            .expect("Invalid upload rate format");

        if let Some(rate) = &args.max_download_rate {
            info!("Download rate limit: {}", rate);
        }
        if let Some(rate) = &args.max_upload_rate {
            info!("Upload rate limit: {}", rate);
        }

        Some(
            BandwidthLimiter::new(download_bps, upload_bps)
                .expect("Failed to create bandwidth limiter"),
        )
    } else {
        None
    };

    let http_client = Client::new();
    let max_peers = args.max_peers.unwrap_or(20);
    info!("Max concurrent peers: {}", max_peers);
    let peer_manager = PeerManager::new(
        Decoder {},
        http_client,
        bandwidth_limiter,
        Arc::new(BandwidthStats::default()),
        Arc::new(PeerConnectionStats::default()),
        max_peers,
    );

    let mut torrent_client =
        BitTorrent::new(Decoder {}, Encoder {}, peer_manager, args.input_file_path).unwrap();

    torrent_client.print_file_metadata().unwrap();

    match torrent_client
        .download_file(args.output_directory_path)
        .await
    {
        Ok(_) => {
            debug!("[main] File downloaded successfully!");
        }
        Err(e) => match e.downcast_ref::<AppError>() {
            Some(AppError::TrackerRejected(reason)) => {
                eprintln!("\n❌ Tracker Error: {}\n", reason);
                eprintln!("This torrent is not authorized for use with this tracker.");
                eprintln!("Possible reasons:");
                eprintln!("  - The torrent may be private and requires registration");
                eprintln!("  - You may need to use a different tracker");
                eprintln!("  - The torrent file may be invalid or expired");
                std::process::exit(1);
            }
            Some(AppError::NoPeersAvailable) => {
                eprintln!("\n❌ No Peers Found\n");
                eprintln!("The tracker did not return any peers.");
                eprintln!("Possible reasons:");
                eprintln!("  - No one is currently seeding this torrent");
                eprintln!("  - The torrent may be dead or inactive");
                eprintln!("  - Network connectivity issues");
                std::process::exit(1);
            }
            _ => {
                eprintln!("\n❌ Download Failed: {}\n", e);
                std::process::exit(1);
            }
        },
    }
}
