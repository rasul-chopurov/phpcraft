//! Data for the Laravel tool window (like Laravel Idea's): environment facts, project
//! issues with their artisan fixes, and errors grouped from `storage/logs/laravel.log`.

use std::path::{Path, PathBuf};

/// `.env` values (no variable expansion).
pub fn env_values(root: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(root.join(".env"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_string(), value.trim().trim_matches('"').trim_matches('\'').to_string()))
        })
        .collect()
}

pub fn env_value(values: &[(String, String)], key: &str) -> Option<String> {
    values.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub title: &'static str,
    pub description: &'static str,
    /// `php artisan …` arguments that fix it.
    pub fix: &'static [&'static str],
}

/// Problems Laravel Idea's dashboard reports: missing .env/APP_KEY, cached config/routes.
pub fn issues(root: &Path) -> Vec<Issue> {
    let mut out = Vec::new();
    let env = env_values(root);
    if !root.join(".env").exists() {
        out.push(Issue { title: ".env is missing", description: "Copy .env.example to .env, then generate a key.", fix: &[] });
    } else if env_value(&env, "APP_KEY").is_none_or(|k| k.is_empty()) {
        out.push(Issue { title: "APP_KEY is not set", description: "Encryption and sessions need an application key.", fix: &["key:generate"] });
    }
    if root.join("bootstrap/cache/config.php").exists() {
        out.push(Issue { title: "Configuration is cached", description: "Changes to config/ and .env are ignored until the cache is cleared.", fix: &["config:clear"] });
    }
    if root.join("bootstrap/cache/routes-v7.php").exists() || root.join("bootstrap/cache/routes.php").exists() {
        out.push(Issue { title: "Routes are cached", description: "New or changed routes are ignored until the cache is cleared.", fix: &["route:clear"] });
    }
    if root.join("bootstrap/cache/events.php").exists() {
        out.push(Issue { title: "Events are cached", description: "New listeners are ignored until the cache is cleared.", fix: &["event:clear"] });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub time: String,
    pub level: String,
    pub message: String,
    /// Stack trace and context lines after the message.
    pub details: Vec<String>,
}

/// Entries of a Laravel log: `[2024-05-01 10:00:00] local.ERROR: message {context}` plus
/// the following lines.
pub fn parse_log(text: &str) -> Vec<LogEntry> {
    let mut out: Vec<LogEntry> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('[')
            && let Some((time, rest)) = rest.split_once("] ")
            && let Some((channel, message)) = rest.split_once(": ")
            && let Some((_, level)) = channel.rsplit_once('.')
            && time.len() >= 19
        {
            out.push(LogEntry { time: time.to_string(), level: level.to_string(), message: message.to_string(), details: Vec::new() });
        } else if let Some(entry) = out.last_mut() {
            if entry.details.len() < 400 {
                entry.details.push(line.to_string());
            }
        }
    }
    out
}

/// The message with numbers, quoted values and ids blanked, so repeats group together.
pub fn signature(message: &str) -> String {
    let message = message.split(" {").next().unwrap_or(message);
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut prev_digit = false;
    for c in message.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
                out.push(q);
            }
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            out.push(c);
            out.push('…');
            continue;
        }
        if c.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
            continue;
        }
        prev_digit = false;
        out.push(c);
    }
    out.trim().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogGroup {
    pub signature: String,
    pub level: String,
    pub count: usize,
    pub first: String,
    pub last: String,
    /// The latest occurrence (shown expanded).
    pub latest: LogEntry,
}

/// Groups entries by [`signature`], most recent first.
pub fn group(entries: Vec<LogEntry>) -> Vec<LogGroup> {
    let mut groups: Vec<LogGroup> = Vec::new();
    for entry in entries {
        let sig = signature(&entry.message);
        match groups.iter_mut().find(|g| g.signature == sig) {
            Some(group) => {
                group.count += 1;
                group.last = entry.time.clone();
                group.latest = entry;
            }
            None => groups.push(LogGroup { signature: sig, level: entry.level.clone(), count: 1, first: entry.time.clone(), last: entry.time.clone(), latest: entry }),
        }
    }
    groups.sort_by(|a, b| b.last.cmp(&a.last));
    groups
}

/// Reads the newest part of `storage/logs/laravel.log` (and daily logs) and groups it.
pub fn log_groups(root: &Path) -> Vec<LogGroup> {
    let dir = root.join("storage/logs");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "log")).collect();
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    let Some(latest) = files.last() else { return Vec::new() };
    let bytes = std::fs::read(latest).unwrap_or_default();
    // The last 2 MB is plenty for recent errors.
    let start = bytes.len().saturating_sub(2 * 1024 * 1024);
    let text = String::from_utf8_lossy(&bytes[start..]);
    group(parse_log(&text))
}

/// `#3 /var/www/html/app/Http/Kernel.php(42): …` -> (`app/Http/Kernel.php` under `root`, 42).
/// Paths from containers are mapped onto the project by their longest existing suffix.
pub fn trace_location(root: &Path, line: &str) -> Option<(PathBuf, usize)> {
    let start = line.find('/').or_else(|| line.find(":\\").map(|i| i - 1))?;
    let rest = &line[start..];
    let open = rest.find(".php(")? + 4;
    let close = rest[open..].find(')')? + open;
    let path = &rest[..open];
    let number: usize = rest[open + 1..close].parse().ok()?;
    let path = Path::new(path);
    if path.exists() {
        return Some((path.to_path_buf(), number));
    }
    let parts: Vec<_> = path.components().collect();
    (0..parts.len()).map(|i| parts[i..].iter().collect::<PathBuf>()).map(|rel| root.join(rel)).find(|p| p.exists()).map(|p| (p, number))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "[2026-10-31 10:00:00] local.ERROR: Undefined variable $user {\"exception\":\"[object]\"}\n[stacktrace]\n#0 /var/www/html/app/Http/Controllers/HomeController.php(12): handle()\n#1 {main}\n[2026-10-31 11:00:00] local.INFO: Order 42 shipped\n[2026-11-01 09:00:00] local.ERROR: Undefined variable $user {\"exception\":\"[object]\"}\n";

    #[test]
    fn groups_log_entries() {
        let entries = parse_log(LOG);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].level, "ERROR");
        assert_eq!(entries[0].details.len(), 3);
        let groups = group(entries);
        assert_eq!(groups[0].count, 2);
        assert_eq!(groups[0].last, "2026-11-01 09:00:00");
        assert_eq!(signature("Order 42 shipped to 'Ann'"), "Order # shipped to '…'");
    }

    #[test]
    fn issues_and_traces() {
        let root = std::env::temp_dir().join(format!("phpcraft-dash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bootstrap/cache")).unwrap();
        std::fs::create_dir_all(root.join("app/Http/Controllers")).unwrap();
        std::fs::write(root.join(".env"), "APP_ENV=local\nAPP_KEY=\nAPP_URL=\"http://app.test\"\n").unwrap();
        std::fs::write(root.join("bootstrap/cache/config.php"), "<?php").unwrap();
        std::fs::write(root.join("app/Http/Controllers/HomeController.php"), "<?php").unwrap();
        let titles: Vec<&str> = issues(&root).iter().map(|i| i.title).collect();
        assert_eq!(titles, ["APP_KEY is not set", "Configuration is cached"]);
        assert_eq!(env_value(&env_values(&root), "APP_URL").as_deref(), Some("http://app.test"));
        let (path, line) = trace_location(&root, "#0 /var/www/html/app/Http/Controllers/HomeController.php(12): handle()").unwrap();
        assert_eq!((path, line), (root.join("app/Http/Controllers/HomeController.php"), 12));
        std::fs::remove_dir_all(root).unwrap();
    }
}
