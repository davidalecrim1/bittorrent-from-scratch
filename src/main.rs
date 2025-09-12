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
        let mut peer_conn = match torrent.create_peer_connection(*b"postman-000000000001", &peer).await {
            Ok(conn) => conn,
            Err(e) => {
                dbg!("[main] failed to create peer connection: {:?}", e);
                continue;
            }
        };

        let peer_id: [u8; 20] = peer_conn.get_peer_id().unwrap();
        dbg!("[main] peer connection success: {:?}", &peer_id);

        let (command_sender, mut message_receiver) = match peer_conn.start_exchanging_messages(pieces_count).await {
            Ok(channels) => channels,
            Err(e) => {
                dbg!("[main] failed to start message exchange: {:?}", e);
                continue;
            }
        };

        tokio::task::spawn(async move {
            while let Some(message) = message_receiver.recv().await {
                match message {
                    types::PeerMessage::Unchoke(_) => {
                        dbg!("[main] received unchoke message - peer is ready to send data");
                    }
                    types::PeerMessage::Bitfield(bitfield) => {
                        dbg!("[main] received bitfield message - peer has pieces info");
                    }
                    types::PeerMessage::Interested(_) => {
                        dbg!("[main] received interested message from peer");
                    }
                }
            }
            dbg!("[main] message handling loop finished");
        });

        let interested_msg = types::PeerMessage::Interested(types::InterestedMessage {});
        dbg!("[main] sending interested message to peer");
        if let Err(e) = command_sender.send(interested_msg).await {
            dbg!("[main] failed to send interested message: {:?}", e);
            continue;
        }

        tokio::time::sleep(Duration::from_secs(10)).await; // just a hack to hang the main thread
        break;
    }
    // for debugging
    //fs::write("./output.txt", format!("{:?}", torrent)).unwrap();
}
