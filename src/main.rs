use std::time::Duration;

use reqwest::{Client};

mod client;
mod encoding;
mod types;

use client::BitTorrent;
use encoding::Decoder;
use encoding::Encoder;


#[tokio::main]
async fn main() {
    log::set_max_level(log::LevelFilter::Debug);

    let http_client = Client::new();
    let decoder = Decoder {};
    let encoder = Encoder {};
    
    let mut torrent = BitTorrent::new(decoder, encoder, http_client);
    torrent.load_file("./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent").unwrap();
    
    dbg!("[main] torrent info hash as hex: {:?}", &torrent.get_info_hash_as_hex().unwrap());
    dbg!("[main] torrent file size: {:?}", &torrent.get_file_size().unwrap());

    let res = torrent.get_peers().await.unwrap();
    dbg!("[main] torrent peers: {:?}", &res);

    let mut peer_conn = torrent.create_peer_connection(*b"postman-000000000001", &res[1]).await.unwrap();
    dbg!("[main] peer connection: {:?}", &peer_conn.get_peer_id().unwrap());

    dbg!("[main] read messages");
    peer_conn.read_messages(torrent.get_num_pieces().unwrap()).await;
    
    // for debugging
    //fs::write("./output.txt", format!("{:?}", torrent)).unwrap();
}
