//! Pull requests (GitHub, through the `gh` CLI) and merge requests (GitLab, through `glab`):
//! listing, details with comments and files, checkout, comments.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    GitHub,
    GitLab,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// The PR's branch and the branch it merges into.
    pub head: String,
    pub base: String,
    pub url: String,
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Details {
    pub body: String,
    /// (author, text)
    pub comments: Vec<(String, String)>,
    pub files: Vec<String>,
}

fn field<'a>(v: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    v.pointer(path).unwrap_or(&serde_json::Value::Null)
}

fn text(v: &serde_json::Value, path: &str) -> String {
    field(v, path).as_str().unwrap_or("").to_string()
}

/// `gh pr list --json number,title,author,headRefName,baseRefName,url,isDraft`.
pub fn parse_github_list(json: &str) -> Vec<PullRequest> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(json) else { return Vec::new() };
    items
        .iter()
        .map(|v| PullRequest {
            number: field(v, "/number").as_u64().unwrap_or(0),
            title: text(v, "/title"),
            author: text(v, "/author/login"),
            head: text(v, "/headRefName"),
            base: text(v, "/baseRefName"),
            url: text(v, "/url"),
            draft: field(v, "/isDraft").as_bool().unwrap_or(false),
        })
        .collect()
}

/// `glab mr list --output json`.
pub fn parse_gitlab_list(json: &str) -> Vec<PullRequest> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(json) else { return Vec::new() };
    items
        .iter()
        .map(|v| PullRequest {
            number: field(v, "/iid").as_u64().unwrap_or(0),
            title: text(v, "/title"),
            author: text(v, "/author/username"),
            head: text(v, "/source_branch"),
            base: text(v, "/target_branch"),
            url: text(v, "/web_url"),
            draft: field(v, "/draft").as_bool().unwrap_or(false),
        })
        .collect()
}

/// `gh pr view N --json body,comments,files`.
pub fn parse_github_details(json: &str) -> Details {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Details::default() };
    Details {
        body: text(&v, "/body"),
        comments: field(&v, "/comments").as_array().into_iter().flatten().map(|c| (text(c, "/author/login"), text(c, "/body"))).collect(),
        files: field(&v, "/files").as_array().into_iter().flatten().map(|f| text(f, "/path")).collect(),
    }
}

fn run(root: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let output = process::command(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|_| format!("`{program}` isn't installed: {}", if program == "gh" { "https://cli.github.com, then `gh auth login`" } else { "https://gitlab.com/gitlab-org/cli, then `glab auth login`" }))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// GitHub or GitLab, from the `origin` remote.
pub fn host(remote_url: &str) -> Option<Host> {
    if remote_url.contains("github") {
        Some(Host::GitHub)
    } else if remote_url.contains("gitlab") {
        Some(Host::GitLab)
    } else {
        None
    }
}

/// Open pull / merge requests.
pub fn list(root: &Path, host: Host) -> Result<Vec<PullRequest>, String> {
    match host {
        Host::GitHub => run(root, "gh", &["pr", "list", "--limit", "100", "--json", "number,title,author,headRefName,baseRefName,url,isDraft"]).map(|j| parse_github_list(&j)),
        Host::GitLab => run(root, "glab", &["mr", "list", "--output", "json", "--per-page", "100"]).map(|j| parse_gitlab_list(&j)),
    }
}

pub fn details(root: &Path, host: Host, number: u64) -> Result<Details, String> {
    let n = number.to_string();
    match host {
        Host::GitHub => run(root, "gh", &["pr", "view", &n, "--json", "body,comments,files"]).map(|j| parse_github_details(&j)),
        Host::GitLab => run(root, "glab", &["mr", "view", &n, "--comments"]).map(|t| Details { body: t, ..Default::default() }),
    }
}

pub fn checkout(root: &Path, host: Host, number: u64) -> Result<String, String> {
    let n = number.to_string();
    match host {
        Host::GitHub => run(root, "gh", &["pr", "checkout", &n]),
        Host::GitLab => run(root, "glab", &["mr", "checkout", &n]),
    }
}

pub fn comment(root: &Path, host: Host, number: u64, body: &str) -> Result<String, String> {
    let n = number.to_string();
    match host {
        Host::GitHub => run(root, "gh", &["pr", "comment", &n, "--body", body]),
        Host::GitLab => run(root, "glab", &["mr", "note", &n, "--message", body]),
    }
}

/// Fetches the PR's head into `refs/phpcraft/pr-N` for diffing; returns that ref.
pub fn fetch_head(root: &Path, host: Host, number: u64) -> Result<String, String> {
    let local = format!("refs/phpcraft/pr-{number}");
    let remote = match host {
        Host::GitHub => format!("pull/{number}/head"),
        Host::GitLab => format!("merge-requests/{number}/head"),
    };
    run(root, "git", &["fetch", "--quiet", "origin", &format!("+{remote}:{local}")]).map(|_| local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_json() {
        let list = r#"[{"number":12,"title":"Add cart","author":{"login":"ann"},"headRefName":"cart","baseRefName":"main","url":"https://github.com/a/b/pull/12","isDraft":false}]"#;
        let prs = parse_github_list(list);
        assert_eq!((prs[0].number, prs[0].author.as_str(), prs[0].head.as_str()), (12, "ann", "cart"));
        let mr = r#"[{"iid":3,"title":"Fix","author":{"username":"bob"},"source_branch":"fix","target_branch":"develop","web_url":"https://gitlab.com/a/b/-/merge_requests/3","draft":true}]"#;
        let mrs = parse_gitlab_list(mr);
        assert_eq!((mrs[0].number, mrs[0].base.as_str(), mrs[0].draft), (3, "develop", true));
        let details = parse_github_details(r#"{"body":"Adds a cart.","comments":[{"author":{"login":"bob"},"body":"LGTM"}],"files":[{"path":"app/Cart.php"}]}"#);
        assert_eq!(details.comments, [("bob".to_string(), "LGTM".to_string())]);
        assert_eq!(details.files, ["app/Cart.php"]);
        assert_eq!(host("git@github.com:a/b.git"), Some(Host::GitHub));
    }
}
