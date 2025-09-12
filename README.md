# BitTorrent in Rust

This is a project to explore the low level of peer to peer applications using Rust.

## Documentation

### Bencode
This the protocol used over TCP for BitTorrrent to communicate. I could use serde implemention of it in Rust, but what's the fun in that? So I started to implement my own version in this app.


## API Design for the Entrypoint of the Program (TODO)
- Load a File
- Get Peers
- Download the File from Active Peers
- Print file Metadata
- Track the file download in the Terminal

Maybe this will be a CLI. I haven't decided.