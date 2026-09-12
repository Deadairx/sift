use clap::Parser;

#[derive(Debug, Parser)]
#[command(version, about = "Submit long-form content to Sift")]
struct Cli {
    /// URL to submit for processing.
    url: Option<String>,
}

fn main() {
    sift::init_tracing();

    let cli = Cli::parse();

    if let Some(url) = cli.url {
        tracing::info!(%url, "job submission is not implemented yet");
        println!("job submission is not implemented yet: {url}");
    }
}
