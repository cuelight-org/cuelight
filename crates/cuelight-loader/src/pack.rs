//! Packed shows: a show folder as one `.cuelight` file, a plain zip with
//! the folder's contents at its root (`show.json`, `test-driver.json`,
//! `assets/...`). Any zip tool opens it; media that is already compressed
//! is stored as is, the rest deflated.

use crate::{LoadError, Manifest};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The file extension of a packed show.
pub const PACK_EXTENSION: &str = "cuelight";

/// Formats that compress no further: stored as they are.
const STORED: &[&str] = &["png", "jpg", "jpeg", "ogg", "mp3", "flac", "woff", "woff2"];

fn zip_error(path: &Path, e: impl std::fmt::Display) -> LoadError {
    LoadError::Asset {
        path: path.to_owned(),
        message: e.to_string(),
    }
}

/// Pack the show folder at `dir` into the file at `out`, the files a
/// [`Manifest`] lists (nothing else: no manifest, no stray files). Returns
/// how many files went in.
///
/// The folder is read into memory and packed with [`pack_bytes`], so the
/// same check applies: a show that does not load is refused.
pub fn pack(dir: impl AsRef<Path>, out: impl AsRef<Path>) -> Result<usize, LoadError> {
    let (dir, out) = (dir.as_ref(), out.as_ref());
    let manifest = Manifest::for_dir(dir)?;
    let mut files = BTreeMap::new();
    for name in &manifest.files {
        files.insert(name.clone(), crate::read(&dir.join(name))?);
    }
    let bytes = pack_bytes(&files).map_err(|e| match e {
        // Paths inside the show are relative; say which show.
        LoadError::NoShowDocument(_) => LoadError::NoShowDocument(dir.to_owned()),
        LoadError::Engine { source, .. } => LoadError::Engine {
            path: dir.join("show.json"),
            source,
        },
        other => other,
    })?;
    std::fs::write(out, bytes).map_err(|source| LoadError::Io {
        path: out.to_owned(),
        source,
    })?;
    Ok(files.len())
}

/// Pack a show held in memory into the bytes of a `.cuelight` file:
/// `files` maps paths relative to the show folder (`show.json`,
/// `assets/orb.png`, ...) to their bytes, as [`unpack`] and
/// [`load_from_memory`](crate::load_from_memory) take them, and every
/// entry goes in. For a host without a disk: an editor in a browser opens
/// a pack from bytes and hands one back as a download.
///
/// The show is loaded first and the pack is refused if it does not load,
/// so a corrupt asset cannot be shipped: packing only moves bytes and
/// would not otherwise notice.
pub fn pack_bytes(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, LoadError> {
    check(files)?;
    let here = Path::new("pack");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        let extension = crate::extension(Path::new(name));
        let method = if STORED.contains(&extension.as_str()) {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let options = zip::write::SimpleFileOptions::default().compression_method(method);
        writer
            .start_file(name, options)
            .and_then(|()| writer.write_all(bytes).map_err(Into::into))
            .map_err(|e| zip_error(here, e))?;
    }
    let cursor = writer.finish().map_err(|e| zip_error(here, e))?;
    Ok(cursor.into_inner())
}

/// Refuse files that would pack into something that cannot load.
///
/// Loading is the check: it decodes every asset, so a corrupt one fails
/// here rather than in whatever opens the pack. Packing itself only moves
/// bytes and would not notice.
fn check(files: &BTreeMap<String, Vec<u8>>) -> Result<(), LoadError> {
    crate::load_from_memory(&mut cuelight::Engine::new(), files).map(|_| ())
}

/// The files of a packed show, by their path in the folder, from the
/// bytes of a `.cuelight` file (or any zip of a show folder: one folder
/// wrapping everything, as archivers make, is unwrapped).
pub fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, LoadError> {
    let here = Path::new("pack");
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| zip_error(here, e))?;
    let mut files = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| zip_error(here, e))?;
        if entry.is_dir() {
            continue;
        }
        // Only sane relative paths, with `/` separators.
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let name = name
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let mut content = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut content)
            .map_err(|e| zip_error(here, e))?;
        files.insert(name, content);
    }
    if files.contains_key("show.json") {
        return Ok(files);
    }
    // A zip of the folder itself: everything under one directory.
    let prefix = files
        .keys()
        .next()
        .and_then(|k| k.split_once('/'))
        .map(|(top, _)| format!("{top}/"));
    if let Some(prefix) = prefix.filter(|p| files.keys().all(|k| k.starts_with(p.as_str()))) {
        return Ok(files
            .into_iter()
            .map(|(k, v)| (k[prefix.len()..].to_owned(), v))
            .collect());
    }
    Err(LoadError::NoShowDocument(PathBuf::from(here)))
}

/// Read a `.cuelight` file into its files; see [`unpack`].
pub fn read_pack(path: impl AsRef<Path>) -> Result<BTreeMap<String, Vec<u8>>, LoadError> {
    let path = path.as_ref();
    unpack(&crate::read(path)?).map_err(|e| match e {
        LoadError::NoShowDocument(_) => LoadError::NoShowDocument(path.to_owned()),
        LoadError::Asset { message, .. } => LoadError::Asset {
            path: path.to_owned(),
            message,
        },
        other => other,
    })
}
