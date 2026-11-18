//! Module specifier resolution: the project's `tsconfig.json` / `jsconfig.json`
//! (`compilerOptions.baseUrl` and `paths`), a Vite config's `resolve.alias`, and the Laravel /
//! Inertia `@/` convention that Laravel projects use without any config at all.
//!
//! The files are read once per project and the rules keep the same order of precedence the
//! compiler uses: an alias first, then a relative path, then `baseUrl`.

use std::path::{Component, Path, PathBuf};

/// How a module specifier turns into files: the project root plus its alias rules.
#[derive(Debug, Clone)]
pub struct Resolve {
    root: PathBuf,
    /// `(prefix, directory)`, longest prefix first: `@/` -> `<root>/resources/js/`.
    aliases: Vec<(String, PathBuf)>,
    /// `compilerOptions.baseUrl`: bare specifiers live under it.
    base_url: Option<PathBuf>,
}

impl Resolve {
    /// Only the Laravel / Inertia convention: `@/` and `~/` point at `resources/js`.
    pub fn new(root: &Path) -> Self {
        let js = root.join("resources").join("js");
        Self { root: root.to_path_buf(), aliases: vec![("@/".into(), js.clone()), ("~/".into(), js)], base_url: None }
    }

    /// The project's own settings, on top of the convention above.
    pub fn load(root: &Path) -> Self {
        let mut out = Self::new(root);
        for name in ["tsconfig.json", "jsconfig.json"] {
            let Ok(text) = std::fs::read_to_string(root.join(name)) else { continue };
            out.read_tsconfig(&text);
            break;
        }
        if let Some(text) = ["vite.config.ts", "vite.config.js", "vite.config.mjs", "vite.config.mts"]
            .iter()
            .find_map(|name| std::fs::read_to_string(root.join(name)).ok())
        {
            out.read_vite(&text);
        }
        // Longest prefix first, so `@app/` wins over `@/`.
        out.aliases.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        out
    }

    /// Paths a module specifier may point at, in probe order.
    pub fn candidates(&self, from: &Path, source: &str) -> Vec<PathBuf> {
        let base = if let Some((prefix, dir)) = self.alias(source) {
            dir.join(source[prefix.len()..].trim_start_matches('/'))
        } else if source.starts_with('.') {
            from.parent().unwrap_or(Path::new("")).join(source)
        } else if let Some(base) = &self.base_url {
            base.join(source)
        } else {
            return Vec::new();
        };
        let base = normalize(&base);
        const EXTENSIONS: [&str; 8] = ["ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte"];
        let mut out = Vec::new();
        if base.extension().is_some() {
            out.push(base.clone());
        }
        for extension in EXTENSIONS {
            out.push(base.with_extension(extension));
        }
        for extension in EXTENSIONS {
            out.push(base.join(format!("index.{extension}")));
        }
        out
    }

    /// The file a module specifier points at, when it exists in `load`.
    pub fn resolve_import(
        &self,
        from: &Path,
        source: &str,
        load: &dyn Fn(&Path) -> Option<String>,
    ) -> Option<PathBuf> {
        self.candidates(from, source).into_iter().find(|candidate| load(candidate).is_some())
    }

    /// The alias rule `source` falls under, plus its directory.
    fn alias(&self, source: &str) -> Option<(&str, &PathBuf)> {
        self.aliases
            .iter()
            .find(|(prefix, _)| {
                // `@app/*` keeps its slash and is a plain prefix; `ziggy` is exact.
                if prefix.ends_with('/') {
                    source.starts_with(prefix.as_str())
                } else {
                    source == prefix.as_str() || source.strip_prefix(prefix.as_str()).is_some_and(|rest| rest.starts_with('/'))
                }
            })
            .map(|(prefix, dir)| (prefix.as_str(), dir))
    }

    /// The specifier that points at `target` from `from`, written the way `like` is written:
    /// an alias stays an alias, a relative path stays relative, and an extension only appears
    /// when the original had one.
    pub fn specifier(&self, from: &Path, target: &Path, like: &str) -> Option<String> {
        let mut specifier = if let Some((prefix, dir)) = self.alias(like) {
            let rest = slashed(target.strip_prefix(dir).ok()?);
            match (prefix.trim_end_matches('/'), rest.as_str()) {
                (prefix, "") => prefix.to_string(),
                (prefix, rest) => format!("{prefix}/{rest}"),
            }
        } else if like.starts_with('.') {
            relative(from.parent()?, target)?
        } else {
            slashed(target.strip_prefix(self.base_url.as_ref()?.as_path()).ok()?)
        };
        if !like_had_extension(like) {
            specifier = strip_module_extension(&specifier);
        }
        Some(specifier)
    }

    /// `compilerOptions.baseUrl` and `paths` of a tsconfig (JSONC: comments and trailing commas
    /// are simply skipped).
    fn read_tsconfig(&mut self, text: &str) {
        let Some(options) = object_after_key(text, "compilerOptions") else { return };
        if let Some(value) = string_after_key(options, "baseUrl") {
            self.base_url = Some(normalize(&self.root.join(value)));
        }
        let Some(paths) = object_after_key(options, "paths") else { return };
        for (key, values) in pairs(paths) {
            let Some(target) = values.first() else { continue };
            let prefix = key.strip_suffix('*').unwrap_or(&key).to_string();
            let target = target.strip_suffix('*').unwrap_or(target);
            if prefix.is_empty() || target.is_empty() {
                continue;
            }
            let base = self.base_url.clone().unwrap_or_else(|| self.root.clone());
            let dir = normalize(&base.join(target));
            self.aliases.push((prefix, dir));
        }
    }

    /// `resolve: { alias: { '@': path.resolve(__dirname, './resources/js') } }`, including the
    /// array form `{ find: '@', replacement: './resources/js' }`.
    fn read_vite(&mut self, text: &str) {
        for (key, value) in vite_aliases(text) {
            let Some(dir) = value else { continue };
            let dir = if dir.starts_with('/') || dir.contains(':') {
                PathBuf::from(dir)
            } else {
                self.root.join(dir.trim_start_matches("./"))
            };
            self.aliases.push((key.trim_end_matches('*').to_string(), normalize(&dir)));
        }
    }
}

/// Removes `.` segments and resolves `..` without touching the file system.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `to` written relative to the directory `base`.
fn relative(base: &Path, to: &Path) -> Option<String> {
    let base = normalize(base);
    let to = normalize(to);
    let from: Vec<Component> = base.components().collect();
    let target: Vec<Component> = to.components().collect();
    let common = from.iter().zip(&target).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return None;
    }
    let mut out = String::new();
    for _ in common..from.len() {
        out.push_str("../");
    }
    let rest: Vec<String> = target[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    out.push_str(&rest.join("/"));
    if out.is_empty() {
        return Some(".".to_string());
    }
    let mut out = if out.starts_with('.') { out } else { format!("./{out}") };
    if out.ends_with('/') {
        out.pop();
    }
    Some(out)
}

fn slashed(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn like_had_extension(like: &str) -> bool {
    like.rsplit('/').next().is_some_and(|name| name.contains('.'))
}

/// Drops `ts` / `js` and the trailing `/index` that module resolution adds implicitly. Component
/// extensions stay: `.vue` must be written out in an import.
fn strip_module_extension(specifier: &str) -> String {
    let mut out = specifier.to_string();
    for extension in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        let suffix = format!(".{extension}");
        if let Some(stripped) = out.strip_suffix(&suffix) {
            out = stripped.to_string();
            if out.ends_with("/index") {
                out.truncate(out.len() - "/index".len());
                if out.is_empty() {
                    out.push('.');
                }
            }
            break;
        }
    }
    out
}

/// The text inside the object that follows `"key":`.
fn object_after_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let at = text.find(&needle)?;
    let rest = &text[at + needle.len()..];
    let colon = rest.find(':')?;
    object_after(&rest[colon + 1..])
}

fn string_after_key(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = text.find(&needle)?;
    let rest = &text[at + needle.len()..];
    let colon = rest.find(':')?;
    let (value, _) = take_string(skip_ws(&rest[colon + 1..]))?;
    Some(value)
}

fn object_after(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = matching(&text[start..], '{', '}')?;
    Some(&text[start + 1..start + end])
}

fn array_after(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = matching(&text[start..], '[', ']')?;
    Some(&text[start + 1..start + end])
}

/// Byte offset of the closing bracket matching the one at offset 0, ignoring quoted strings.
fn matching(text: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (offset, ch) in text.char_indices() {
        if let Some(active) = quote {
            if ch == active && !text[..offset].ends_with('\\') {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// `(key, values)` pairs of a JSON object.
fn pairs(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        rest = skip_ws(rest.trim_start_matches(','));
        let Some((key, after)) = take_string(rest) else { break };
        rest = skip_ws(after);
        let Some(after) = rest.strip_prefix(':') else { break };
        rest = skip_ws(after);
        let values = if rest.starts_with('[') {
            let Some(array) = array_after(rest) else { break };
            let end = matching(rest, '[', ']').unwrap_or(rest.len());
            rest = &rest[end + 1..];
            strings_in(array)
        } else if let Some((value, after)) = take_string(rest) {
            rest = after;
            vec![value]
        } else {
            // A call such as `path.resolve(__dirname, './resources/js')`: keep its strings.
            let end = top_level_comma(rest);
            let raw = &rest[..end];
            rest = &rest[end..];
            quoted_strings(raw)
        };
        out.push((key, values));
    }
    out
}

/// Offset of the first `,` that is not inside brackets, braces, parentheses or a string.
fn top_level_comma(text: &str) -> usize {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (offset, ch) in text.char_indices() {
        if let Some(active) = quote {
            if ch == active && !text[..offset].ends_with('\\') {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' | '`' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    return offset;
                }
                depth -= 1;
            }
            ',' if depth == 0 => return offset,
            _ => {}
        }
    }
    text.len()
}

/// Every quoted string of a value expression.
fn quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(['"', '\'']) {
        let Some((value, after)) = take_string(&rest[at..]) else { break };
        out.push(value);
        rest = after;
    }
    out
}

/// The quoted strings of an array.
fn strings_in(array: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = array;
    loop {
        rest = skip_ws(rest.trim_start_matches(','));
        let Some((value, after)) = take_string(rest) else { break };
        out.push(value);
        rest = after;
    }
    out
}

/// A leading JSON string, unescaped.
fn take_string(text: &str) -> Option<(String, &str)> {
    let mut chars = text.char_indices();
    let (_, quote) = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for (offset, ch) in chars {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            c if c == quote => return Some((out, &text[offset + c.len_utf8()..])),
            c => out.push(c),
        }
    }
    None
}

fn skip_ws(text: &str) -> &str {
    text.trim_start_matches([' ', '\t', '\r', '\n'])
}

/// `alias` entries of a Vite config, in both the object and the array form.
fn vite_aliases(text: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("alias") {
        rest = &rest[at + "alias".len()..];
        let Some(block) = object_after(rest).or_else(|| array_after(rest)) else { continue };
        if block.contains("find") {
            // { find: '@', replacement: './resources/js' }
            let find = string_after_key(block, "find");
            let replacement = string_after_key(block, "replacement");
            if let Some(key) = find {
                out.push((key.to_string(), replacement));
            }
            continue;
        }
        for (key, values) in pairs(block) {
            let value = values.into_iter().rev().find(|value| value.starts_with('.') || value.contains('/'));
            out.push((key, value));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tsconfig_paths_and_base_url() {
        let mut resolve = Resolve::new(Path::new("/p"));
        resolve.read_tsconfig(
            r#"{
              // comments and trailing commas must not matter
              "compilerOptions": {
                "baseUrl": "resources/js",
                "paths": {
                  "@app/*": ["./app/*"],
                  "ziggy": ["./vendor/ziggy/src/js/index.js"],
                },
              },
            }"#,
        );
        resolve.aliases.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        assert_eq!(resolve.base_url.as_deref(), Some(Path::new("/p/resources/js")));
        let candidates = resolve.candidates(Path::new("/p/resources/js/Pages/Home.vue"), "@app/lib/x");
        assert_eq!(candidates[0], Path::new("/p/resources/js/app/lib/x.ts"));
        let candidates = resolve.candidates(Path::new("/p/resources/js/Pages/Home.vue"), "@/lib/x");
        assert_eq!(candidates[0], Path::new("/p/resources/js/lib/x.ts"));
        let candidates = resolve.candidates(Path::new("/p/resources/js/Pages/Home.vue"), "ziggy");
        assert_eq!(candidates[0], Path::new("/p/resources/js/vendor/ziggy/src/js/index.js"));
        let candidates = resolve.candidates(Path::new("/p/resources/js/Pages/Home.vue"), "composables/useUser");
        assert_eq!(candidates[0], Path::new("/p/resources/js/composables/useUser.ts"));
    }

    #[test]
    fn reads_vite_aliases() {
        let text = r#"
            export default defineConfig({
                plugins: [laravel({ input: ['resources/js/app.ts'] })],
                resolve: {
                    alias: {
                        '@': path.resolve(__dirname, './resources/js'),
                        '~assets': '/abs/assets',
                    },
                },
            })
        "#;
        let aliases = vite_aliases(text);
        assert!(aliases.iter().any(|(key, value)| key == "@" && value.as_deref() == Some("./resources/js")), "{aliases:?}");
        assert!(aliases.iter().any(|(key, value)| key == "~assets" && value.as_deref() == Some("/abs/assets")), "{aliases:?}");

        let mut resolve = Resolve::new(Path::new("/p"));
        resolve.read_vite(text);
        let candidates = resolve.candidates(Path::new("/p/resources/js/Pages/Home.vue"), "@/lib/format");
        assert_eq!(candidates[0], Path::new("/p/resources/js/lib/format.ts"));
        let candidates = resolve.candidates(Path::new("/p/resources/js/Pages/Home.vue"), "~assets/logo.png");
        assert_eq!(candidates[0], Path::new("/abs/assets/logo.png"));
    }

    #[test]
    fn writes_specifiers_in_the_same_style() {
        let resolve = Resolve::new(Path::new("/p"));
        let from = Path::new("/p/resources/js/Pages/Home.vue");
        assert_eq!(resolve.specifier(from, Path::new("/p/resources/js/lib/format.ts"), "./x").as_deref(), Some("../lib/format"));
        assert_eq!(resolve.specifier(from, Path::new("/p/resources/js/lib/format.ts"), "@/x").as_deref(), Some("@/lib/format"));
        assert_eq!(resolve.specifier(from, Path::new("/p/resources/js/Card.vue"), "../y").as_deref(), Some("../Card.vue"));
        assert_eq!(resolve.specifier(from, Path::new("/p/resources/js/lib/index.ts"), "./x").as_deref(), Some("../lib"));
    }
}
