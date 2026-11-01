//! PHP's built-in classes and functions from JetBrains phpstorm-stubs (Apache-2.0, see
//! STUBS-LICENSE): packed into the binary (`stubs.bin.gz`, refreshed by
//! script/update-stubs.sh), unpacked once into a folder that is indexed like project code, so
//! Go to Declaration and Quick Documentation open the real signatures and docs.

use std::io::Read;
use std::path::{Path, PathBuf};

const PACKED: &[u8] = include_bytes!("../stubs.bin.gz");

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.at..self.at + n)?;
        self.at += n;
        Some(slice)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn chunk(&mut self) -> Option<Vec<u8>> {
        let len = self.u32()? as usize;
        self.take(len).map(<[u8]>::to_vec)
    }
}

/// Reads the packed archive: `PCST`, the version, a file count, then (path, content) pairs,
/// each a little-endian u32 length and its bytes.
fn unpacked() -> Option<(String, Vec<(String, Vec<u8>)>)> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(PACKED).read_to_end(&mut raw).ok()?;
    let mut r = Reader { bytes: &raw, at: 0 };
    if r.take(4)? != b"PCST" {
        return None;
    }
    let version = String::from_utf8(r.chunk()?).ok()?;
    let count = r.u32()?;
    let mut files = Vec::new();
    for _ in 0..count {
        let path = String::from_utf8(r.chunk()?).ok()?;
        files.push((path, r.chunk()?));
    }
    Some((version, files))
}

/// Unpacks the stubs into `<base>/<version>` (once) and returns that folder.
pub fn unpack(base: &Path) -> Option<PathBuf> {
    let (version, files) = unpacked()?;
    let dir = base.join(&version);
    let done = dir.join(".complete");
    if done.is_file() {
        return Some(dir);
    }
    for (path, content) in files {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent()?).ok()?;
        std::fs::write(&target, content).ok()?;
    }
    std::fs::write(&done, b"").ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_and_indexes() {
        let base = std::env::temp_dir().join(format!("phpcraft-stubs-{}", std::process::id()));
        let dir = unpack(&base).unwrap();
        assert!(dir.join("Core/Core_c.php").is_file() || dir.join("standard/standard_0.php").is_file());
        let mut index = crate::Index::new();
        index.add_dir(&dir, None);
        assert!(index.class("PDO").is_some());
        assert!(index.class("DateTimeImmutable").is_some());
        assert!(index.find_member("PDO", "prepare", &[crate::MemberKind::Method]).is_some());
        assert!(index.function("array_map").is_some());
        assert!(index.function("str_contains").is_some());
        std::fs::remove_dir_all(&base).ok();
    }
}
