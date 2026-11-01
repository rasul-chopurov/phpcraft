//! Registered routes via `php artisan route:list --json` (for Route Search).

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteInfo {
    /// `GET|HEAD`, `POST`, ...
    pub method: String,
    pub uri: String,
    pub name: Option<String>,
    /// `App\Http\Controllers\UserController@show`, an invokable class, or `Closure`.
    pub action: String,
    /// Where a closure route is defined: (relative file, 1-based line).
    pub defined_at: Option<(String, usize)>,
}

impl RouteInfo {
    /// Controller class and method the route dispatches to, if it is not a closure.
    pub fn controller(&self) -> Option<(&str, &str)> {
        if self.action == "Closure" {
            return None;
        }
        Some(self.action.split_once('@').unwrap_or((&self.action, "__invoke")))
    }

    /// Text matched by the search box.
    pub fn haystack(&self) -> String {
        format!("{} /{} {} {}", self.method, self.uri.trim_start_matches('/'), self.name.as_deref().unwrap_or(""), self.action)
            .to_ascii_lowercase()
    }
}

/// Blocking; call off the UI thread.
pub fn list_routes(root: &Path) -> Result<Vec<RouteInfo>, String> {
    let output = process::command(crate::php_binary())
        .args(["artisan", "route:list", "--json"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run php: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    parse(&String::from_utf8_lossy(&output.stdout))
}

fn parse(json: &str) -> Result<Vec<RouteInfo>, String> {
    let routes: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|e| format!("unexpected route:list output: {e}"))?;
    Ok(routes
        .iter()
        .map(|r| {
            let text = |key: &str| r[key].as_str().unwrap_or_default().to_string();
            let defined_at = r["path"].as_str().and_then(|p| {
                let (file, line) = p.rsplit_once(':')?;
                Some((file.to_string(), line.parse().ok()?))
            });
            RouteInfo {
                method: text("method"),
                uri: text("uri"),
                name: r["name"].as_str().map(str::to_string),
                action: text("action"),
                defined_at,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_route_list() {
        let json = r#"[
            {"domain":null,"method":"GET|HEAD","uri":"/","name":null,"action":"Closure","middleware":["web"],"path":"routes/web.php:5"},
            {"domain":null,"method":"POST","uri":"users","name":"users.store","action":"App\\Http\\Controllers\\UserController@store","middleware":[]},
            {"domain":null,"method":"GET|HEAD","uri":"admin","name":"dash","action":"Filament\\Pages\\Dashboard","middleware":[]}
        ]"#;
        let routes = parse(json).unwrap();
        assert_eq!(routes[0].defined_at, Some(("routes/web.php".into(), 5)));
        assert_eq!(routes[0].controller(), None);
        assert_eq!(routes[1].controller(), Some(("App\\Http\\Controllers\\UserController", "store")));
        assert_eq!(routes[2].controller(), Some(("Filament\\Pages\\Dashboard", "__invoke")));
        assert!(routes[1].haystack().contains("post /users users.store"));
    }
}
