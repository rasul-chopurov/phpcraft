//! On-disk index cache: the classes and functions of every scanned file, keyed by the file's
//! modification time and size, so a restart only re-parses files that changed.
//!
//! A small hand-written binary format: strings are length-prefixed UTF-8, numbers are
//! little-endian. Bump `FORMAT` whenever `extract` or the structures below change.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::extract::{ClassInfo, ClassKind, FunctionInfo, Member, MemberKind};

const MAGIC: &[u8; 4] = b"PCIX";
const FORMAT: u32 = 1;

/// A file's modification time (ns since the epoch) and size.
pub type Stamp = (u64, u64);

pub struct Entry {
    pub stamp: Stamp,
    pub classes: Vec<ClassInfo>,
    pub functions: Vec<FunctionInfo>,
}

pub fn stamp(metadata: &std::fs::Metadata) -> Stamp {
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    (mtime, metadata.len())
}

fn identity() -> String {
    format!("{FORMAT}/{}", env!("CARGO_PKG_VERSION"))
}

// ---- writing ----

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn opt_str(&mut self, s: Option<&str>) {
        match s {
            Some(s) => {
                self.u8(1);
                self.str(s);
            }
            None => self.u8(0),
        }
    }
    fn range(&mut self, r: &Range<usize>) {
        self.u32(r.start as u32);
        self.u32(r.end as u32);
    }
    fn strings(&mut self, list: &[String]) {
        self.u32(list.len() as u32);
        for s in list {
            self.str(s);
        }
    }
}

fn class_kind(kind: ClassKind) -> u8 {
    match kind {
        ClassKind::Class => 0,
        ClassKind::Interface => 1,
        ClassKind::Trait => 2,
        ClassKind::Enum => 3,
    }
}

fn member_kind(kind: MemberKind) -> u8 {
    match kind {
        MemberKind::Method => 0,
        MemberKind::Property => 1,
        MemberKind::Constant => 2,
        MemberKind::Case => 3,
    }
}

/// Serializes `entries`; the class and function paths are implied by each entry's file.
pub fn encode(entries: &[(PathBuf, Entry)]) -> Vec<u8> {
    let mut w = Writer(Vec::with_capacity(1 << 20));
    w.0.extend_from_slice(MAGIC);
    w.str(&identity());
    w.u32(entries.len() as u32);
    for (path, entry) in entries {
        w.str(&path.to_string_lossy());
        w.u64(entry.stamp.0);
        w.u64(entry.stamp.1);
        w.u32(entry.classes.len() as u32);
        for class in &entry.classes {
            w.str(&class.fqn);
            w.str(&class.name);
            w.u8(class_kind(class.kind));
            w.range(&class.range);
            w.strings(&class.extends);
            w.strings(&class.implements);
            w.strings(&class.traits);
            w.strings(&class.mixins);
            w.strings(&class.see);
            w.u32(class.members.len() as u32);
            for m in &class.members {
                w.str(&m.name);
                w.u8(member_kind(m.kind));
                w.u8(u8::from(m.is_static) | (u8::from(m.virtual_member) << 1));
                w.range(&m.range);
                w.opt_str(m.type_fqn.as_deref());
                w.str(&m.detail);
                w.opt_str(m.source.as_ref().map(|p| p.to_string_lossy()).as_deref());
            }
        }
        w.u32(entry.functions.len() as u32);
        for f in &entry.functions {
            w.str(&f.fqn);
            w.range(&f.range);
            w.opt_str(f.type_fqn.as_deref());
            w.str(&f.detail);
        }
    }
    w.0
}

// ---- reading ----

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn str(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }
    fn opt_str(&mut self) -> Option<Option<String>> {
        match self.u8()? {
            0 => Some(None),
            _ => Some(Some(self.str()?)),
        }
    }
    fn range(&mut self) -> Option<Range<usize>> {
        Some(self.u32()? as usize..self.u32()? as usize)
    }
    fn strings(&mut self) -> Option<Vec<String>> {
        let n = self.u32()? as usize;
        (0..n).map(|_| self.str()).collect()
    }
}

fn read_entries(bytes: &[u8]) -> Option<HashMap<PathBuf, Entry>> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != MAGIC || r.str()? != identity() {
        return None;
    }
    let count = r.u32()? as usize;
    // `count` comes from the file: cap it so a damaged file can't request a huge allocation.
    let mut out = HashMap::with_capacity(count.min(bytes.len() / 16));
    for _ in 0..count {
        let path = PathBuf::from(r.str()?);
        let shared: Arc<Path> = Arc::from(path.as_path());
        let stamp = (r.u64()?, r.u64()?);
        let classes = (0..r.u32()?)
            .map(|_| {
                let fqn = r.str()?;
                let name = r.str()?;
                let kind = match r.u8()? {
                    0 => ClassKind::Class,
                    1 => ClassKind::Interface,
                    2 => ClassKind::Trait,
                    _ => ClassKind::Enum,
                };
                let range = r.range()?;
                let (extends, implements, traits, mixins, see) = (r.strings()?, r.strings()?, r.strings()?, r.strings()?, r.strings()?);
                let members = (0..r.u32()?)
                    .map(|_| {
                        let name = r.str()?;
                        let kind = match r.u8()? {
                            0 => MemberKind::Method,
                            1 => MemberKind::Property,
                            2 => MemberKind::Constant,
                            _ => MemberKind::Case,
                        };
                        let flags = r.u8()?;
                        Some(Member {
                            name,
                            kind,
                            is_static: flags & 1 != 0,
                            virtual_member: flags & 2 != 0,
                            range: r.range()?,
                            type_fqn: r.opt_str()?,
                            detail: r.str()?,
                            source: r.opt_str()?.map(|p| Arc::from(Path::new(&p))),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(ClassInfo { fqn, name, kind, path: shared.clone(), range, extends, implements, traits, mixins, see, members })
            })
            .collect::<Option<Vec<_>>>()?;
        let functions = (0..r.u32()?)
            .map(|_| {
                Some(FunctionInfo { fqn: r.str()?, path: shared.clone(), range: r.range()?, type_fqn: r.opt_str()?, detail: r.str()? })
            })
            .collect::<Option<Vec<_>>>()?;
        out.insert(path, Entry { stamp, classes, functions });
    }
    Some(out)
}

/// Loads a cache; a missing, stale-format or damaged file gives an empty cache.
pub fn load(file: &Path) -> HashMap<PathBuf, Entry> {
    std::fs::read(file).ok().and_then(|bytes| read_entries(&bytes)).unwrap_or_default()
}

/// Writes encoded cache bytes atomically (temp file, then rename).
pub fn write(file: &Path, bytes: &[u8]) {
    let Some(dir) = file.parent() else { return };
    let _ = std::fs::create_dir_all(dir);
    // A unique temp name: two scans may write the cache at the same time.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = file.with_extension(format!("{}-{n}.tmp", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let path = PathBuf::from("/p/app/User.php");
        let shared: Arc<Path> = Arc::from(path.as_path());
        let class = ClassInfo {
            fqn: "App\\User".into(),
            name: "User".into(),
            kind: ClassKind::Class,
            path: shared.clone(),
            range: 10..14,
            extends: vec!["Model".into()],
            implements: vec![],
            traits: vec!["HasFactory".into()],
            mixins: vec![],
            see: vec![],
            members: vec![Member {
                name: "posts".into(),
                kind: MemberKind::Method,
                is_static: false,
                range: 40..45,
                type_fqn: Some("HasMany".into()),
                detail: "(): HasMany".into(),
                virtual_member: true,
                source: Some(Arc::from(Path::new("/p/database/migrations/x.php"))),
            }],
        };
        let function = FunctionInfo { fqn: "helper".into(), path: shared, range: 1..2, type_fqn: None, detail: "()".into() };
        let entries = vec![(path.clone(), Entry { stamp: (7, 99), classes: vec![class], functions: vec![function] })];
        let decoded = read_entries(&encode(&entries)).unwrap();
        let entry = &decoded[&path];
        assert_eq!(entry.stamp, (7, 99));
        assert_eq!(entry.classes[0].fqn, "App\\User");
        assert_eq!(&*entry.classes[0].path, path.as_path());
        let member = &entry.classes[0].members[0];
        assert!(member.virtual_member && !member.is_static);
        assert_eq!(member.source.as_deref(), Some(Path::new("/p/database/migrations/x.php")));
        assert_eq!(entry.functions[0].detail, "()");
        assert!(read_entries(b"junk").is_none());
    }
}
