# BitTorrent in Rust

This is a project to explore the low level of peer to peer applications using Rust. The goal is to build a BitTorrent client that on a giving torrent file, parse it given the Bencode format, connect to the Tracker server, get all available peers with that download the file based on the available peers.

## Documentation

### Bencode
This the protocol used over TCP for BitTorrrent to communicate. I could use serde implemention of it in Rust, but what's the fun in that? So I started to implement my own version in this app.

### BitTorrent Client
Each file that is shared in a torrent it's transformed into a Bencode representation that is the .torrent file. Beyond some metadata, it says how many pieces the file has, with it's hash to validate that it was properly transfered between peers.
The client can get a list of peers from a Tracker server. It tells the IP/Port of the peers. Then it can open TCP connection using a protocol called Peer Messages exchanged in a full duplex way in that connection.
This is where the magic happens. Using the Peer Messages in raw TCP to get the pieces (splited in chunks over the network) to join together in the final file in a distri

### Torrent File
A .torrent file contains a few key fields:
- piece length → size of each piece in bytes (e.g., 256 KiB)
- pieces → a concatenated string of SHA1 hashes for each piece
- length → total size of the file (or files for multi-file torrents)

## API Design for the Entrypoint of the Program (TODO)
- Load a File
- Get Peers
- Download the File from Active Peers
- Print file Metadata
- Track the file download in the Terminal

Maybe this will be a CLI. I haven't decided.

## Current Features
- [x] Parse the Torrent File;
- [x] Connect with a peer;
- [x] Exchange messages between the client and the peer;