use bittorrent_from_scratch::{
    cli::Args,
    encoding::{Decoder, Encoder},
    error::AppError,
    file_manager::BitTorrent,
    peer_manager::PeerManager,
};
use clap::Parser;
use log::debug;
use reqwest::Client;

#[tokio::main]
async fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .target(env_logger::Target::Stdout)
        .init();

    let http_client = Client::new();
    let peer_manager = PeerManager::new(Decoder {}, http_client);

    let args = Args::parse();
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
