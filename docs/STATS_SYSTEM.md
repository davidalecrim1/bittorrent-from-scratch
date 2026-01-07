# Concurrency-Safe Statistics in Rust

**(For event-driven, high-concurrency systems like BitTorrent clients)**

## Core Principle

**Single writer, many readers.**
All mutable statistics are owned by **one task**. Everyone else sends **events**, never mutations.

This aligns with Rust’s strengths: ownership, message passing, and explicit synchronization.

---

## Architecture

### 1. Stats Actor (Single Owner)

Create one async task (or thread) that:

* Owns all mutable stats (`HashMap`, counters, timers)
* Processes events sequentially
* Periodically publishes read-only snapshots

No `Arc<Mutex<...>>` shared across the system.

---

### 2. Event-Driven Updates

Producers (peer handlers, piece downloaders) emit **events**:

```rust
enum StatsEvent {
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    PeerChoked(PeerId),
    PeerUnchoked(PeerId),
    PieceRequested,
    PieceReceived { bytes: usize },
}
```

They send events through an `mpsc` channel.

**Rule:** producers never read or mutate stats.

---

### 3. Snapshot-Based Reads

The stats actor periodically builds an **immutable snapshot**:

```rust
#[derive(Clone)]
struct StatsSnapshot {
    connected_peers: usize,
    choked_peers: usize,
    download_rate_bps: u64,
}
```

Publish via:

* `Arc<StatsSnapshot>` + `atomic::AtomicPtr`, or
* `ArcSwap`, or
* `tokio::sync::watch`

Readers only access snapshots — never live state.

---

### 4. Read Path Is Lock-Free

Consumers (UI, logger, metrics exporter):

* Load the latest snapshot atomically
* Never block writers
* Never allocate or compute

This keeps hot paths predictable.

---

## Why This Works Well in Rust

* **Ownership clarity**: one task owns mutation
* **No lock contention**: avoids `Mutex` hell
* **Composable**: add new stats by adding events
* **Testable**: feed events, assert snapshots

Rust’s type system enforces the discipline instead of relying on convention.

---

## Backpressure Strategy

Stats are **observational**, not critical.

If the channel is full:

* Drop events, or
* Coalesce (e.g., batch counters)

Never block peer IO on metrics.

---

## Anti-Patterns (Avoid)

* `Arc<Mutex<HashMap<PeerId, PeerStats>>>`
* Atomics everywhere “for performance”
* Reading partial state from multiple locks
* Computing rates in peer handlers

These break scalability and mental simplicity.

---

## Mental Model

> *Peers produce facts.
> One task turns facts into truth.
> Everyone else observes snapshots.*
