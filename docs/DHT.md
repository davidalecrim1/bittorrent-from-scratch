# DHT (Distributed Hash Table) in BitTorrent

## Overview

The Distributed Hash Table (DHT) is a decentralized peer discovery mechanism for BitTorrent, enabling **trackerless** torrents. Instead of relying on centralized tracker servers, DHT distributes peer information across the network itself, making the system more resilient and eliminating single points of failure.

## Why DHT Matters

### Traditional Tracker Limitations
- **Single point of failure**: If tracker goes down, no peer discovery
- **Centralization**: Tracker operator controls access
- **Privacy concerns**: Tracker knows who downloads what
- **Censorship risk**: Trackers can be blocked or shut down

### DHT Advantages
- **Decentralized**: No single server controls peer discovery
- **Self-healing**: Network reorganizes automatically when nodes leave
- **Resilient**: Works even when all trackers are offline
- **Privacy**: No central authority tracking downloads
- **Scalability**: Distributes load across all participating nodes

## Core Concepts

### Kademlia DHT

BitTorrent uses the **Kademlia** DHT algorithm (BEP 5), which organizes nodes in a binary tree structure based on XOR distance metric.

**Node ID**: Every DHT node has a unique 160-bit identifier (same size as SHA1 hash)

**Distance Metric**: XOR distance between two IDs determines "closeness"
```
distance(A, B) = A XOR B
```
- Smaller XOR result = closer nodes
- Used to route queries toward target efficiently

**K-Buckets**: Each node maintains a routing table with 160 k-buckets
- One bucket per bit position in the 160-bit ID space
- Each bucket stores up to K (8) closest nodes for that distance range
- Organizes the network into a structured overlay

### Info Hash as Target

When searching for peers:
1. Calculate SHA1 hash of torrent's "info" dictionary → **info_hash**
2. Use info_hash as DHT lookup target
3. Find DHT nodes closest to this target
4. Query those nodes for peers sharing that torrent

This creates a natural mapping: peers sharing the same torrent cluster around the same DHT location.

## DHT Protocol (KRPC)

### Message Format

DHT uses **KRPC** (Kademlia RPC) protocol over UDP with bencode encoding.

**Message Types**:
- **Query**: Request from one node to another
- **Response**: Reply to a query
- **Error**: Error condition

**Transaction ID**: Unique identifier to match responses to queries (prevents response mixing)

### Query Types

**ping**: Verify a node is alive
```
Query:  { id: <sender_node_id> }
Response: { id: <responder_node_id> }
```

**find_node**: Find K closest nodes to target
```
Query:  { id: <sender_id>, target: <target_node_id> }
Response: { id: <responder_id>, nodes: <compact_node_list> }
```

**get_peers**: Find peers for an info_hash
```
Query:  { id: <sender_id>, info_hash: <20_bytes> }
Response: {
  id: <responder_id>,
  token: <opaque_token>,
  values: <peer_list>  // OR
  nodes: <closer_nodes>  // if no peers known
}
```

**announce_peer**: Announce that you have the torrent
```
Query:  { id: <sender_id>, info_hash: <20_bytes>, port: <port>, token: <token> }
Response: { id: <responder_id> }
```

## Iterative Lookup Algorithm

DHT uses **iterative** (not recursive) lookups for efficiency and resilience.

### Phases

**1. Bootstrap**
- Query hardcoded bootstrap nodes (e.g., router.bittorrent.com)
- Receive initial set of DHT nodes
- Populate routing table

**2. Iterative Search**
- Start with K closest nodes from routing table
- Query ALPHA (3) nodes in parallel
- Process responses:
  - **Peers found**: Add to result set, continue searching
  - **Nodes returned**: Add to candidate list, sort by distance
- Select next ALPHA closest unqueried nodes
- Repeat until:
  - MAX_ITERATIONS reached (8 rounds)
  - No closer nodes found (convergence)
  - No unqueried candidates remain

**3. Result Accumulation**
- Collect all peers from responses
- Continue through full iteration (don't exit early)
- Return accumulated peer list

### Key Parameters

```rust
K = 8            // Bucket size (max nodes per bucket)
ALPHA = 3        // Parallelism (concurrent queries per iteration)
MAX_ITERATIONS = 8  // Safety limit on lookup rounds
```

**Network Cost**: Up to 24 queries (8 iterations × 3 parallel queries)
**Typical Duration**: 2-4 seconds for complete lookup

## Architecture Components

### DhtManager
Central orchestrator managing DHT operations.

**Responsibilities**:
- Bootstrap DHT with initial nodes
- Perform iterative peer lookups
- Handle incoming queries
- Maintain routing table

**Background Tasks**:
- **Message handler**: Process incoming UDP messages
- **Bootstrap refresh**: Re-bootstrap every 15 minutes to keep routing table fresh
- **Query cleanup**: Remove expired queries (5-second timeout)

### Routing Table
Stores known DHT nodes organized in k-buckets.

**Operations**:
- `insert(node)`: Add node to appropriate bucket
- `find_closest(target, k)`: Get K nodes closest to target by XOR distance
- Eviction policy: Remove oldest nodes when bucket full

**Properties**:
- 160 buckets (one per bit position)
- Each bucket holds max 8 nodes
- Ordered by last-seen time
- Self-node never added

### Message I/O
Handles KRPC message serialization over UDP.

**Encoding**: Bencode format
- Text fields → UTF-8 strings
- Binary fields (node IDs, hashes, tokens) → Raw bytes
- Auto-detects binary vs text during decoding

**Protocol Details**:
- UDP port 6881 (standard)
- Max message size: 1472 bytes
- Length-prefixed bencode: `N:[bytes]`

### Query Manager
Tracks in-flight queries and matches responses.

**Responsibilities**:
- Generate unique transaction IDs
- Register outgoing queries with oneshot channels
- Route incoming responses to correct channel
- Cleanup expired queries (5-second timeout)

**Pattern**: Each query gets a oneshot channel; response awakens waiting task.

## Response Handling

### GetPeers Response Types

**Case 1: Node has peers (values field)**
```rust
Response::GetPeers {
  values: Some([peer1, peer2, ...]),
  nodes: Some([node1, node2, ...]),  // May also include closer nodes
  token: <token>
}
```
→ Collect peers AND add nodes to candidate list

**Case 2: Node doesn't have peers (nodes field only)**
```rust
Response::GetPeers {
  values: None,
  nodes: Some([node1, node2, ...]),  // Closer nodes
  token: <token>
}
```
→ Add nodes to candidate list, continue search

**BEP 5 Extension**: Modern implementations return BOTH values and nodes fields. Always check both.

### Convergence Detection

Stop iterating when search stops improving:
```rust
if current_closest_distance >= last_closest_distance {
    break;  // No progress, search converged
}
```

This allows early exit when the algorithm has found the optimal nodes, even before MAX_ITERATIONS.

## Integration with Peer Manager

### Parallel Discovery
Both tracker and DHT run simultaneously:
- **Tracker**: Queries every 30 minutes (if available)
- **DHT**: Initial query with retries, then every 2 minutes

### Retry Strategy for Trackerless Torrents

When DHT finds 0 peers initially:
1. **Retry 1**: Wait 30 seconds, try again
2. **Retry 2**: Wait 30 seconds, try again
3. **Retry 3**: Wait 30 seconds, try again (final immediate retry)
4. **Regular polling**: Continue checking every 2 minutes

**Timeline**: 4 attempts in 90 seconds, then every 2 minutes indefinitely

**Rationale**: DHT is eventually consistent. Nodes may not have info_hash initially, but as more peers join and announce, information propagates through the network.

### Peer Pool Management

All discovered peers (tracker + DHT) go into shared `available_peers` pool:
- PeerManager connector task attempts connections every 10 seconds
- Up to 20 concurrent peer connections (configurable)
- Failed connections permanently removed (no per-peer retry)

## Network Behavior

### Bootstrap Process

**Initial State**: Empty routing table
**Bootstrap Flow**:
1. Resolve bootstrap hostnames to IPs (IPv4 only)
2. Send `find_node` queries with self_id as target
3. Receive 3-16 nodes per response
4. Add nodes to routing table
5. Routing table now seeded for peer lookups

**Bootstrap Nodes** (hardcoded):
- dht.libtorrent.org:25401
- router.bittorrent.com:6881
- router.utorrent.com:6881
- dht.transmissionbt.com:6881

### Routing Table Growth

As peer lookups proceed:
- Every response adds nodes to routing table
- Routing table organizes nodes by XOR distance
- Future lookups start with better candidates
- Network knowledge improves over time

### Periodic Maintenance

**Bootstrap refresh** (every 15 minutes):
- Re-query bootstrap nodes
- Refresh routing table
- Discover new nodes
- Remove stale nodes

**Query cleanup** (every 1 second):
- Remove queries older than 5 seconds
- Free associated resources
- Prevent memory leaks

## Performance Characteristics

### Lookup Performance
- **Best case**: 3 queries (peers found in first ALPHA nodes)
- **Typical case**: 6-12 queries over 2-3 iterations
- **Worst case**: 24 queries (8 iterations × 3 parallel)
- **Duration**: 2-4 seconds for complete lookup

### Peer Discovery Rates
- **With early exit (old)**: 1-8 peers from first responding node
- **Full iteration (new)**: 10-50+ peers from multiple nodes
- **Trackerless torrents**: May take 1-3 retries to find initial peers

### Network Overhead
- **UDP messages**: Small (< 1KB typically)
- **Bandwidth**: Minimal (few KB/s during active lookup)
- **CPU**: Low (bencode encode/decode, XOR calculations)
- **Memory**: Routing table ~10KB (160 buckets × 8 nodes × ~80 bytes/node)

## Comparison: Tracker vs DHT

| Aspect | Tracker | DHT |
|--------|---------|-----|
| **Architecture** | Centralized | Distributed |
| **Reliability** | Single point of failure | Self-healing network |
| **Latency** | Fast (1 HTTP request) | Slower (iterative lookup) |
| **Peer count** | All known peers | Sample of network |
| **Privacy** | Tracker logs downloads | No central logging |
| **Censorship** | Easy to block/shut down | Resistant to censorship |
| **Setup** | Requires tracker URL | Works without configuration |

## Best Practices

### When to Use DHT
- **Trackerless torrents**: DHT is the only option
- **Backup discovery**: Use alongside tracker for resilience
- **Censorship resistance**: When trackers blocked or unavailable
- **Privacy**: When avoiding centralized tracking

### When to Use Tracker
- **Speed**: Faster initial peer discovery
- **Completeness**: Gets full peer list from tracker
- **Reliability**: More predictable behavior
- **Private torrents**: Tracker controls access

### Recommended Strategy
**Use both simultaneously**: Most clients enable both tracker and DHT by default, maximizing peer discovery and resilience.

## References

- **BEP 5**: DHT Protocol specification
- **Kademlia Paper**: Original distributed hash table algorithm
- **BEP 5 Extension**: Modern DHT enhancements (both values and nodes fields)
