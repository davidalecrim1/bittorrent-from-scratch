# Peer Manager - Download Orchestration and Peer Lifecycle Management

This file documents how the PeerManager is responsible for coordinating BitTorrent piece downloads across multiple peers. It manages the peer connection pool, assigns pieces to optimal peers based on availability, and handles retry logic for failed downloads.

## Architecture

The PeerManager operates with three main responsibilities:

1. **Peer Pool Management**: Maintains connections to up to `max_peers` peers simultaneously, automatically connecting to new peers from the tracker as needed.

2. **Piece Assignment**: Matches pending piece requests to peers that have the piece (based on bitfield) and minimal active downloads, ensuring optimal load distribution.

3. **Download Coordination**: Tracks in-flight pieces, handles completion/failure events, and implements retry logic (up to 3 attempts per piece).

## Message Flow

```text
FileManager                PeerManager              PeerConnection
    |                          |                          |
    |--request_pieces()------->|                          |
    |                          |--assign_piece_to_peer()-->|
    |                          |<--CompletedPiece---------|
    |<--CompletedPiece---------|                          |
    |                          |                          |
    |                          |<--FailedPiece------------|
    |  (re-queue piece)        |                          |
```

## Implementation Details

- **Eager Assignment**: All pieces are requested at once for better parallelism
- **Peer Selection**: Chooses peer with fewest active downloads that has the piece
- **Automatic Reconnection**: Background task maintains peer pool at max_peers capacity
- **Tracker Integration**: Uses `watch_tracker()` to discover new peers periodically

## Key Methods

### `initialize(config: PeerManagerConfig)`
Configures the PeerManager with torrent-specific settings including info hash, client peer ID, file size, number of pieces, and maximum peer connections.

### `start(announce_url, piece_completion_tx, piece_failure_tx)`
Starts the peer management system by spawning two background tasks:
1. **Peer Pool Manager**: Maintains connections to available peers up to max_peers
2. **Piece Assignment Loop**: Continuously assigns pending pieces to available peers

Also integrates with the tracker by calling `watch_tracker()` to periodically fetch new peers.

### `request_pieces(requests: Vec<PieceDownloadRequest>)`
Queues piece download requests. Pieces are added to the pending queue and will be assigned to peers by the piece assignment loop.

### `connect_peer(peer: Peer)` (internal)
Establishes a connection to a peer by:
1. Creating a PeerConnection
2. Performing handshake
3. Retrieving peer ID and bitfield
4. Starting the PeerConnection's read/write tasks
5. Returning a ConnectedPeer struct for tracking

### `assign_piece_to_peer(request: PieceDownloadRequest)` (internal)
Assigns a piece to the optimal peer by:
1. Finding peers that have the piece (based on bitfield)
2. Selecting the peer with the fewest active downloads
3. Sending the download request via the peer's channel
4. Tracking the piece as in-flight

## Data Structures

### `PeerManager`
- `available_peers`: HashMap of peers discovered from tracker
- `connected_peers`: HashMap of active peer connections
- `config`: Configuration with torrent metadata
- `pending_pieces`: Queue of pieces waiting to be assigned
- `in_flight_pieces`: Map of piece index to peer address
- `failed_attempts`: Track retry count per piece

### `ConnectedPeer`
- `peer`: Peer address information
- `peer_id`: Unique peer identifier
- `download_request_tx`: Channel to send piece requests
- `bitfield`: Which pieces the peer has
- `active_downloads`: Set of piece indices currently downloading
