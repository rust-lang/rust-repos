// Copyright (c) 2018 Pietro Albini <pietro@pietroalbini.org>
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of
// this software and associated documentation files (the "Software"), to deal in
// the Software without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
// of the Software, and to permit persons to whom the Software is furnished to do
// so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

extern crate crossbeam_utils;
extern crate csv;
extern crate ctrlc;
extern crate env_logger;
#[macro_use]
extern crate failure;
#[macro_use]
extern crate log;
extern crate reqwest;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;

mod config;
mod data;
mod github;
mod prelude;
mod utils;

use config::Config;
use prelude::*;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

fn app() -> Fallible<()> {
    // Get the GitHub token from the environment
    let github_token =
        std::env::var("GITHUB_TOKEN").context("failed to get the GitHub API token")?;

    let timeout = if let Ok(var) = std::env::var("RUST_REPOS_TIMEOUT") {
        Some(
            var.parse::<u64>()
                .context("failed to parse RUST_REPOS_TIMEOUT")?,
        )
    } else {
        None
    };

    // Parse CLI arguments
    let args = std::env::args().skip(1).collect::<Vec<String>>();
    if args.is_empty() {
        bail!("missing argument: <data_dir>");
    } else if args.len() > 1 {
        bail!("too many arguments");
    }

    // Ensure the data directory exists
    let data_dir = PathBuf::from(&args[0]);
    if !data_dir.is_dir() {
        debug!(
            "created missing data directory: {}",
            data_dir.to_string_lossy()
        );
        std::fs::create_dir_all(&data_dir)?;
    }

    let config = Config {
        github_token,
        data_dir,
        timeout,
    };

    let data = data::Data::new(&config);

    let should_stop = Arc::new(AtomicBool::new(false));
    let stop = should_stop.clone();
    ctrlc::set_handler(move || {
        info!("received Ctrl+C, terminating...");
        stop.store(true, Ordering::SeqCst);
    })?;

    github::scrape(&data, &config, &should_stop)?;

    Ok(())
}

fn main() {
    // Initialize logging
    // This doesn't use from_default_env() because it doesn't allow to override filter_module()
    // with the RUST_LOG environment variable
    let mut logger = env_logger::Builder::new();
    logger.filter_module("rust_repos", log::LevelFilter::Info);
    if let Ok(content) = std::env::var("RUST_LOG") {
        logger.parse_filters(&content);
    }
    logger.init();

    let start = Instant::now();

    let result = app();
    if let Err(ref err) = &result {
        utils::log_error(err);
    }

    info!(
        "execution completed in {} seconds",
        start.elapsed().as_secs()
    );

    if result.is_err() {
        std::process::exit(1);
    }
}
