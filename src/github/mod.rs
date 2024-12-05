use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use api::Github;
use tokio::{
    signal::ctrl_c,
    task::JoinSet,
    time::{sleep, Instant},
};
use tracing::{debug, info, warn};

use crate::{
    config::Config,
    data::{Data, Forge},
};

mod api;

#[derive(Debug, Clone)]
pub struct Scraper {
    gh: Arc<Github>,
    data: Data,
    finished: Arc<AtomicBool>,
    timeout: Option<u64>,
}

impl Scraper {
    pub fn new(config: Config, data: Data) -> Self {
        let timeout = config.timeout;
        let gh = Github::new(config);

        let finished = Arc::new(AtomicBool::new(false));
        let f2 = finished.clone();

        tokio::spawn(async move {
            ctrl_c().await.expect("Failed to install Ctrl+C Handler");
            warn!("Ctrl+C received, stopping...");
            f2.store(true, Ordering::SeqCst);
        });

        Self {
            gh: Arc::new(gh),
            data,
            finished,
            timeout,
        }
    }

    async fn load_repositories(&self, repos: Vec<String>) -> color_eyre::Result<()> {
        let mut graph_repos = self.gh.load_repositories(&repos).await?;
        let mut js = JoinSet::new();
        for repo in graph_repos.drain(..) {
            if repo
                .languages
                .nodes
                .iter()
                .filter_map(Option::as_ref)
                .any(|el| el.name == "Rust")
            {
                let this = self.clone();
                js.spawn(async move {
                    let mut repo = repo.into_repo(false, false);
                    let files = this.gh.tree(&repo, false).await;
                    match files {
                        Ok(tree) => {
                            for node in tree.tree {
                                if node.path == "Cargo.toml" {
                                    repo.has_cargo_toml = true;
                                } else if node.path == "Cargo.lock" {
                                    repo.has_cargo_lock = true;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Could not get tree for {}, error: {e:?}", repo.name);
                        }
                    }

                    this.data.store_repo(Forge::Github, repo).await;
                });
            }
        }

        js.join_all().await;

        Ok(())
    }

    pub async fn scrape(&self) -> color_eyre::Result<()> {
        let start = Instant::now();

        let mut to_load = Vec::with_capacity(api::N);

        let mut last_id = self.data.get_last_id(Forge::Github);

        loop {
            let start_loop = Instant::now();
            if let Some(timeout) = self.timeout {
                if start.elapsed() >= Duration::from_secs(timeout) {
                    info!("Timeout reached, stopped scraping");
                    break;
                }
            }

            let mut repos = self.gh.scrape_repositories(last_id).await?;
            let mut js = JoinSet::new();

            for repo in repos.drain(..) {
                last_id = repo.id;
                if repo.fork {
                    continue;
                }

                to_load.push(repo.node_id);

                if to_load.len() == api::N {
                    let to_load_now = to_load.clone();
                    let this = self.clone();
                    js.spawn(async move { this.load_repositories(to_load_now).await });
                    to_load.clear();
                }
            }

            self.data.set_last_id(Forge::Github, last_id).await?;

            for res in js.join_all().await {
                if let Err(e) = res {
                    warn!("Failed scraping repo: {:?}", e);
                }
            }

            if self.finished.load(Ordering::SeqCst) {
                if !to_load.is_empty() {
                    self.load_repositories(to_load).await?;
                }
                break;
            }

            debug!("Loaded 100 repos in {}ms", start_loop.elapsed().as_millis());

            if let Some(time) = Duration::from_millis(250).checked_sub(start_loop.elapsed()) {
                sleep(time).await;
            }
        }

        info!("Took {} seconds", start.elapsed().as_secs());

        Ok(())
    }
}
