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

mod api;

use config::Config;
use crossbeam_utils::thread::scope;
use data::{Data, Repo};
use github::api::GitHubApi;
use prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use utils::wrap_thread;

static WANTED_LANG: &'static str = "Rust";

fn load_thread(api: &GitHubApi, data: &Data, to_load: Vec<String>) -> Fallible<()> {
    debug!(
        "collected {} non-fork repositories, loading them",
        to_load.len()
    );

    let mut graph_repos = api.load_repositories(&to_load)?;
    for repo in graph_repos.drain(..) {
        if let Some(repo) = repo {
            let mut found = false;
            for lang in repo.languages.nodes.iter().filter_map(Option::as_ref) {
                if lang.name == WANTED_LANG {
                    found = true;
                    break;
                }
            }

            if found {
                let has_cargo_toml = api.file_exists(&repo, "Cargo.toml")?;
                let has_cargo_lock = api.file_exists(&repo, "Cargo.lock")?;

                data.store_repo(
                    "github",
                    Repo {
                        id: repo.id,
                        name: repo.name_with_owner.clone(),
                        has_cargo_toml,
                        has_cargo_lock,
                    },
                )?;

                info!(
                    "found {}: Cargo.toml = {:?}, Cargo.lock = {:?}",
                    repo.name_with_owner, has_cargo_toml, has_cargo_lock,
                );
            }
        }
    }

    // Applease Clippy
    ::std::mem::drop(to_load);

    Ok(())
}

pub fn scrape(data: &Data, config: &Config, should_stop: &AtomicBool) -> Fallible<()> {
    info!("started scraping for GitHub repositories");

    let gh = api::GitHubApi::new(config);
    let mut to_load = Vec::with_capacity(100);

    let result = scope(|scope| {
        let mut last_id = data.get_last_id("github")?.unwrap_or(0);

        loop {
            // Wait 2 minutes if GitHub is slowing us down
            if gh.should_slow_down() {
                warn!("slowing down the scraping (2 minutes pause)");
                ::std::thread::sleep(Duration::from_secs(120));
            }

            let start = Instant::now();

            debug!("scraping 100 repositories from the REST API");

            // Load all the non-fork repositories in the to_load vector
            let mut repos = gh.scrape_repositories(last_id)?;
            let finished = repos.len() < 100 || should_stop.load(Ordering::SeqCst);
            for repo in repos.drain(..) {
                if let Some(repo) = repo {
                    last_id = repo.id;
                    if repo.fork {
                        continue;
                    }

                    to_load.push(repo.node_id);

                    if to_load.len() == 100 {
                        let to_load_now = to_load.clone();
                        scope.spawn(|_| wrap_thread(|| load_thread(&gh, data, to_load_now)));
                        to_load.clear();
                    }
                }
            }

            data.set_last_id("github", last_id)?;

            if finished {
                // Ensure all the remaining repositories are loaded
                if !to_load.is_empty() {
                    let to_load_now = to_load.clone();
                    scope.spawn(|_| wrap_thread(|| load_thread(&gh, data, to_load_now)));
                }

                break;
            }

            // Avoid hammering GitHub too much
            if let Some(sleep) = Duration::from_secs(1).checked_sub(start.elapsed()) {
                ::std::thread::sleep(sleep);
            }
        }

        Ok(())
    })
    .unwrap();

    info!("finished scraping for GitHub repositories");
    result
}
