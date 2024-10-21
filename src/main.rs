use std::{env, path::PathBuf};

use config::Config;
use data::Data;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod config;
mod data;
mod github;

fn get_tokens_from_env() -> Vec<String> {
    let env_token = env::var("GITHUB_TOKEN").ok();
    let env_tokens = env::var("GITHUB_TOKENS").ok();

    let mut tokens = vec![];

    if let Some(t) = env_token {
        tokens.push(t)
    }

    if let Some(ts) = env_tokens {
        let ts = ts.split(',');
        for t in ts {
            tokens.push(t.to_string());
        }
    }

    tokens
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    // console_subscriber::init();

    let github_token = get_tokens_from_env();

    let timeout = env::var("RUST_REPOS_TIMEOUT")
        .ok()
        .and_then(|el| el.parse::<u64>().ok());

    let data_dir = PathBuf::from("./data_new");

    let config = Config {
        github_token,
        data_dir,
        timeout,
    };

    let data = Data::new(&config)?;

    let scraper = github::Scraper::new(config, data);

    scraper.scrape().await
}
