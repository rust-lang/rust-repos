use config::Config;
use data::{Data, Repo};
use prelude::*;
use reqwest::blocking::Client;
use std::sync::atomic::{AtomicBool, Ordering};

static USER_AGENT: &str = "rust-repos (https://github.com/rust-ops/rust-repos)";

static GRAPHQL_QUERY_REPOSITORIES: &str = r#"
query ListRustRepos($after: String) {
  projects(
    first: 100
    after: $after
    programmingLanguageName: "Rust"
    archived: EXCLUDE
  ) {
    pageInfo {
      hasNextPage
      endCursor
    }
    nodes {
      id
      name
      fullPath
      path
      webUrl
      repository {
        cargoFiles: blobs(paths: ["Cargo.toml", "Cargo.lock"] ref: "HEAD") {
          nodes {
            path
          }
        }
      }
    }
  }
}
"#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String,
    name: String,
    full_path: String,
    path: String,
    web_url: String,
    repository: Option<Repository>,
}

impl Project {
    fn has_cargo_toml(&self) -> bool {
        match &self.repository {
            Some(repo) => repo
                .cargo_files
                .nodes
                .iter()
                .any(|x| x.path == "Cargo.toml"),
            None => false,
        }
    }

    fn has_cargo_lock(&self) -> bool {
        match &self.repository {
            Some(repo) => repo
                .cargo_files
                .nodes
                .iter()
                .any(|x| x.path == "Cargo.lock"),
            None => false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    cargo_files: FilesNode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilesNode {
    nodes: Vec<FilePath>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilePath {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Projects {
    page_info: PageInfo,
    nodes: Vec<Project>,
}

#[derive(Debug, Deserialize)]
struct ApiData {
    projects: Projects,
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    data: Option<ApiData>,
    errors: Option<serde_json::Value>,
}

pub fn scrape(data: &Data, config: &Config, should_stop: &AtomicBool) -> Fallible<()> {
    let client = Client::new();

    info!(
        "scraping GitLab endpoint {}",
        config.gitlab_graphql_endpoint
    );
    let mut after: Option<String> = None;
    while !should_stop.load(Ordering::SeqCst) {
        let variables = serde_json::json!({ "after": after });

        let mut request = client
            .post(&config.gitlab_graphql_endpoint)
            .header(reqwest::header::USER_AGENT, USER_AGENT);
        if let Some(token) = &config.gitlab_token {
            request = request.bearer_auth(token);
        }

        let resp: GraphQlResponse = request
            .json(&serde_json::json!({
                "query": GRAPHQL_QUERY_REPOSITORIES,
                "variables": variables
            }))
            .send()?
            .error_for_status()?
            .json()?;

        if let Some(errors) = resp.errors {
            eprintln!("GraphQL errors: {errors:#?}");
            break;
        }

        let gitlab_data = resp
            .data
            .ok_or_else(|| err_msg("GitLab GraphQL response did not contain data"))?;
        let projects = gitlab_data.projects;

        for project in projects.nodes {
            data.store_repo(
                "gitlab",
                Repo {
                    id: project.id.clone(),
                    name: project.full_path.to_string(),
                    has_cargo_toml: project.has_cargo_toml(),
                    has_cargo_lock: project.has_cargo_lock(),
                },
            )?;
        }

        if !projects.page_info.has_next_page {
            break;
        }

        after = projects.page_info.end_cursor;
    }

    Ok(())
}
