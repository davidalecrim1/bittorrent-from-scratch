use clap::Parser;
use reqwest::Client;

mod cli;
mod encoding;
mod file_manager;
mod peer_manager;
mod types;

use cli::Args;
use encoding::Decoder;
use encoding::Encoder;
use file_manager::BitTorrent;
use log::debug;
use peer_manager::PeerManager;

#[tokio::main]
async fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let http_client = Client::new();
    let peer_manager = PeerManager::new(Decoder {}, Encoder {}, http_client);

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
        Err(e) => {
            let error_msg = e.to_string();
            if error_msg.contains("Tracker rejected request") {
                eprintln!("\n❌ Tracker Error: {}\n", error_msg);
                eprintln!("This torrent is not authorized for use with this tracker.");
                eprintln!("Possible reasons:");
                eprintln!("  - The torrent may be private and requires registration");
                eprintln!("  - You may need to use a different tracker");
                eprintln!("  - The torrent file may be invalid or expired");
                std::process::exit(1);
            } else if error_msg.contains("No peers available") {
                eprintln!("\n❌ No Peers Found: {}\n", error_msg);
                eprintln!("The tracker did not return any peers.");
                eprintln!("Possible reasons:");
                eprintln!("  - No one is currently seeding this torrent");
                eprintln!("  - The torrent may be dead or inactive");
                eprintln!("  - Network connectivity issues");
                std::process::exit(1);
            } else {
                eprintln!("\n❌ Download Failed: {}\n", error_msg);
                std::process::exit(1);
            }
        }
    }
}
