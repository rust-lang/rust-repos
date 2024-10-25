use std::{
    borrow::Cow,
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use reqwest::{header, Client, Method, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use serde_derive::Deserialize;
use serde_json::json;
use thiserror::Error;
use tokio::{task::yield_now, time::sleep};
use tracing::{error, trace, warn};

use crate::{config::Config, data::Repo};

static USER_AGENT: &str = "rust-repos (https://github.com/rust-lang/rust-repos)";

#[derive(Debug)]
pub struct Github {
    client: Client,
    config: Config,
    current_token_index: AtomicUsize,
}

#[derive(Debug, Deserialize)]
pub struct GitHubError {
    message: String,

    #[allow(unused)]
    r#type: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Node {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct GithubTree {
    pub tree: Vec<Node>,
}

#[derive(Debug, Deserialize)]
pub struct RestRepository {
    pub id: usize,
    // Useful for debugging, if something does go wrong
    #[allow(unused)]
    pub full_name: String,
    pub node_id: String,
    pub fork: bool,
}

#[derive(Deserialize)]
struct GraphResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GitHubError>>,
    #[allow(unused)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct GraphRateLimit {
    cost: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphRepositories {
    nodes: Vec<Option<GraphRepository>>,
    rate_limit: GraphRateLimit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRepository {
    pub id: String,
    pub name_with_owner: String,
    pub languages: GraphLanguages,
}

impl GraphRepository {
    pub fn into_repo(self, has_cargo_toml: bool, has_cargo_lock: bool) -> Repo {
        Repo {
            id: self.id,
            name: self.name_with_owner,
            has_cargo_toml,
            has_cargo_lock,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GraphLanguages {
    pub nodes: Vec<Option<GraphLanguage>>,
}

#[derive(Debug, Deserialize)]
pub struct GraphLanguage {
    pub name: String,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("reqwest error occurred {0:?}")]
    Reqwest(#[from] reqwest::Error),
    #[error("rate limit hit {0}")]
    RateLimit(StatusCode),
    #[error("other http error: {0}")]
    HttpStatus(StatusCode),

    #[error("Response did not contain requested data")]
    EmptyData,
    #[error("IO Error {0}")]
    Io(#[from] std::io::Error),
}

/// 100 is the max results per page of the GH API
pub(crate) const N: usize = 100;

const GRAPHQL_QUERY_REPOSITORIES: &str = "
query($ids: [ID!]!) {
    nodes(ids: $ids) {
        ... on Repository {
            id
            nameWithOwner
            languages(first: 100, orderBy: { field: SIZE, direction: DESC }) {
                nodes {
                    name
                }
            }
        }
    }

    rateLimit {
        cost
    }
}
";

impl Github {
    pub fn new(config: Config) -> Self {
        Github {
            client: Client::new(),
            current_token_index: AtomicUsize::new(0),
            config,
        }
    }

    #[inline]
    fn get_token(&self) -> &str {
        &self.config.github_token[self.current_token_index.load(Ordering::Relaxed)]
    }

    fn build_request(&self, method: Method, url: &str) -> RequestBuilder {
        let url = if url.starts_with("https://") {
            Cow::from(url)
        } else {
            Cow::from(format!("https://api.github.com/{url}"))
        };
        trace!("Sending request to {url}");
        self.client
            .request(method, url.as_ref())
            .header(header::AUTHORIZATION, format!("token {}", self.get_token()))
            .header(header::USER_AGENT, USER_AGENT)
        // .header(header::ACCEPT, "application/vnd.github+json")
    }

    async fn graphql<T: DeserializeOwned, S: Serialize>(
        &self,
        query: &str,
        variables: S,
    ) -> Result<T, Error> {
        let resp = self
            .build_request(Method::POST, "graphql")
            .json(&json!({
                "query": query,
                "variables": variables,
            }))
            .send()
            .await?;

        let data: GraphResponse<T> = handle_response_json(resp).await?;

        if let Some(errs) = data.errors {
            let errs: Vec<_> = errs
                .into_iter()
                // Some repos on GitHub are just marked as NOT_FOUND, does not seem like our fault
                .filter(|el| el.r#type.as_deref() != Some("NOT_FOUND"))
                .collect();
            if !errs.is_empty() {
                warn!("GraphQL Errors: \n {:#?}", errs);
            }
        }

        data.data.ok_or_else(|| Error::EmptyData)
    }

    pub async fn load_repositories(
        &self,
        node_ids: &[String],
    ) -> Result<Vec<GraphRepository>, Error> {
        let data: GraphRepositories = self
            .retry(|| async {
                self.graphql(
                    GRAPHQL_QUERY_REPOSITORIES,
                    json!({
                        "ids": node_ids,
                    }),
                )
                .await
            })
            .await?;

        assert!(
            data.rate_limit.cost <= 1,
            "load repositories query too costly"
        );

        Ok(data.nodes.into_iter().flatten().collect())
    }

    /// gets a file tree of a specific github repo
    pub async fn tree(&self, repo: &Repo, recursive: bool) -> Result<GithubTree, Error> {
        let mut url = format!("repos/{}/git/trees/HEAD", repo.name);
        if recursive {
            url = format!("{url}?recursive=1");
        }

        self.retry(|| async {
            let resp = self.build_request(Method::GET, &url).send().await?;

            handle_response_json(resp).await
        })
        .await
    }

    /// scrapes all github repos (paginated)
    pub async fn scrape_repositories(&self, since: usize) -> Result<Vec<RestRepository>, Error> {
        // Maybe needs to be a Vec<Option<RestRepository>>
        let output: Vec<RestRepository> = self
            .retry(|| async {
                let resp = self
                    .build_request(
                        Method::GET,
                        &format!("repositories?since={since}&per_page{N}"),
                    )
                    .send()
                    .await?;

                handle_response_json(resp).await
            })
            .await?;

        if output.len() != N {
            warn!("Github API returned {} instead of {N} repos", output.len());
        }

        Ok(output)
    }

    /// retry a github api request and rotate tokens to circumvent rate limiting
    /// On reqwest errors does exponential backoff until 5 mins.
    async fn retry<F, Fu, R>(&self, fun: F) -> Result<R, Error>
    where
        F: Fn() -> Fu,
        Fu: Future<Output = Result<R, Error>>,
    {
        let mut backoff = Duration::from_secs(1);
        loop {
            match fun().await {
                ok @ Ok(_) => return ok,
                Err(Error::Reqwest(reqwest_error)) => {
                    warn!("Reqwest encountered error {reqwest_error:?}");
                    warn!("Backing off for {} seconds", backoff.as_secs());
                    sleep(backoff).await;

                    backoff = backoff + backoff + Duration::from_millis(123); // Exponential backoff + jitter

                    // After 5 minutes bail
                    if backoff.as_secs() > 300 {
                        error!("Failed sending request 5 times");
                        return Err(Error::Reqwest(reqwest_error));
                    }
                }
                Err(err @ Error::HttpStatus(_)) => return Err(err),
                Err(Error::RateLimit(_)) => {
                    let mut wait = false;
                    self.current_token_index
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                            if old + 1 >= self.config.github_token.len() {
                                wait = true;
                                Some(0)
                            } else {
                                Some(old + 1)
                            }
                        })
                        .unwrap();

                    if wait {
                        warn!("Tokens wrapped around, sleeping for 1 minute");
                        sleep(Duration::from_secs(60)).await;
                    }
                }
                err @ Err(_) => return err,
            }

            // Yield
            yield_now().await;
        }
    }
}

async fn handle_response_json<T: DeserializeOwned>(resp: Response) -> Result<T, Error> {
    let res = handle_response(resp).await?.json().await?;
    Ok(res)
}

/// Converts github responses into the correct error codes (helper for the retry function)
async fn handle_response(resp: Response) -> Result<Response, Error> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::UNPROCESSABLE_ENTITY
    {
        warn!("Rate limit hit");
        Err(Error::RateLimit(status))
    } else if let Ok(error) = resp.json().await {
        let error: GitHubError = error;
        if error.message.contains("abuse") || error.message.contains("rate limit") {
            warn!("Rate limit hit ({}): {}", status.as_u16(), error.message);
            Err(Error::RateLimit(status))
        } else {
            warn!("Http Error ({}): {}", status.as_u16(), error.message);
            Err(Error::HttpStatus(status))
        }
    } else {
        Err(Error::HttpStatus(status))
    }
}
