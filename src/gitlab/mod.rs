use config::Config;
use data::{Data, Repo};
use prelude::*;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};

const GITLAB_GRAPHQL_ENDPOINT: &str = "https://gitlab.com/api/graphql";

static USER_AGENT: &str = "rust-repos (https://github.com/rust-ops/rust-repos)";

static GRAPHQL_QUERY_REPOSITORIES: &str = r#"
query ListRustRepos($after: String) {
  projects(
    first: 50
    after: $after
    programmingLanguageName: "Rust"
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
struct GraphQLResponse {
    data: Option<ApiData>,
    errors: Option<serde_json::Value>,
}

pub fn scrape(data: &Data, config: &Config, should_stop: &AtomicBool) -> Fallible<()> {
    let client = Client::new();

    let mut after: Option<String> = None;
    let mut page = 1;

    while !should_stop.load(Ordering::SeqCst) {
        println!("Fetching page {page}...");

        let variables = serde_json::json!({ "after": after });

        let resp: GraphQLResponse = client
            .post(GITLAB_GRAPHQL_ENDPOINT)
            .json(&serde_json::json!({
                "query": GRAPHQL_QUERY_REPOSITORIES,
                "variables": variables
            }))
            .send()?
            .json()?;

        if let Some(errors) = resp.errors {
            eprintln!("GraphQL errors: {errors:#?}");
            break;
        }

        let gitlab_data = resp.data.expect("No data returned");
        println!("{:?}", gitlab_data);
        let projects = gitlab_data.projects;

        for project in projects.nodes {
            println!("{:?}", project);
            data.store_repo(
                "gitlab",
                Repo {
                    id: project.id.clone(),
                    name: project.full_path.to_string(),
                    has_cargo_toml: true, // TODO set
                    has_cargo_lock: true,
                },
            )?;
        }

        if !projects.page_info.has_next_page {
            println!("No more pages");
            break;
        }

        after = projects.page_info.end_cursor;
        page += 1;
    }

    Ok(())
}
