//! Git integration through the `git` command line (identical behaviour on macOS, Windows
//! and Linux): status, diffs, staging, commits, branches, log, blame, push/pull.

use std::ops::Range;
use std::path::{Path, PathBuf};

use similar::{ChangeTag, DiffOp, TextDiff};

#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl Change {
    pub fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Path relative to the repository root, `/`-separated.
    pub path: String,
    pub change: Change,
    /// Has changes in the index.
    pub staged: bool,
    /// Has changes in the working tree (or is untracked).
    pub unstaged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    /// Branch/tag names pointing here (`HEAD -> main, origin/main`).
    pub refs: String,
    pub parents: Vec<String>,
}

/// Filters for the Git Log (all optional).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Matches the message, or a commit hash prefix.
    pub text: String,
    pub author: String,
    /// A branch or ref; empty means all branches.
    pub branch: String,
    /// Only commits touching this path.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetails {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
}

/// Lane segments for one row of the commit graph. Each segment joins a lane position at
/// one edge of the row to a lane position at the row's middle (top half) or from the middle
/// to the bottom edge (bottom half). Lanes are colour-indexed by their position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane holding the commit's dot.
    pub column: usize,
    /// (lane at top edge, lane at middle)
    pub top: Vec<(usize, usize)>,
    /// (lane at middle, lane at bottom edge)
    pub bottom: Vec<(usize, usize)>,
    /// Number of lanes used by this row (for layout width).
    pub width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub short: String,
    pub author: String,
    /// `2026-11-01`
    pub date: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChange {
    Added,
    Modified,
    /// Lines were removed just above this line.
    DeletedAbove,
}

impl Repo {
    /// The repository containing `path`, if any.
    /// Looks for `.git` (a folder, or a file in worktrees and submodules) instead of running
    /// `git rev-parse`: opening a project shouldn't wait for a process to start.
    pub fn discover(path: &Path) -> Option<Self> {
        let root = path.ancestors().find(|dir| dir.join(".git").exists())?;
        // A canonicalized Windows path (`\\?\C:\…`) as a plain one, the form `git` itself reports.
        let plain = root.to_str().and_then(|s| s.strip_prefix(r"\\?\")).filter(|s| s.as_bytes().get(1) == Some(&b':'));
        Some(Self { root: plain.map_or_else(|| root.to_path_buf(), PathBuf::from) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Commits from `base` (exclusive) to HEAD, oldest first: the interactive rebase list.
    pub fn commits_since(&self, base: &str) -> Result<Vec<(String, String)>, String> {
        let range = format!("{base}..HEAD");
        let out = self.git(&["log", "--reverse", "--format=%H%x00%s", &range])?;
        Ok(out.lines().filter_map(|l| l.split_once('\0')).map(|(h, s)| (h.to_string(), s.to_string())).collect())
    }

    /// Runs `git rebase -i base` with the given steps instead of opening an editor: the todo
    /// list comes from `steps`, squash messages are kept as git combines them, and a reword
    /// becomes an `exec git commit --amend -m …` after its pick.
    pub fn rebase_interactive(&self, base: &str, steps: &[RebaseStep]) -> Result<String, String> {
        let git_dir = self.git(&["rev-parse", "--absolute-git-dir"])?.trim().to_string();
        let todo_file = Path::new(&git_dir).join("phpcraft-rebase-todo");
        std::fs::write(&todo_file, rebase_todo(steps)).map_err(|e| e.to_string())?;
        let copy = format!("cp '{}'", todo_file.to_string_lossy().replace('\\', "/").replace('\'', "'\\''"));
        let output = process::command("git")
            .args(["rebase", "-i", base])
            .current_dir(&self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_SEQUENCE_EDITOR", copy)
            .env("GIT_EDITOR", "true")
            .output()
            .map_err(|e| format!("failed to run git: {e}"))?;
        let _ = std::fs::remove_file(&todo_file);
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        if output.status.success() { Ok(text) } else { Err(text) }
    }

    /// Any git command in the repository (the Git menu's simpler items).
    pub fn run(&self, args: &[String]) -> Result<String, String> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        self.git(&args)
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = process::command("git")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            // Background `git status` must not take the index lock (it would fight the user's
            // own git commands and rewrite .git/index every poll).
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|e| format!("failed to run git: {e}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Err(if stderr.is_empty() { stdout } else { stderr })
        }
    }

    /// Runs git and returns stdout+stderr (for push/pull, whose progress goes to stderr).
    fn git_verbose(&self, args: &[&str]) -> Result<String, String> {
        let output = process::command("git")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("failed to run git: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        if output.status.success() { Ok(text) } else { Err(text) }
    }

    /// Path relative to the repository root with `/` separators.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(&self.root).ok().or_else(|| {
            // Symlinked temp dirs (macOS /var -> /private/var) differ textually.
            let canonical = std::fs::canonicalize(path).ok()?;
            let root = std::fs::canonicalize(&self.root).ok()?;
            let rel = canonical.strip_prefix(&root).ok()?.to_path_buf();
            Some(Box::leak(rel.into_boxed_path()))
        })?;
        Some(relative.to_string_lossy().replace('\\', "/"))
    }

    pub fn current_branch(&self) -> Option<String> {
        let name = self.git(&["rev-parse", "--abbrev-ref", "HEAD"]).ok()?.trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    pub fn status(&self) -> Result<Vec<FileStatus>, String> {
        let output = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        Ok(parse_status(&output))
    }

    /// Local branches, then remote ones (`origin/main`).
    pub fn branches(&self) -> Result<(Vec<String>, Vec<String>), String> {
        let local = self.git(&["branch", "--format=%(refname:short)"])?;
        let remote = self.git(&["branch", "-r", "--format=%(refname:short)"]).unwrap_or_default();
        let lines = |s: &str| s.lines().map(str::trim).filter(|l| !l.is_empty() && !l.ends_with("/HEAD")).map(str::to_string).collect();
        Ok((lines(&local), lines(&remote)))
    }

    pub fn checkout(&self, branch: &str) -> Result<String, String> {
        // `origin/feature` -> track it as a local `feature`.
        if let Some((_, local)) = branch.split_once('/')
            && self.branches().map(|(l, _)| !l.iter().any(|b| b == local)).unwrap_or(false)
            && self.git(&["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{branch}")]).is_ok()
        {
            return self.git_verbose(&["checkout", "-b", local, "--track", branch]);
        }
        self.git_verbose(&["checkout", branch])
    }

    pub fn create_branch(&self, name: &str) -> Result<String, String> {
        self.git_verbose(&["checkout", "-b", name])
    }

    /// New branch starting at `base` (a branch, tag or commit), checked out.
    pub fn create_branch_from(&self, name: &str, base: &str) -> Result<String, String> {
        self.git_verbose(&["checkout", "-b", name, base])
    }

    pub fn delete_branch(&self, name: &str, force: bool) -> Result<String, String> {
        self.git_verbose(&["branch", if force { "-D" } else { "-d" }, name])
    }

    pub fn rename_branch(&self, old: &str, new: &str) -> Result<String, String> {
        self.git_verbose(&["branch", "-m", old, new])
    }

    /// Merges `branch` into the current branch.
    pub fn merge(&self, branch: &str) -> Result<String, String> {
        self.git_verbose(&["merge", "--no-edit", branch])
    }

    /// Commits ahead/behind the upstream, if there is one.
    pub fn ahead_behind(&self) -> Option<(usize, usize)> {
        let out = self.git(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).ok()?;
        let mut parts = out.split_whitespace().filter_map(|p| p.parse().ok());
        Some((parts.next()?, parts.next()?))
    }

    pub fn stage(&self, paths: &[String]) -> Result<(), String> {
        let mut args = vec!["add", "-A", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.git(&args).map(|_| ())
    }

    pub fn unstage(&self, paths: &[String]) -> Result<(), String> {
        let mut args = vec!["reset", "-q", "HEAD", "--"];
        args.extend(paths.iter().map(String::as_str));
        // A repository without commits has no HEAD: fall back to removing from the index.
        self.git(&args).map(|_| ()).or_else(|_| {
            let mut args = vec!["rm", "--cached", "-q", "-r", "--"];
            args.extend(paths.iter().map(String::as_str));
            self.git(&args).map(|_| ())
        })
    }

    /// Reverts working-tree changes (Rollback). Untracked files are deleted.
    pub fn discard(&self, files: &[FileStatus]) -> Result<(), String> {
        let tracked: Vec<&str> = files.iter().filter(|f| f.change != Change::Untracked).map(|f| f.path.as_str()).collect();
        if !tracked.is_empty() {
            let mut args = vec!["checkout", "HEAD", "--"];
            args.extend(tracked);
            self.git(&args)?;
        }
        for file in files.iter().filter(|f| f.change == Change::Untracked) {
            std::fs::remove_file(self.root.join(&file.path)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Commits exactly `paths` (staging them first), optionally amending the last commit.
    pub fn commit(&self, message: &str, paths: &[String], amend: bool) -> Result<String, String> {
        if message.trim().is_empty() && !amend {
            return Err("Commit message is empty".into());
        }
        if paths.is_empty() && !amend {
            return Err("No files selected".into());
        }
        if !paths.is_empty() {
            self.stage(paths)?;
        }
        let mut args = vec!["commit", "-q"];
        if amend {
            args.push("--amend");
            if message.trim().is_empty() {
                args.push("--no-edit");
            }
        }
        if !message.trim().is_empty() {
            args.extend(["-m", message]);
        }
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
        self.git(&args)?;
        Ok(self.git(&["log", "-1", "--format=%h %s"]).unwrap_or_default().trim().to_string())
    }

    pub fn last_commit_message(&self) -> Option<String> {
        self.git(&["log", "-1", "--format=%B"]).ok().map(|m| m.trim().to_string())
    }

    pub fn push(&self) -> Result<String, String> {
        let branch = self.current_branch().unwrap_or_default();
        let has_upstream = self.git(&["rev-parse", "--abbrev-ref", "@{upstream}"]).is_ok();
        if has_upstream {
            self.git_verbose(&["push"])
        } else {
            self.git_verbose(&["push", "-u", "origin", &branch])
        }
    }

    pub fn pull(&self) -> Result<String, String> {
        self.git_verbose(&["pull", "--ff-only"])
    }

    pub fn fetch(&self) -> Result<String, String> {
        self.git_verbose(&["fetch", "--all", "--prune"])
    }

    pub fn log(&self, limit: usize) -> Result<Vec<Commit>, String> {
        self.log_filtered(&LogFilter::default(), limit)
    }

    /// Commits newest first (topological, all branches unless filtered).
    pub fn log_filtered(&self, filter: &LogFilter, limit: usize) -> Result<Vec<Commit>, String> {
        let limit = format!("-{limit}");
        let mut args: Vec<String> = vec![
            "log".into(),
            limit,
            "--topo-order".into(),
            "--date=format:%d.%m.%y, %H:%M".into(),
            "--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s%x1f%D%x1f%P%x1e".into(),
        ];
        let text = filter.text.trim();
        let is_hash = text.len() >= 4 && text.chars().all(|c| c.is_ascii_hexdigit());
        if !text.is_empty() && !is_hash {
            args.extend(["-i".into(), format!("--grep={text}")]);
        }
        if !filter.author.trim().is_empty() {
            args.push(format!("--author={}", filter.author.trim()));
        }
        if filter.branch.trim().is_empty() {
            args.push("--all".into());
        } else {
            args.push(filter.branch.trim().to_string());
        }
        if !filter.path.trim().is_empty() {
            args.extend(["--".into(), filter.path.trim().to_string()]);
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        // A repository without commits has nothing to log.
        let output = self.git(&refs).unwrap_or_default();
        let mut commits: Vec<Commit> = output
            .split('\u{1e}')
            .filter_map(|record| {
                let mut f = record.trim_start_matches('\n').split('\u{1f}');
                Some(Commit {
                    hash: f.next()?.to_string(),
                    short: f.next()?.to_string(),
                    author: f.next()?.to_string(),
                    date: f.next()?.to_string(),
                    subject: f.next()?.to_string(),
                    refs: f.next().unwrap_or_default().trim().to_string(),
                    parents: f.next().unwrap_or_default().split_whitespace().map(str::to_string).collect(),
                })
            })
            .collect();
        if is_hash {
            commits.retain(|c| c.hash.starts_with(text));
        }
        Ok(commits)
    }

    pub fn commit_details(&self, hash: &str) -> Option<CommitDetails> {
        let out = self.git(&["show", "-s", "--date=format:%d.%m.%Y %H:%M", "--format=%H%x1f%an%x1f%ae%x1f%ad%x1f%B", hash]).ok()?;
        let mut f = out.split('\u{1f}');
        Some(CommitDetails {
            hash: f.next()?.to_string(),
            author: f.next()?.to_string(),
            email: f.next()?.to_string(),
            date: f.next()?.to_string(),
            message: f.next().unwrap_or_default().trim().to_string(),
        })
    }

    pub fn tags(&self) -> Vec<String> {
        self.git(&["tag", "--sort=-creatordate"]).map(|o| o.lines().map(str::to_string).collect()).unwrap_or_default()
    }

    /// Local branches by most recent commit.
    pub fn recent_branches(&self, limit: usize) -> Vec<String> {
        self.git(&["for-each-ref", "--sort=-committerdate", &format!("--count={limit}"), "--format=%(refname:short)", "refs/heads"])
            .map(|o| o.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Upstream of a local branch (`origin/main`), if tracked.
    pub fn upstream(&self, branch: &str) -> Option<String> {
        let out = self.git(&["rev-parse", "--abbrev-ref", &format!("{branch}@{{upstream}}")]).ok()?;
        Some(out.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Detached checkout of a tag or commit.
    pub fn checkout_revision(&self, revision: &str) -> Result<String, String> {
        self.git_verbose(&["checkout", "--detach", revision])
    }

    /// `git clone <url> <dir>` (blocking).
    pub fn clone_into(url: &str, dir: &Path) -> Result<String, String> {
        let output = process::command("git")
            .args(["clone", url])
            .arg(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("failed to run git: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        if output.status.success() { Ok(text) } else { Err(text) }
    }

    /// Files changed by a commit: (change, path).
    pub fn commit_files(&self, hash: &str) -> Result<Vec<(Change, String)>, String> {
        let output = self.git(&["show", "--name-status", "--format=", "-z", hash])?;
        let mut parts = output.split('\0').filter(|s| !s.is_empty());
        let mut files = Vec::new();
        while let Some(status) = parts.next() {
            let change = match status.chars().next() {
                Some('A') => Change::Added,
                Some('D') => Change::Deleted,
                Some('R') | Some('C') => {
                    parts.next(); // old name
                    Change::Renamed
                }
                _ => Change::Modified,
            };
            if let Some(path) = parts.next() {
                files.push((change, path.to_string()));
            }
        }
        Ok(files)
    }

    /// File contents at a revision (`HEAD`, a hash, or `:` for the index); `None` if absent.
    pub fn show(&self, revision: &str, path: &str) -> Option<String> {
        self.git(&["show", &format!("{revision}:{path}")]).ok()
    }

    pub fn blame(&self, path: &str) -> Result<Vec<BlameLine>, String> {
        let output = self.git(&["blame", "--line-porcelain", "--date=short", "--", path])?;
        Ok(parse_blame(&output))
    }
}

fn parse_status(output: &str) -> Vec<FileStatus> {
    let mut files = Vec::new();
    let mut entries = output.split('\0').filter(|e| e.len() > 3);
    while let Some(entry) = entries.next() {
        let bytes = entry.as_bytes();
        let (x, y) = (bytes[0] as char, bytes[1] as char);
        let path = entry[3..].to_string();
        if x == 'R' || x == 'C' {
            entries.next(); // original path
        }
        let change = match (x, y) {
            ('?', '?') => Change::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => Change::Conflicted,
            ('A', _) => Change::Added,
            ('R', _) | ('C', _) => Change::Renamed,
            ('D', _) | (_, 'D') => Change::Deleted,
            _ => Change::Modified,
        };
        files.push(FileStatus {
            path,
            change,
            staged: !matches!(x, ' ' | '?'),
            unstaged: !matches!(y, ' '),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

fn parse_blame(output: &str) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut current = BlameLine { short: String::new(), author: String::new(), date: String::new(), summary: String::new() };
    for line in output.lines() {
        if let Some(code) = line.strip_prefix('\t') {
            let _ = code;
            lines.push(current.clone());
        } else if let Some(author) = line.strip_prefix("author ") {
            current.author = author.to_string();
        } else if let Some(time) = line.strip_prefix("author-time ") {
            current.date = time.parse::<i64>().map(format_date).unwrap_or_default();
        } else if let Some(summary) = line.strip_prefix("summary ") {
            current.summary = summary.to_string();
        } else if line.len() >= 40 && line.as_bytes()[..40].iter().all(u8::is_ascii_hexdigit) {
            current.short = line[..8].to_string();
            if current.short.chars().all(|c| c == '0') {
                current.short = "--------".into();
                current.author = "Not committed".into();
            }
        }
    }
    lines
}

/// Unix seconds -> `YYYY-MM-DD` (UTC).
fn format_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Lays commits (newest first, with parents) out in lanes for drawing a graph.
pub fn graph(commits: &[Commit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let column = match lanes.iter().position(|l| l.as_deref() == Some(commit.hash.as_str())) {
            Some(i) => i,
            None => match lanes.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };
        let mut row = GraphRow { column, ..Default::default() };
        // Top half: lanes waiting for this commit converge on its dot; others pass through.
        for (i, lane) in lanes.iter().enumerate() {
            match lane {
                Some(h) if *h == commit.hash => row.top.push((i, column)),
                Some(_) => row.top.push((i, i)),
                None => {}
            }
        }
        for lane in lanes.iter_mut() {
            if lane.as_deref() == Some(commit.hash.as_str()) {
                *lane = None;
            }
        }
        // The first parent continues in this column; others join or open lanes.
        let mut targets = Vec::new();
        for (n, parent) in commit.parents.iter().enumerate() {
            if n == 0 {
                if let Some(existing) = lanes.iter().position(|l| l.as_deref() == Some(parent.as_str())) {
                    targets.push(existing);
                } else {
                    lanes[column] = Some(parent.clone());
                    targets.push(column);
                }
                continue;
            }
            match lanes.iter().position(|l| l.as_deref() == Some(parent.as_str())) {
                Some(existing) => targets.push(existing),
                None => {
                    let free = lanes.iter().enumerate().position(|(i, l)| l.is_none() && i != column);
                    let i = free.unwrap_or_else(|| {
                        lanes.push(None);
                        lanes.len() - 1
                    });
                    lanes[i] = Some(parent.clone());
                    targets.push(i);
                }
            }
        }
        for (i, lane) in lanes.iter().enumerate() {
            if lane.is_some() && !targets.contains(&i) {
                row.bottom.push((i, i));
            }
        }
        for target in targets {
            row.bottom.push((column, target));
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        row.width = lanes.len().max(column + 1).max(row.top.iter().map(|(a, _)| a + 1).max().unwrap_or(0));
        rows.push(row);
    }
    rows
}

/// Per-line markers for the editor gutter, comparing `base` (e.g. HEAD) to `current`.
/// Returns (0-based line, change) for the current text.
pub fn line_changes(base: &str, current: &str) -> Vec<(usize, LineChange)> {
    let diff = TextDiff::from_lines(base, current);
    let mut out = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Insert { new_index, new_len, .. } => out.extend((new_index..new_index + new_len).map(|l| (l, LineChange::Added))),
            DiffOp::Replace { new_index, new_len, .. } => out.extend((new_index..new_index + new_len).map(|l| (l, LineChange::Modified))),
            DiffOp::Delete { new_index, .. } => out.push((new_index, LineChange::DeletedAbove)),
            DiffOp::Equal { .. } => {}
        }
    }
    out
}

/// One row of a side-by-side diff: left/right line numbers (0-based) and texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub left: Option<(usize, String)>,
    pub right: Option<(usize, String)>,
    pub tag: RowTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowTag {
    Equal,
    Removed,
    Added,
    Changed,
}

/// Aligns two texts into side-by-side rows; long unchanged stretches are kept (the view
/// scrolls), changed blocks pair removed and added lines.
pub fn side_by_side(old: &str, new: &str) -> Vec<DiffRow> {
    let diff = TextDiff::from_lines(old, new);
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let line = |lines: &[&str], i: usize| lines.get(i).map(|s| s.to_string()).unwrap_or_default();
    let mut rows = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { old_index, new_index, len } => {
                for i in 0..len {
                    rows.push(DiffRow {
                        left: Some((old_index + i, line(&old_lines, old_index + i))),
                        right: Some((new_index + i, line(&new_lines, new_index + i))),
                        tag: RowTag::Equal,
                    });
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                for i in 0..old_len {
                    rows.push(DiffRow { left: Some((old_index + i, line(&old_lines, old_index + i))), right: None, tag: RowTag::Removed });
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                for i in 0..new_len {
                    rows.push(DiffRow { left: None, right: Some((new_index + i, line(&new_lines, new_index + i))), tag: RowTag::Added });
                }
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                for i in 0..old_len.max(new_len) {
                    rows.push(DiffRow {
                        left: (i < old_len).then(|| (old_index + i, line(&old_lines, old_index + i))),
                        right: (i < new_len).then(|| (new_index + i, line(&new_lines, new_index + i))),
                        tag: RowTag::Changed,
                    });
                }
            }
        }
    }
    rows
}

/// Changed character ranges within a modified line pair (for inline highlighting).
pub fn inline_changes(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let diff = TextDiff::from_chars(old, new);
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let (mut o, mut n) = (0, 0);
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                o += len;
                n += len;
            }
            ChangeTag::Delete => {
                left.push(o..o + len);
                o += len;
            }
            ChangeTag::Insert => {
                right.push(n..n + len);
                n += len;
            }
        }
    }
    (left, right)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseAction {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl RebaseAction {
    pub const ALL: [RebaseAction; 5] = [RebaseAction::Pick, RebaseAction::Reword, RebaseAction::Squash, RebaseAction::Fixup, RebaseAction::Drop];

    pub fn name(self) -> &'static str {
        match self {
            RebaseAction::Pick => "pick",
            RebaseAction::Reword => "reword",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "drop",
        }
    }
}

/// One line of an interactive rebase, in the order the commits will be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseStep {
    pub action: RebaseAction,
    pub hash: String,
    pub subject: String,
    /// The new message for `Reword`.
    pub message: String,
}

/// The todo list git reads: rewords as a pick plus an amend with the new message.
pub fn rebase_todo(steps: &[RebaseStep]) -> String {
    let mut out = String::new();
    for step in steps {
        match step.action {
            RebaseAction::Reword => {
                out.push_str(&format!("pick {} {}\n", step.hash, step.subject));
                let message = step.message.replace('\'', "'\\''");
                out.push_str(&format!("exec git commit --amend --allow-empty --no-verify -m '{message}'\n"));
            }
            action => out.push_str(&format!("{} {} {}\n", action.name(), step.hash, step.subject)),
        }
    }
    out
}

/// A shelved set of changes (`.phpcraft/shelf/<id>/`): `changes.patch` for tracked files,
/// copies of new files under `new/`, and `info` with the name and file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    pub id: String,
    pub name: String,
    pub files: Vec<String>,
}

/// Shelves in `dir`, newest first.
pub fn shelves(dir: &Path) -> Vec<Shelf> {
    let mut out: Vec<Shelf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let info = std::fs::read_to_string(e.path().join("info")).ok()?;
            let mut lines = info.lines();
            let name = lines.next()?.to_string();
            Some(Shelf { id: e.file_name().to_string_lossy().into_owned(), name, files: lines.map(str::to_string).collect() })
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

impl Repo {
    /// Shelves `files` (relative paths): saves their changes into `dir` and reverts them.
    pub fn shelve(&self, dir: &Path, name: &str, files: &[String]) -> Result<Shelf, String> {
        let id = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
        let shelf = dir.join(&id);
        std::fs::create_dir_all(shelf.join("new")).map_err(|e| e.to_string())?;
        let untracked: Vec<String> = self.git(&["ls-files", "--others", "--exclude-standard", "--"]).unwrap_or_default().lines().map(str::to_string).collect();
        let (new, tracked): (Vec<&String>, Vec<&String>) = files.iter().partition(|f| untracked.contains(f));
        if !tracked.is_empty() {
            let mut args = vec!["diff", "--binary", "HEAD", "--"];
            args.extend(tracked.iter().map(|s| s.as_str()));
            let patch = self.git(&args)?;
            std::fs::write(shelf.join("changes.patch"), patch).map_err(|e| e.to_string())?;
        }
        for file in &new {
            let target = shelf.join("new").join(file);
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::copy(self.root.join(file), &target).map_err(|e| e.to_string())?;
        }
        std::fs::write(shelf.join("info"), format!("{name}\n{}", files.join("\n"))).map_err(|e| e.to_string())?;
        // Back to HEAD.
        if !tracked.is_empty() {
            let mut args = vec!["checkout", "HEAD", "--"];
            args.extend(tracked.iter().map(|s| s.as_str()));
            self.git(&args)?;
        }
        for file in &new {
            let _ = std::fs::remove_file(self.root.join(file));
        }
        Ok(Shelf { id, name: name.to_string(), files: files.to_vec() })
    }

    /// Applies a shelf back to the working tree; it is removed unless `keep`.
    pub fn unshelve(&self, dir: &Path, id: &str, keep: bool) -> Result<(), String> {
        let shelf = dir.join(id);
        let patch = shelf.join("changes.patch");
        if patch.is_file() {
            self.git(&["apply", "--3way", &patch.to_string_lossy()])?;
        }
        let new_dir = shelf.join("new");
        let mut stack = vec![new_dir.clone()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(&new_dir) {
                    let target = self.root.join(rel);
                    std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
                    std::fs::copy(&path, &target).map_err(|e| e.to_string())?;
                }
            }
        }
        if !keep {
            std::fs::remove_dir_all(&shelf).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// Branch name, or empty when detached.
    pub branch: String,
    pub head: String,
}

/// `git worktree list --porcelain`.
pub fn parse_worktrees(text: &str) -> Vec<Worktree> {
    text.split("\n\n")
        .filter_map(|block| {
            let mut tree = Worktree { path: PathBuf::new(), branch: String::new(), head: String::new() };
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    tree.path = PathBuf::from(p);
                } else if let Some(h) = line.strip_prefix("HEAD ") {
                    tree.head = h.chars().take(8).collect();
                } else if let Some(b) = line.strip_prefix("branch ") {
                    tree.branch = b.trim_start_matches("refs/heads/").to_string();
                }
            }
            (!tree.path.as_os_str().is_empty()).then_some(tree)
        })
        .collect()
}

/// `git remote -v`: (name, fetch URL).
pub fn parse_remotes(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(url)) = (parts.next(), parts.next())
            && !out.iter().any(|(n, _)| n == name)
        {
            out.push((name.to_string(), url.to_string()));
        }
    }
    out
}

impl Repo {
    pub fn worktrees(&self) -> Vec<Worktree> {
        self.git(&["worktree", "list", "--porcelain"]).map(|t| parse_worktrees(&t)).unwrap_or_default()
    }

    pub fn remotes(&self) -> Vec<(String, String)> {
        self.git(&["remote", "-v"]).map(|t| parse_remotes(&t)).unwrap_or_default()
    }
}

/// A merge conflict block in a file: byte ranges of the whole block and of each side's text
/// (without the marker lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub whole: Range<usize>,
    pub ours: Range<usize>,
    /// `|||||||` common ancestor (diff3 style), if present.
    pub base: Option<Range<usize>>,
    pub theirs: Range<usize>,
    /// Labels after the markers (`HEAD`, the branch name).
    pub ours_label: String,
    pub theirs_label: String,
}

/// The `<<<<<<<` … `=======` … `>>>>>>>` blocks of `text`.
pub fn conflicts(text: &str) -> Vec<Conflict> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut open: Option<(usize, usize, String)> = None;
    let mut base: Option<(usize, usize)> = None;
    let mut middle: Option<(usize, usize)> = None;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let marker = |m: &str| line.starts_with(m) && line[m.len()..].chars().next().is_none_or(|c| c == ' ' || c == '\n' || c == '\r');
        if marker("<<<<<<<") {
            open = Some((start, offset, line[7..].trim().to_string()));
            base = None;
            middle = None;
        } else if open.is_some() && middle.is_none() && marker("|||||||") {
            base = Some((start, offset));
        } else if open.is_some() && middle.is_none() && marker("=======") {
            middle = Some((start, offset));
        } else if marker(">>>>>>>")
            && let (Some((o_start, o_end, ours_label)), Some((m_start, m_end))) = (open.take(), middle.take())
        {
            let ours_end = base.map_or(m_start, |(b, _)| b);
            out.push(Conflict {
                whole: o_start..offset,
                ours: o_end..ours_end,
                base: base.map(|(_, b_end)| b_end..m_start),
                theirs: m_end..start,
                ours_label,
                theirs_label: line[7..].trim().to_string(),
            });
            base = None;
        }
    }
    out
}

/// The text of a conflict resolved one way: `ours`, `theirs`, or both in either order.
pub fn resolve(text: &str, conflict: &Conflict, ours: bool, theirs: bool, theirs_first: bool) -> String {
    let (o, t) = (&text[conflict.ours.clone()], &text[conflict.theirs.clone()]);
    match (ours, theirs) {
        (true, true) if theirs_first => format!("{t}{o}"),
        (true, true) => format!("{o}{t}"),
        (true, false) => o.to_string(),
        (false, true) => t.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    }

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            short: hash.into(),
            author: String::new(),
            date: String::new(),
            subject: String::new(),
            refs: String::new(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn graph_lanes() {
        // m merges b into a:  m(a,b) -> a -> base ; b -> base
        let rows = graph(&[commit("m", &["a", "b"]), commit("a", &["base"]), commit("b", &["base"]), commit("base", &[])]);
        assert_eq!(rows[0].column, 0);
        assert_eq!(rows[0].bottom, [(0, 0), (0, 1)], "merge opens a second lane");
        assert_eq!(rows[1].column, 0);
        assert_eq!(rows[1].top, [(0, 0), (1, 1)]);
        assert_eq!(rows[2].column, 1, "b stays in its lane");
        assert_eq!(rows[2].bottom, [(1, 0)], "b's parent is already expected in lane 0");
        assert_eq!(rows[3].column, 0);
        assert_eq!(rows[3].top, [(0, 0)]);
        assert!(rows[3].bottom.is_empty());
    }

    #[test]
    fn diffs() {
        let changes = line_changes("a\nb\nc\nd\n", "a\nB\nc\nx\ny\n");
        assert_eq!(changes, [(1, LineChange::Modified), (3, LineChange::Modified), (4, LineChange::Modified)]);
        assert_eq!(line_changes("a\nb\n", "a\n"), [(1, LineChange::DeletedAbove)]);
        assert_eq!(line_changes("a\n", "a\nb\n"), [(1, LineChange::Added)]);
        let rows = side_by_side("a\nb\nc\n", "a\nB\nc\nd\n");
        let tags: Vec<RowTag> = rows.iter().map(|r| r.tag).collect();
        assert_eq!(tags, [RowTag::Equal, RowTag::Changed, RowTag::Equal, RowTag::Added]);
        let (l, r) = inline_changes("let a = 1;", "let b = 1;");
        assert_eq!((l, r), (vec![4..5], vec![4..5]));
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_793_534_400), "2026-11-01");
    }

    #[test]
    fn repository_workflow() {
        let dir = std::env::temp_dir().join(format!("phpcraft-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        run(&dir, &["config", "user.email", "test@example.com"]);
        run(&dir, &["config", "user.name", "Tester"]);
        std::fs::write(dir.join("a.php"), "<?php\necho 1;\n").unwrap();
        std::fs::write(dir.join("b.php"), "<?php\n").unwrap();

        let repo = Repo::discover(&dir).unwrap();
        let status = repo.status().unwrap();
        assert_eq!(status.iter().map(|s| (s.path.as_str(), s.change)).collect::<Vec<_>>(), [("a.php", Change::Untracked), ("b.php", Change::Untracked)]);

        let summary = repo.commit("first", &["a.php".into()], false).unwrap();
        assert!(summary.ends_with("first"), "{summary}");
        assert_eq!(repo.status().unwrap().len(), 1, "only b.php left");
        assert_eq!(repo.current_branch().as_deref(), Some("main"));

        std::fs::write(dir.join("a.php"), "<?php\necho 2;\n").unwrap();
        let status = repo.status().unwrap();
        let a = status.iter().find(|s| s.path == "a.php").unwrap();
        assert_eq!((a.change, a.staged, a.unstaged), (Change::Modified, false, true));
        assert_eq!(repo.show("HEAD", "a.php").as_deref(), Some("<?php\necho 1;\n"));
        repo.stage(&["a.php".into()]).unwrap();
        assert!(repo.status().unwrap().iter().any(|s| s.path == "a.php" && s.staged));
        repo.unstage(&["a.php".into()]).unwrap();
        assert!(repo.status().unwrap().iter().any(|s| s.path == "a.php" && !s.staged));

        repo.commit("second", &["a.php".into()], false).unwrap();
        repo.commit("second, amended", &[], true).unwrap();
        let log = repo.log(10).unwrap();
        assert_eq!(log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["second, amended", "first"]);
        assert_eq!(log[0].parents, [log[1].hash.clone()]);
        let filtered = repo.log_filtered(&LogFilter { text: "FIRST".into(), ..Default::default() }, 10).unwrap();
        assert_eq!(filtered.len(), 1);
        let by_hash = repo.log_filtered(&LogFilter { text: log[1].hash[..7].into(), ..Default::default() }, 10).unwrap();
        assert_eq!(by_hash[0].subject, "first");
        assert_eq!(repo.commit_details(&log[0].hash).unwrap().author, "Tester");
        assert!(log[0].refs.contains("main"));
        assert_eq!(repo.commit_files(&log[0].hash).unwrap(), [(Change::Modified, "a.php".to_string())]);

        let blame = repo.blame("a.php").unwrap();
        assert_eq!(blame.len(), 2);
        assert_eq!(blame[1].author, "Tester");
        assert_eq!(blame[1].summary, "second, amended");

        repo.create_branch("feature/x").unwrap();
        assert_eq!(repo.current_branch().as_deref(), Some("feature/x"));
        std::fs::write(dir.join("c.php"), "<?php\n").unwrap();
        repo.commit("on feature", &["c.php".into()], false).unwrap();
        repo.checkout("main").unwrap();
        assert_eq!(repo.branches().unwrap().0, ["feature/x", "main"]);
        repo.merge("feature/x").unwrap();
        assert!(dir.join("c.php").exists());
        repo.rename_branch("feature/x", "feature/y").unwrap();
        repo.delete_branch("feature/y", false).unwrap();
        repo.create_branch_from("hotfix", "main").unwrap();
        assert_eq!(repo.current_branch().as_deref(), Some("hotfix"));
        repo.checkout("main").unwrap();
        assert_eq!(repo.branches().unwrap().0, ["hotfix", "main"]);

        let b = repo.status().unwrap();
        repo.discard(&b).unwrap();
        assert!(repo.status().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_conflicts() {
        let text = "a\n<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> feature\nz\n";
        let found = conflicts(text);
        assert_eq!(found.len(), 1);
        let c = &found[0];
        assert_eq!((&text[c.ours.clone()], &text[c.theirs.clone()]), ("mine\n", "theirs\n"));
        assert_eq!(&text[c.base.clone().unwrap()], "old\n");
        assert_eq!((c.ours_label.as_str(), c.theirs_label.as_str()), ("HEAD", "feature"));
        assert_eq!(&text[c.whole.clone()], "<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> feature\n");
        assert_eq!(resolve(text, c, true, true, false), "mine\ntheirs\n");
        assert_eq!(resolve(text, c, false, true, false), "theirs\n");
    }

    #[test]
    #[cfg(unix)]
    fn interactive_rebase() {
        let dir = std::env::temp_dir().join(format!("phpcraft-rebase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        run(&dir, &["config", "user.email", "t@t"]);
        run(&dir, &["config", "user.name", "T"]);
        for (file, msg) in [("a", "base"), ("b", "add b"), ("c", "add c"), ("d", "fix c")] {
            std::fs::write(dir.join(file), file).unwrap();
            run(&dir, &["add", "."]);
            run(&dir, &["commit", "-q", "-m", msg]);
        }
        let repo = Repo::discover(&dir).unwrap();
        let base = repo.git(&["rev-parse", "HEAD~3"]).unwrap().trim().to_string();
        let commits = repo.commits_since(&base).unwrap();
        assert_eq!(commits.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(), ["add b", "add c", "fix c"]);
        let step = |i: usize, action, message: &str| RebaseStep { action, hash: commits[i].0.clone(), subject: commits[i].1.clone(), message: message.into() };
        // c first, fold "fix c" into it, reword b, applied last.
        let steps = vec![step(1, RebaseAction::Pick, ""), step(2, RebaseAction::Fixup, ""), step(0, RebaseAction::Reword, "add b (renamed)")];
        repo.rebase_interactive(&base, &steps).unwrap();
        let log = repo.git(&["log", "--format=%s"]).unwrap();
        assert_eq!(log.lines().collect::<Vec<_>>(), ["add b (renamed)", "add c", "base"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn shelve_and_unshelve() {
        let dir = std::env::temp_dir().join(format!("phpcraft-shelf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        run(&dir, &["config", "user.email", "t@t"]);
        run(&dir, &["config", "user.name", "T"]);
        std::fs::write(dir.join("a.php"), "one\n").unwrap();
        std::fs::write(dir.join("b.php"), "two\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("a.php"), "one changed\n").unwrap();
        std::fs::write(dir.join("b.php"), "two changed\n").unwrap();
        std::fs::write(dir.join("new.php"), "new\n").unwrap();
        let repo = Repo::discover(&dir).unwrap();
        let shelf_dir = dir.join(".phpcraft/shelf");
        let shelf = repo.shelve(&shelf_dir, "wip", &["a.php".into(), "new.php".into()]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.php")).unwrap(), "one\n");
        assert!(!dir.join("new.php").exists());
        assert_eq!(std::fs::read_to_string(dir.join("b.php")).unwrap(), "two changed\n", "not shelved: untouched");
        assert_eq!(shelves(&shelf_dir)[0].name, "wip");
        repo.unshelve(&shelf_dir, &shelf.id, false).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.php")).unwrap(), "one changed\n");
        assert_eq!(std::fs::read_to_string(dir.join("new.php")).unwrap(), "new\n");
        assert!(shelves(&shelf_dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_worktrees_and_remotes() {
        let wt = "worktree /src/shop\nHEAD 0123456789abcdef\nbranch refs/heads/main\n\nworktree /src/shop-fix\nHEAD fedcba9876543210\ndetached\n";
        let trees = parse_worktrees(wt);
        assert_eq!(trees.len(), 2);
        assert_eq!((trees[0].branch.as_str(), trees[0].head.as_str()), ("main", "01234567"));
        assert!(trees[1].branch.is_empty());
        let remotes = parse_remotes("origin\tgit@github.com:a/b.git (fetch)\norigin\tgit@github.com:a/b.git (push)\nup\thttps://x/y (fetch)\n");
        assert_eq!(remotes, [("origin".to_string(), "git@github.com:a/b.git".to_string()), ("up".to_string(), "https://x/y".to_string())]);
    }
}
