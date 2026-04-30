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

    if config.security.validate_only {
        if let Err(error) = server::validate_config(&config) {
            eprintln!("opensearch-lite config validation failed: {error}");
            std::process::exit(1);
        }
        eprintln!("opensearch-lite config validation passed");
        return;
    }

    if let Err(error) = server::run(config).await {
        eprintln!("opensearch-lite failed: {error}");
        std::process::exit(1);
    }
}
