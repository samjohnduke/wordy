//! Zip a whole project folder: the backup / move-between-machines format.
//! The SQLite index is derived and left out.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Component, Path};

use anyhow::{anyhow, Context, Result};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Files (by name) that never go into the archive.
fn skip(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with("index.sqlite") || name.ends_with(".tmp") || name == ".DS_Store"
}

/// Write `dir` into a zip at `out`, under a top-level folder named after `dir`.
/// Returns the number of files written.
pub fn zip_project(dir: &Path, out: &Path) -> Result<usize> {
    let file = File::create(out).with_context(|| format!("creating {}", out.display()))?;
    let n = write_zip(dir, io::BufWriter::new(file))?;
    Ok(n)
}

pub fn write_zip<W: Write + io::Seek>(dir: &Path, writer: W) -> Result<usize> {
    let root_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("project folder has no name"))?;
    let mut zip = ZipWriter::new(writer);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut count = 0;
    for entry in WalkDir::new(dir).sort_by_file_name().into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path == dir || skip(path) {
            continue;
        }
        let rel = path.strip_prefix(dir)?;
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            continue;
        }
        let mut name = format!("{root_name}/{}", rel.to_string_lossy().replace('\\', "/"));
        if entry.file_type().is_dir() {
            name.push('/');
            zip.add_directory(name, opts)?;
        } else if entry.file_type().is_file() {
            zip.start_file(name, opts)?;
            let mut f = File::open(path).with_context(|| format!("reading {}", path.display()))?;
            io::copy(&mut f, &mut zip)?;
            count += 1;
        }
    }
    zip.finish()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn zips_folder_without_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("My Novel");
        std::fs::create_dir_all(dir.join("snapshots")).unwrap();
        std::fs::write(dir.join("project.loro"), b"loro").unwrap();
        std::fs::write(dir.join("project.json"), b"{}").unwrap();
        std::fs::write(dir.join("index.sqlite"), b"db").unwrap();
        std::fs::write(dir.join("snapshots/a.loro"), b"old").unwrap();

        let mut buf = Cursor::new(Vec::new());
        let n = write_zip(&dir, &mut buf).unwrap();
        assert_eq!(n, 3);
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"My Novel/project.loro".to_string()), "{names:?}");
        assert!(names.contains(&"My Novel/snapshots/a.loro".to_string()));
        assert!(!names.iter().any(|n| n.contains("index.sqlite")));
        let mut s = String::new();
        zip.by_name("My Novel/project.loro")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "loro");
    }
}
