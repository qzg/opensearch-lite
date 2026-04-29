use opensearch_lite::{server, Config};

#[tokio::main]
async fn main() {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    if let Err(error) = server::run(config).await {
        eprintln!("opensearch-lite failed: {error}");
        std::process::exit(1);
    }
}
