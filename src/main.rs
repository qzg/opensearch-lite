use mainstack_search::{server, Config};

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
            eprintln!("mainstack-search config validation failed: {error}");
            std::process::exit(1);
        }
        eprintln!("mainstack-search config validation passed");
        return;
    }

    if let Err(error) = server::run(config).await {
        eprintln!("mainstack-search failed: {error}");
        std::process::exit(1);
    }
}
