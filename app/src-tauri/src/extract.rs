//! Unpacks a finished archive into a folder beside it: .zip, .tar.gz, .tgz,
//! .tar and .7z. Paths that climb out of the folder (`../x`) are never written.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const KINDS: [&str; 5] = [".tar.gz", ".tgz", ".tar", ".zip", ".7z"];

fn kind(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    KINDS.into_iter().find(|k| n.ends_with(k))
}

pub fn is_archive(name: &str) -> bool {
    kind(name).is_some()
}

/// The folder name for an archive: its name without the extension.
fn stem(name: &str) -> String {
    match kind(name) {
        Some(k) => name[..name.len() - k.len()].to_string(),
        None => name.to_string(),
    }
}

/// `dir/name`, or `dir/name (2)`... when that is taken.
fn free_folder(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    (1..).map(|n| if n == 1 { first.clone() } else { dir.join(format!("{name} ({n})")) }).find(|p| !p.exists()).unwrap()
}

/// Unpacks `archive` and returns the folder it went into.
pub fn unpack(archive: &Path) -> Result<PathBuf> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = kind(&name).context("not an archive SwiftDL can unpack")?;
    let dir = archive.parent().unwrap_or(Path::new("."));
    let target = free_folder(dir, &stem(&name));
    std::fs::create_dir_all(&target)?;
    let done = match kind {
        ".zip" => unzip(archive, &target),
        ".tar.gz" | ".tgz" => untar(flate2::read::GzDecoder::new(std::fs::File::open(archive)?), &target),
        ".tar" => untar(std::fs::File::open(archive)?, &target),
        _ => un7z(archive, &target),
    };
    if let Err(e) = done {
        let _ = std::fs::remove_dir_all(&target); // no half-unpacked folder
        return Err(e);
    }
    Ok(target)
}

fn unzip(archive: &Path, target: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?).context("not a valid zip file")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else { continue }; // unsafe path: skipped
        let out = target.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut std::fs::File::create(&out)?)?;
        }
    }
    Ok(())
}

fn untar(reader: impl std::io::Read, target: &Path) -> Result<()> {
    // `unpack` refuses entries that would land outside `target`.
    tar::Archive::new(reader).unpack(target).context("couldn't unpack the tar file")
}

fn un7z(archive: &Path, target: &Path) -> Result<()> {
    let reader = sevenz_rust::SevenZReader::open(archive, sevenz_rust::Password::empty()).context("couldn't open the 7z file (it may be encrypted)")?;
    for entry in &reader.archive().files {
        let n = entry.name().replace('\\', "/");
        if n.starts_with('/') || n.split('/').any(|p| p == "..") || n.contains(':') {
            bail!("the archive has a path that leaves its folder: {n}");
        }
    }
    sevenz_rust::decompress_file(archive, target).context("couldn't unpack the 7z file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("swiftdl-extract-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names() {
        assert!(is_archive("A.ZIP") && is_archive("x.tar.gz") && is_archive("x.7z") && !is_archive("x.rar") && !is_archive("x.iso"));
        assert_eq!(stem("project.tar.gz"), "project");
        assert_eq!(stem("Photos.zip"), "Photos");
    }

    #[test]
    fn unzips_safely_into_a_free_folder() {
        let dir = temp("zip");
        let file = dir.join("pack.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("a/b.txt", o).unwrap();
        w.write_all(b"hello").unwrap();
        w.start_file("../escape.txt", o).unwrap();
        w.write_all(b"bad").unwrap();
        w.finish().unwrap();
        let out = unpack(&file).unwrap();
        assert_eq!(out, dir.join("pack"));
        assert_eq!(std::fs::read(out.join("a/b.txt")).unwrap(), b"hello");
        assert!(!dir.join("escape.txt").exists(), "a path that climbs out is never written");
        assert_eq!(unpack(&file).unwrap(), dir.join("pack (2)"), "an existing folder is kept");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn untars_tar_gz() {
        let dir = temp("tgz");
        let file = dir.join("pack.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&file).unwrap(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let data = b"tar text";
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "d/x.txt", &data[..]).unwrap();
        b.into_inner().unwrap().finish().unwrap();
        let out = unpack(&file).unwrap();
        assert_eq!(std::fs::read(out.join("d/x.txt")).unwrap(), b"tar text");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_bad_archive_leaves_no_folder() {
        let dir = temp("bad");
        let file = dir.join("broken.zip");
        std::fs::write(&file, b"this is not a zip").unwrap();
        assert!(unpack(&file).is_err());
        assert!(!dir.join("broken").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// With SWIFTDL_7Z set to a .7z file: `cargo test seven -- --ignored`.
    #[test]
    #[ignore]
    fn seven_zip() {
        let f = PathBuf::from(std::env::var("SWIFTDL_7Z").unwrap());
        let out = unpack(&f).unwrap();
        println!("UNPACKED {}", out.display());
    }
}
