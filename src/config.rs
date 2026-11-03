use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    // GitHub rate-limits are per-token, using more than one can speed up the scraper
    pub github_token: Vec<String>,
    pub data_dir: PathBuf,
    pub timeout: Option<u64>,
}
