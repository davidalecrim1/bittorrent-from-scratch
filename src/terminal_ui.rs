use std::io::Write;

pub struct ProgressDisplay {
    last_line_count: usize,
    first_print: bool,
}

impl ProgressDisplay {
    pub fn new() -> Self {
        Self {
            last_line_count: 0,
            first_print: true,
        }
    }

    pub fn print(&mut self, stats: &ProgressStats) {
        let mut lines = Vec::new();

        let percentage = if stats.num_pieces > 0 {
            (stats.completed * 100) / stats.num_pieces
        } else {
            0
        };

        lines.push(format!(
            "Peers:     {} connected | {} available ({} tracker, {} DHT) | {} choking",
            stats.total_peers,
            stats.available_peer_count,
            stats.tracker_count,
            stats.dht_count,
            stats.choking_peers
        ));

        lines.push(format!(
            "Pieces:    {}/{} ({}%) | {} pending | {} downloading",
            stats.completed, stats.num_pieces, percentage, stats.pending, stats.in_flight
        ));

        let download_str = format_rate(stats.download_rate);
        let upload_str = format_rate(stats.upload_rate);
        lines.push(format!("Bandwidth: ↓ {} | ↑ {}", download_str, upload_str));

        if let Some(ref dht_stats) = stats.dht_stats {
            lines.push(format!(
                "DHT:       {} nodes | {} buckets ({} full) | {} IPv4, {} IPv6",
                dht_stats.total_nodes,
                dht_stats.buckets_used,
                dht_stats.buckets_full,
                dht_stats.ipv4_nodes,
                dht_stats.ipv6_nodes
            ));
        }

        if self.first_print {
            print!("\x1B[2J\x1B[H");
        } else {
            print!("\x1B[{}A", self.last_line_count);
        }

        for line in &lines {
            println!("\x1B[K{}", line);
        }
        println!();

        let _ = std::io::stdout().flush();

        self.first_print = false;
        self.last_line_count = lines.len() + 1;
    }
}

impl Default for ProgressDisplay {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ProgressStats {
    pub total_peers: usize,
    pub choking_peers: usize,
    pub available_peer_count: usize,
    pub tracker_count: usize,
    pub dht_count: usize,
    pub completed: usize,
    pub num_pieces: usize,
    pub pending: usize,
    pub in_flight: usize,
    pub download_rate: f64,
    pub upload_rate: f64,
    pub dht_stats: Option<DhtStats>,
}

pub struct DhtStats {
    pub total_nodes: usize,
    pub buckets_used: usize,
    pub buckets_full: usize,
    pub ipv4_nodes: usize,
    pub ipv6_nodes: usize,
}

fn format_rate(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_048_576.0 {
        format!("{:.2} MB/s", bytes_per_sec / 1_048_576.0)
    } else if bytes_per_sec >= 1_024.0 {
        format!("{:.1} KB/s", bytes_per_sec / 1_024.0)
    } else {
        format!("{:.0} B/s", bytes_per_sec)
    }
}
