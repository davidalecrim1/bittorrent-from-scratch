use std::sync::Arc;
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

    let pieces_count = torrent.get_num_pieces().unwrap();

    for peer in res {
        let peer_conn_res = torrent.create_peer_connection(*b"postman-000000000001", &peer).await;
        if peer_conn_res.is_err() {
            dbg!("[main] failed to create peer connection: {:?}", peer_conn_res.err().unwrap());
            continue;
        }

        let peer_conn = Arc::new(peer_conn_res.unwrap());
        let peer_id: [u8; 20] = peer_conn.get_peer_id().unwrap();

        dbg!("[main] peer connection success: {:?}", &peer_id);
        dbg!("[main] peer connection read messages");
        
        tokio::task::spawn(async move {
            let peer_conn = peer_conn.clone();
            peer_conn.read_messages(pieces_count).await;
        });
        
        peer_conn.send_interested().await;
        break;
    }
    // for debugging
    //fs::write("./output.txt", format!("{:?}", torrent)).unwrap();
}
