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

use config::Config;
use prelude::*;
use reqwest::{header, Client, Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use std::borrow::Cow;

static USER_AGENT: &'static str = "rust-ops/rust-repos";

static GRAPHQL_QUERY_REPOSITORIES: &'static str = "
query($ids: [ID!]!) {
    nodes(ids: $ids) {
        ... on Repository {
            id
            nameWithOwner
            defaultBranchRef {
                name
            }
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

pub struct GitHubApi<'conf> {
    config: &'conf Config,
    client: Client,
}

impl<'conf> GitHubApi<'conf> {
    pub fn new(config: &'conf Config) -> Self {
        GitHubApi {
            config,
            client: Client::new(),
        }
    }

    fn build_request(&self, method: Method, url: &str) -> RequestBuilder {
        let url = if !url.starts_with("https://") {
            Cow::Owned(format!("https://api.github.com/{}", url))
        } else {
            Cow::Borrowed(url)
        };

        let mut req = self.client.request(method, url.as_ref());
        req.header(header::Authorization(format!(
            "token {}",
            self.config.github_token
        )));
        req.header(header::UserAgent::new(USER_AGENT));
        req
    }

    fn graphql<T: DeserializeOwned, V: Serialize>(&self, query: &str, variables: V) -> Fallible<T> {
        let resp: GraphResponse<T> = self
            .build_request(Method::Post, "graphql")
            .json(&json!({
                "query": query,
                "variables": variables,
            })).send()?
            .json()?;

        if let Some(data) = resp.data {
            if let Some(errors) = resp.errors {
                for error in errors {
                    if let Some(ref type_) = error.type_ {
                        if type_ == "NOT_FOUND" {
                            debug!("ignored GraphQL error: {}", error.message);
                            continue;
                        }
                    }

                    warn!("non-fatal GraphQL error: {}", error.message);
                }
            }

            Ok(data)
        } else if let Some(mut errors) = resp.errors {
            return Err(err_msg(errors.pop().unwrap().message)
                .context("GitHub GraphQL call failed")
                .into());
        } else {
            bail!("neither data or errors present in the GraphQL query");
        }
    }

    pub fn scrape_repositories(&self, since: usize) -> Fallible<Vec<RestRepository>> {
        let mut resp = self
            .build_request(Method::Get, &format!("repositories?since={}", since))
            .send()?;

        let status = resp.status();
        if status == StatusCode::Ok {
            Ok(resp.json()?)
        } else {
            let error: GitHubError = resp.json()?;
            Err(err_msg(error.message)
                .context(format!(
                    "GitHub API call failed with status code: {}",
                    status
                )).context(format!(
                    "failed to fetch GitHub repositories since ID {}",
                    since
                )).into())
        }
    }

    pub fn load_repositories(&self, node_ids: &[String]) -> Fallible<Vec<Option<GraphRepository>>> {
        let data: GraphRepositories = self.graphql(
            GRAPHQL_QUERY_REPOSITORIES,
            json!({
            "ids": node_ids,
        }),
        )?;

        assert_eq!(
            data.rate_limit.cost, 1,
            "load repositories query too costly"
        );
        Ok(data.nodes)
    }

    pub fn file_exists(&self, repo: &GraphRepository, path: &str) -> Fallible<bool> {
        let url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}",
            repo.name_with_owner,
            if let Some(ref_) = &repo.default_branch_ref {
                &ref_.name
            } else {
                "master"
            },
            path,
        );

        let resp = self.build_request(Method::Get, &url).send()?;
        match resp.status() {
            StatusCode::Ok => Ok(true),
            StatusCode::NotFound => Ok(false),
            status => Err(
                err_msg(format!("GitHub API returned status code {}", status))
                    .context(format!(
                        "failed to fetch file {} from repo {}",
                        path, repo.name_with_owner,
                    )).into(),
            ),
        }
    }
}

#[derive(Deserialize)]
struct GitHubError {
    message: String,
    #[serde(rename = "type")]
    type_: Option<String>,
}

#[derive(Deserialize)]
pub struct RestRepository {
    pub id: usize,
    pub full_name: String,
    pub node_id: String,
    pub fork: bool,
}

#[derive(Deserialize)]
struct GraphResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GitHubError>>,
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
    pub default_branch_ref: Option<GraphRef>,
    pub languages: GraphLanguages,
}

#[derive(Debug, Deserialize)]
pub struct GraphLanguages {
    pub nodes: Vec<Option<GraphLanguage>>,
}

#[derive(Debug, Deserialize)]
pub struct GraphLanguage {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct GraphRef {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GitHubErrorType {
    NotFound,
    Other(String),
}
