// Copyright (c) 2018 The Rust Project Developers
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
use data::{Data, Repo};
use github::api::{GitHubApi, GraphRepository};
use prelude::*;

static WANTED_LANG: &'static str = "Rust";

// Start around the time when rust-lang/rust was created
// There is no point in looking for rust repos before then
static STARTING_ID: usize = 724_000;

pub fn scrape(data: &mut Data, config: &Config) -> Fallible<()> {
    info!("started scraping for GitHub repositories");

    let gh = api::GitHubApi::new(config);

    let mut to_load = Vec::with_capacity(200);
    let mut last_id = if let Some(id) = data.get_last_id("github")? {
        id
    } else {
        STARTING_ID
    };

    loop {
        debug!("scraping 100 repositories from the REST API");

        // Load all the non-fork repositories in the to_load vector
        let mut repos = gh.scrape_repositories(last_id)?;
        for repo in repos.drain(..) {
            if repo.fork {
                continue;
            }

            to_load.push(repo.node_id);
            last_id = repo.id;
            data.set_last_id("github", repo.id)?;
        }

        if to_load.len() >= 100 {
            debug!("collected 100 non-fork repositories, loading them");

            let cutoff = to_load.len() - 100;
            for repo in gh.load_repositories(&to_load[cutoff..])? {
                if let Some(repo) = repo {
                    for lang in repo.languages.nodes.iter().filter_map(|l| l.as_ref()) {
                        if lang.name == WANTED_LANG {
                            add_repo(&data, &repo, &gh)?;
                        }
                    }
                }
            }

            to_load.truncate(cutoff);
        }
    }
}

fn add_repo(data: &Data, repo: &GraphRepository, api: &GitHubApi) -> Fallible<()> {
    let has_cargo_toml = api.file_exists(repo, "Cargo.toml")?;
    let has_cargo_lock = api.file_exists(repo, "Cargo.lock")?;

    data.store_repo(
        "github",
        Repo {
            id: repo.id.clone(),
            name: repo.name_with_owner.clone(),
            has_cargo_toml,
            has_cargo_lock,
        },
    )?;

    info!(
        "found {}: Cargo.toml = {:?}, Cargo.lock = {:?}",
        repo.name_with_owner, has_cargo_toml, has_cargo_lock,
    );

    Ok(())
}
