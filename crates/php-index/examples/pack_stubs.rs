//! Packs the PHP extensions PhpCraft knows about from a JetBrains phpstorm-stubs checkout
//! into `stubs.bin.gz` (run by script/update-stubs.sh):
//! `cargo run -p php-index --example pack_stubs -- <stubs dir> <out file> <version>`.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The extensions of a typical PHP / Laravel install (like PhpStorm's default PHP Runtime).
const EXTENSIONS: &[&str] = &[
    "Core", "standard", "SPL", "Reflection", "date", "json", "pcre", "random", "mbstring", "ctype", "iconv", "intl",
    "PDO", "pdo_mysql", "pdo_pgsql", "pdo_sqlite", "mysqli", "pgsql", "sqlite3", "redis", "memcached",
    "curl", "openssl", "hash", "sodium", "filter", "fileinfo", "session", "tokenizer", "zlib", "zip", "bcmath", "gmp",
    "dom", "libxml", "SimpleXML", "xml", "xmlreader", "xmlwriter", "xsl", "soap", "gd", "imagick", "exif",
    "posix", "pcntl", "sockets", "readline", "ftp", "calendar", "gettext", "Phar", "Zend OPcache", "apcu",
    "igbinary", "msgpack", "ds", "uuid", "yaml", "mongodb", "amqp", "xdebug", "pcov", "superglobals", "meta",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, source, out, version] = &args[..] else {
        eprintln!("usage: pack_stubs <stubs dir> <out file> <version>");
        std::process::exit(2);
    };
    let source = Path::new(source);
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for ext in EXTENSIONS {
        let dir = source.join(ext);
        if !dir.is_dir() {
            eprintln!("skipping missing extension {ext}");
            continue;
        }
        collect(&dir, source, &mut files);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut raw = Vec::new();
    raw.extend_from_slice(b"PCST");
    write_str(&mut raw, version.as_bytes());
    raw.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (path, content) in &files {
        write_str(&mut raw, path.as_bytes());
        write_str(&mut raw, content);
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&raw).unwrap();
    let packed = encoder.finish().unwrap();
    std::fs::write(out, &packed).unwrap();
    println!("{} files, {} bytes → {} bytes", files.len(), raw.len(), packed.len());
}

fn write_str(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.is_dir() {
            collect(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "php") {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, std::fs::read(&path).unwrap()));
        }
    }
}
