mod application;
mod infrastructure;
mod transport;

#[tokio::main]
pub async fn run() -> anyhow::Result<()> {
    transport::serve(
        tokio::io::BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}
