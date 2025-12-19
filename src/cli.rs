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
}
