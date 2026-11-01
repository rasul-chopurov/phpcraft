//! Docker through its CLI: containers, images and volumes, and the commands the Services
//! tool window runs on them.

use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// `running`, `exited`…
    pub state: String,
    pub status: String,
    pub ports: String,
    /// `com.docker.compose.project`, if started by Compose.
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub repository: String,
    pub tag: String,
    pub id: String,
    pub size: String,
}

fn field(v: &serde_json::Value, name: &str) -> String {
    v.get(name).and_then(|f| f.as_str()).unwrap_or("").to_string()
}

fn docker(args: &[&str]) -> Result<String, String> {
    let output = process::command("docker").args(args).output().map_err(|e| format!("docker: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if err.contains("daemon") || err.contains("docker.sock") { "Docker isn't running. Start Docker Desktop (or the daemon) and refresh.".into() } else { err })
    }
}

fn json_lines(text: &str) -> Vec<serde_json::Value> {
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// `docker ps -a`, parsed from its JSON lines.
pub fn parse_containers(text: &str) -> Vec<Container> {
    json_lines(text)
        .into_iter()
        .map(|v| {
            let labels = field(&v, "Labels");
            Container {
                id: field(&v, "ID"),
                name: field(&v, "Names"),
                image: field(&v, "Image"),
                state: field(&v, "State"),
                status: field(&v, "Status"),
                ports: field(&v, "Ports"),
                project: labels.split(',').find_map(|l| l.strip_prefix("com.docker.compose.project=")).map(str::to_string),
            }
        })
        .collect()
}

pub fn containers() -> Result<Vec<Container>, String> {
    docker(&["ps", "-a", "--format", "{{json .}}"]).map(|t| parse_containers(&t))
}

pub fn images() -> Result<Vec<Image>, String> {
    docker(&["images", "--format", "{{json .}}"]).map(|t| {
        json_lines(&t)
            .into_iter()
            .map(|v| Image { repository: field(&v, "Repository"), tag: field(&v, "Tag"), id: field(&v, "ID"), size: field(&v, "Size") })
            .collect()
    })
}

pub fn volumes() -> Result<Vec<String>, String> {
    docker(&["volume", "ls", "--format", "{{json .}}"]).map(|t| json_lines(&t).into_iter().map(|v| field(&v, "Name")).collect())
}

/// `docker start|stop|restart <id>`.
pub fn container_action(action: &str, id: &str) -> Result<(), String> {
    docker(&[action, id]).map(|_| ())
}

/// The Compose project name Docker gives a folder (lowercase, letters, digits, `-` and `_`).
pub fn compose_project(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect())
        .unwrap_or_default()
}

pub fn has_compose_file(root: &Path) -> bool {
    ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"].iter().any(|f| root.join(f).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_ps() {
        let text = r#"{"Command":"\"docker-php…\"","ID":"a1b2","Image":"sail-8.3/app","Labels":"com.docker.compose.service=laravel.test,com.docker.compose.project=shop","Names":"shop-laravel.test-1","Ports":"0.0.0.0:80->80/tcp","State":"running","Status":"Up 2 hours"}
{"ID":"c3d4","Image":"redis","Labels":"","Names":"cache","State":"exited","Status":"Exited (0) 1 day ago","Ports":""}"#;
        let list = parse_containers(text);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].project.as_deref(), Some("shop"));
        assert_eq!(list[1].state, "exited");
        assert_eq!(compose_project(Path::new("/Users/me/My Shop")), "myshop");
    }
}
