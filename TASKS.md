# BitTorrent Client - Pending Tasks

:This file tracks pending tasks and improvements identified in the codebase.

## Features
- [ ] Consider the DHT feature to download any kind of file.

## Out of Scope
- [ ] Choke uploads if peers are abusing the requests.
- [ ] Support magnet links instead of torrent files.
- [ ] Support DHT (Distributed Hash Table) instead of only the tracker server.
- [ ] Implement upload task queuing with per-peer rate limits.
- [ ] Implement tit-for-tat unchoke rotation strategy.
- [ ] Implement Cancel message support for aborting in-flight uploads.
- [ ] Implement snubbing detection to deprioritize idle peers.
- [ ] Implement per-peer upload fairness and accounting.
- [ ] Clean up in-flight upload tasks when peers disconnect.
- [ ] Handle NotInterested by stopping further uploads to that peer.

## Testability Improvements
- [ ] Add 70% coverage to the File Manager.
