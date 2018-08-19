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
use crossbeam_channel::{self, Receiver, Sender};
use crossbeam_utils::thread::scope;
use data::{Data, Repo};
use github::api::{GitHubApi, GraphRepository};
use prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use utils::wrap_thread;

static WANTED_LANG: &'static str = "Rust";

enum ThreadInput<T> {
    Data(T),
    Finished,
}

fn load_thread(
    api: &GitHubApi,
    add_repo: Sender<ThreadInput<GraphRepository>>,
    recv: Receiver<ThreadInput<String>>,
) -> Fallible<()> {
    let mut to_load = Vec::with_capacity(200);
    let mut finished = false;

    for input in recv {
        match input {
            ThreadInput::Data(repo) => to_load.push(repo),
            ThreadInput::Finished => finished = true,
        }

        while to_load.len() >= 100 || (finished && !to_load.is_empty()) {
            let (to_collect, cutoff) = if to_load.len() >= 100 {
                (100, to_load.len() - 100)
            } else {
                (to_load.len(), 0)
            };

            debug!(
                "collected {} non-fork repositories, loading them",
                to_collect
            );

            let mut graph_repos = api.load_repositories(&to_load[cutoff..])?;
            for repo in graph_repos.drain(..) {
                if let Some(repo) = repo {
                    let mut found = false;
                    for lang in repo.languages.nodes.iter().filter_map(|l| l.as_ref()) {
                        if lang.name == WANTED_LANG {
                            found = true;
                            break;
                        }
                    }

                    if found {
                        add_repo.send(ThreadInput::Data(repo));
                    }
                }
            }

            to_load.truncate(cutoff);
        }

        if finished {
            add_repo.send(ThreadInput::Finished);
            break;
        }
    }

    // Applease Clippy
    ::std::mem::drop(add_repo);

    Ok(())
}

fn add_repo_thread(
    data: &Data,
    api: &GitHubApi,
    recv: Receiver<ThreadInput<GraphRepository>>,
) -> Fallible<()> {
    for input in recv {
        match input {
            ThreadInput::Data(repo) => {
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
            ThreadInput::Finished => break,
        }
    }

    Ok(())
}

pub fn scrape(data: &Data, config: &Config, should_stop: &AtomicBool) -> Fallible<()> {
    info!("started scraping for GitHub repositories");

    let gh = api::GitHubApi::new(config);

    scope(|scope| {
        let (add_repo, add_repo_recv) = crossbeam_channel::unbounded();
        let (load, load_recv) = crossbeam_channel::unbounded();

        let add_repo_thread =
            scope.spawn(|| wrap_thread(|| add_repo_thread(&data, &gh, add_repo_recv)));

        // Two threads are created because the load operation is slower
        let mut load_threads = Vec::new();
        for _ in 0..2 {
            let add_repo = add_repo.clone();
            let load_recv = load_recv.clone();
            load_threads.push(
                scope.spawn(|| wrap_thread(|| load_thread(&gh, add_repo, load_recv)))
            );
        }

        let mut last_id = data.get_last_id("github")?.unwrap_or(0);

        loop {
            debug!("scraping 100 repositories from the REST API");

            // Load all the non-fork repositories in the to_load vector
            let mut repos = gh.scrape_repositories(last_id)?;
            let finished = repos.len() < 100 || should_stop.load(Ordering::SeqCst);
            for repo in repos.drain(..) {
                if repo.fork {
                    continue;
                }

                load.send(ThreadInput::Data(repo.node_id));
                last_id = repo.id;
            }

            data.set_last_id("github", last_id)?;

            if finished {
                for _ in 0..load_threads.len() {
                    load.send(ThreadInput::Finished);
                }
                break;
            }
        }

        for thread in load_threads.drain(..) {
            thread.join().unwrap();
        }
        add_repo_thread.join().unwrap();

        info!("finished scraping for GitHub repositories");

        Ok(())
    })
}
