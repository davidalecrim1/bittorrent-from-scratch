# BitTorrent Client - Pending Tasks

This file tracks pending tasks and improvements identified in the codebase.

## Features
- [ ] Improve the folder directory here to get rid of the types file.
- [ ] Break the file types into the existing files and remove it.

## Out of Scope
- [ ] Choke uploads if peers are abusing the requests.
- [ ] Implement upload task queuing with per-peer rate limits.
  - Maybe a semaphore just like the for downloads.
- [ ] Implement tit-for-tat unchoke rotation strategy.
- [ ] Implement Cancel message support for aborting in-flight uploads.
- [ ] Implement snubbing detection to deprioritize idle peers.
- [ ] Implement per-peer upload fairness and accounting.
- [ ] Support storing at the disk the current state of a file download.
- [ ] Allow others peers to handshake with me?
  - Does this make sense?
  - How are they going to find me? Am I added to a tracker or DHT?

## Maybe
- [ ] Add 70% coverage to the File Manager.
- [ ] Fix the coverage on the DHT files to be 70% at least.
