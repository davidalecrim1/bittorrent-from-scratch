use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, allow_external_subcommands = true)]
pub struct Args {
    /// Input torrent file to process
    #[arg(short = 'i', long = "input")]
    pub input_file_path: String,

    /// Output directory to save the downloaded file
    #[arg(short = 'o', long = "output", default_value = "./downloaded")]
    pub output_directory_path: String,

    /// Maximum download rate (e.g., "5M", "100K", "1G"). No limit if omitted.
    #[arg(long = "max-download-rate")]
    pub max_download_rate: Option<String>,

    /// Maximum upload rate (e.g., "1M", "500K"). No limit if omitted.
    #[arg(long = "max-upload-rate")]
    pub max_upload_rate: Option<String>,
}
