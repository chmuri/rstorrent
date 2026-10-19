#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    rstorrent::run_app().await
}
