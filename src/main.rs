
use reqwest::{Client};

mod client;
mod encoding;
mod types;

use client::BitTorrent;
use encoding::Decoder;
use encoding::Encoder;
use log::debug;

#[tokio::main]
async fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let http_client = Client::new();
    let decoder = Decoder {};
    let encoder = Encoder {};
    
    let mut torrent = BitTorrent::new(decoder, encoder, http_client);
    
    torrent.load_file("./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent").unwrap();
    torrent.print_file_metadata().unwrap();
    torrent.download_file().await.unwrap();

    debug!("[main] file downloaded");
}
