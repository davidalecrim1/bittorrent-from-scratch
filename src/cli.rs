use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, allow_external_subcommands = true)]
pub struct Args {
    /// Input: either a .torrent file path OR a magnet link
    #[arg(short = 'i', long = "input")]
    pub input: String,

    /// Output directory to save the downloaded file
    #[arg(short = 'o', long = "output", default_value = "./downloaded")]
    pub output_directory_path: String,

    /// Optional: Override output filename (useful for magnet links)
    #[arg(short = 'n', long = "name")]
    pub output_name: Option<String>,

    /// Maximum download rate (e.g., "5M", "100K", "1G"). No limit if omitted.
    #[arg(long = "max-download-rate")]
    pub max_download_rate: Option<String>,

    /// Maximum upload rate (e.g., "1M", "500K"). No limit if omitted.
    #[arg(long = "max-upload-rate")]
    pub max_upload_rate: Option<String>,

    /// Maximum number of concurrent peer connections (default: 20)
    #[arg(long = "max-peers")]
    pub max_peers: Option<usize>,

    /// Directory to store log files (default: ~/Library/Logs/bittorrent on macOS, ./logs elsewhere)
    #[arg(long = "log-dir")]
    pub log_dir: Option<String>,

    /// Disable HTTP tracker for peer discovery (DHT only)
    #[arg(long = "no-tracker")]
    pub no_tracker: bool,

    /// Disable DHT for peer discovery (tracker only)
    #[arg(long = "no-dht")]
    pub no_dht: bool,
}
