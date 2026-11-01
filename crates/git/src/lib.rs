//! Git integration through the `git` command line (identical behaviour on macOS, Windows
//! and Linux): status, diffs, staging, commits, branches, log, blame, push/pull.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    pub fn discover(path: &Path) -> Option<Self> {
        let output = Command::new("git").args(["rev-parse", "--show-toplevel"]).current_dir(path).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(Self { root: PathBuf::from(root) })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
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
        let output = Command::new("git")
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
        let limit = format!("-{limit}");
        let output = self.git(&["log", &limit, "--date=short", "--format=%H%x1f%h%x1f%an%x1f%ad%x1f%s%x1f%D%x1e"])?;
        Ok(output
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
                })
            })
            .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
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
}
