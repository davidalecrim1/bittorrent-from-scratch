use std::fs::{self, OpenOptions};

use bittorrent_from_scratch::{
    BandwidthStats, PeerConnectionStats,
    bandwidth_limiter::{BandwidthLimiter, parse_bandwidth_arg},
    bittorrent_client::BitTorrent,
    cli::Args,
    dht::{DefaultUdpSocketFactory, DhtClient, DhtManager, UdpMessageIO, UdpSocketFactory},
    encoding::{Decoder, Encoder},
    error::AppError,
    peer_manager::{DefaultPeerConnectionFactory, PeerManager},
    tracker_client::{HttpTrackerClient, TrackerClient},
};
use chrono::Utc;
use clap::Parser;
use env_logger::{Builder, Target};
use log::{LevelFilter, debug, info, warn};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::broadcast;

const DHT_PORT: u16 = 6881;

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let default_log_dir = if cfg!(target_os = "macos") {
        std::env::var("HOME")
            .map(|home| format!("{}/Library/Logs/bittorrent", home))
            .unwrap_or_else(|_| "./logs".to_string())
    } else {
        "./logs".to_string()
    };

    let log_dir = args.log_dir.as_deref().unwrap_or(&default_log_dir);
    fs::create_dir_all(log_dir).unwrap();

    let filename = format!(
        "{}/app-{}.log",
        log_dir,
        Utc::now().format("%Y-%m-%d_%H-%M-%S")
    );

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

    if args.no_tracker && args.no_dht {
        eprintln!("Error: Cannot disable both tracker and DHT. At least one must be enabled.");
        std::process::exit(1);
    }

    let (_dht_shutdown_tx, dht_client): (
        Option<broadcast::Sender<()>>,
        Option<Arc<dyn DhtClient>>,
    ) = if args.no_dht {
        info!("DHT disabled via --no-dht flag");
        (None, None)
    } else {
        let socket_factory = DefaultUdpSocketFactory;
        let dht_bind_addr = format!("0.0.0.0:{}", DHT_PORT);
        let udp_socket = match socket_factory.bind(&dht_bind_addr).await {
            Ok(socket) => {
                if let Ok(local_addr) = socket.local_addr() {
                    info!("DHT listening on UDP {}", local_addr);
                } else {
                    info!("DHT listening on UDP port {}", DHT_PORT);
                }
                socket
            }
            Err(e) => {
                warn!(
                    "Failed to bind UDP port {}: {}, trying ephemeral port",
                    DHT_PORT, e
                );
                socket_factory
                    .bind("0.0.0.0:0")
                    .await
                    .expect("Failed to bind UDP socket for DHT on ephemeral port")
            }
        };

        let message_io = Arc::new(UdpMessageIO::new(udp_socket, Encoder {}, Decoder {}));
        let dht_manager = Arc::new(DhtManager::new(message_io));

        let (shutdown_tx, _shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

        dht_manager
            .clone()
            .spawn_message_handler(shutdown_tx.subscribe(), Some(ready_tx));
        dht_manager
            .clone()
            .spawn_bootstrap_refresh(shutdown_tx.subscribe());
        dht_manager
            .clone()
            .spawn_query_cleanup(shutdown_tx.subscribe());

        ready_rx.await.expect("Message handler failed to start");
        info!("DHT message handler ready, starting bootstrap");

        if let Err(e) = dht_manager.bootstrap().await {
            log::warn!("DHT bootstrap failed: {}", e);
        } else {
            info!("DHT bootstrapped successfully");
        }

        (Some(shutdown_tx), Some(dht_manager as Arc<dyn DhtClient>))
    };

    let tracker_client = if args.no_tracker {
        info!("HTTP tracker disabled via --no-tracker flag");
        None
    } else {
        Some(Arc::new(HttpTrackerClient::new(http_client, Decoder {})) as Arc<dyn TrackerClient>)
    };

    let peer_manager = PeerManager::new_with_dht(
        tracker_client,
        dht_client,
        Arc::new(DefaultPeerConnectionFactory),
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
