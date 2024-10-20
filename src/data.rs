use enum_map::{Enum, EnumMap};
use serde_derive::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::task::{spawn_blocking, JoinSet};
use tracing::{debug, info, trace};

use crate::config::Config;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::{
    fs::{self, File},
    io::{prelude::*, BufWriter},
};

#[derive(Debug, Enum, Serialize, Deserialize, Copy, Clone)]
pub enum Forge {
    Github,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State(EnumMap<Forge, AtomicUsize>);

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Repo {
    pub id: String,
    pub name: String,
    pub has_cargo_toml: bool,
    pub has_cargo_lock: bool,
}

#[derive(Debug, Clone)]
pub struct Data {
    data_dir: PathBuf,

    state_lock: Arc<Mutex<()>>,

    state_cache: Arc<State>,

    repos_state: Arc<Mutex<EnumMap<Forge, BTreeMap<String, Repo>>>>,
}

impl Data {
    pub fn new(config: &Config) -> color_eyre::Result<Self> {
        fs::create_dir_all(&config.data_dir)?;

        let mut data = Data {
            data_dir: config.data_dir.clone(),

            state_lock: Arc::new(Mutex::new(())),
            state_cache: Arc::new(State::default()),
            repos_state: Arc::new(Mutex::new(EnumMap::default())),
        };

        let state_path = data.state_path();
        if state_path.exists() {
            let state_cache: State = serde_json::from_slice(&fs::read(&state_path)?)?;

            data.state_cache = Arc::new(state_cache)
        }

        Ok(data)
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }

    pub fn csv_path(&self, forge: Forge) -> PathBuf {
        match forge {
            Forge::Github => self.data_dir.join("github.csv"),
        }
    }

    pub fn get_last_id(&self, forge: Forge) -> usize {
        self.state_cache.0[forge].load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Store the state cache to disk, i.e. last fetched ids
    async fn store_state_cache(&self) -> color_eyre::Result<()> {
        let state = self.state_cache.clone();
        let lock = self.state_lock.clone();
        let state_path = self.state_path();
        spawn_blocking(move || -> color_eyre::Result<()> {
            let guard = lock.blocking_lock();

            let file = File::create(state_path)?;
            let mut file = BufWriter::new(file);
            serde_json::to_writer_pretty(&mut file, state.as_ref())?;
            file.write_all(b"\n")?;

            drop(guard);

            Ok(())
        })
        .await
        .unwrap()
    }

    /// Stores the repos found to disk in a CSV
    async fn store_csv(&self) -> color_eyre::Result<()> {
        debug!("storing csv file");
        let mut repos = self.repos_state.lock().await;

        let mut js = JoinSet::new();

        for (forge, repos) in repos.iter() {
            let path = self.csv_path(forge);
            let repos = repos.clone(); // is this necessary?
            js.spawn_blocking(|| -> color_eyre::Result<()> {
                let mut write_headers = false;
                if !path.exists() {
                    File::create(&path)?;
                    write_headers = true;
                }

                let file = OpenOptions::new().append(true).open(path)?;

                let mut writer = csv::WriterBuilder::new()
                    .has_headers(write_headers)
                    .from_writer(file);

                for (_, repo) in repos {
                    writer.serialize(repo)?;
                }

                Ok(())
            });
        }

        js.join_all().await.into_iter().collect::<Result<(), _>>()?;

        // Clear the map
        repos.iter_mut().for_each(|(_, m)| m.clear());

        Ok(())
    }

    pub async fn set_last_id(&self, forge: Forge, n: usize) -> color_eyre::Result<()> {
        self.state_cache.0[forge].store(n, std::sync::atomic::Ordering::SeqCst);

        self.store_csv().await?;
        self.store_state_cache().await?;

        Ok(())
    }

    pub async fn store_repo(&self, forge: Forge, repo: Repo) {
        let mut repos_state = self.repos_state.lock().await;
        repos_state[forge].insert(repo.name.clone(), repo);
    }
}
