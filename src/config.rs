use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub github_token: Vec<String>,
    pub data_dir: PathBuf,
    pub timeout: Option<u64>,
}
