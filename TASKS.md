# BitTorrent Client - Pending Tasks

This file tracks pending tasks and improvements identified in the codebase.

## Bugs

## Features
- [x] Consider the DHT feature to download any kind of file.
  - [ ] Add stats to see if the peer comes from DHT or Tracker.
  - [ ] Support IPV6 DHT Node - Until then will cause this: bittorrent_from_scratch::dht::manager] Failed to bootstrap from [2a02:752:0:18::128]:25401: Invalid argument (os error 22)
- [ ] Add some folder structure consider the DHT to make it clearer.
- [ ] Fix the coverage on the DHT files to be 70% at least.

## Out of Scope
- [ ] Choke uploads if peers are abusing the requests.
- [ ] Support magnet links instead of torrent files.
- [ ] Implement upload task queuing with per-peer rate limits.
- [ ] Implement tit-for-tat unchoke rotation strategy.
- [ ] Implement Cancel message support for aborting in-flight uploads.
- [ ] Implement snubbing detection to deprioritize idle peers.
- [ ] Implement per-peer upload fairness and accounting.
- [ ] Support storing at the disk the current state of a file download.
- [ ] Allow others peers to handshake with me?

## Testability Improvements
- [ ] Add 70% coverage to the File Manager.
