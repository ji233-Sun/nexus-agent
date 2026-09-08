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
pub use infrastructure::process_tree::{
    configure as configure_child_process, terminate as terminate_child_process,
};
