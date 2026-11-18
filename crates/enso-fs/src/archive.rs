//! Archives: what's in one, taking it out, and making one — all in pure
//! Rust. Read: zip, and tar plain or squeezed with gzip, xz, zstd, or
//! bzip2, and those squeezers on a single file. Made: zip, `.tar.gz`, and
//! `.tar.xz`.
//!
//! Taking out goes into a hidden folder beside where it's going, then
//! settles: an archive holding one folder becomes that folder; anything
//! else, a folder named after the archive — neither ever over something
//! already there. Nothing in an archive can land outside it: absolute
//! names, `..`, and links leading out are skipped, as are devices.

use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crate::ops::{Progress, Report, Watcher, free_name};

/// What kind of archive a file is, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarXz,
    TarZst,
    TarBz2,
    /// One file, squeezed.
    Gz,
    Xz,
    Zst,
    Bz2,
}

impl Format {
    /// The format a file called `name` is in, if it's an archive.
    pub fn of(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let ends = |suffixes: &[&str]| suffixes.iter().any(|suffix| lower.ends_with(suffix));
        Some(if ends(&[".zip", ".cbz", ".jar", ".epub"]) {
            Self::Zip
        } else if ends(&[".tar.gz", ".tgz"]) {
            Self::TarGz
        } else if ends(&[".tar.xz", ".txz"]) {
            Self::TarXz
        } else if ends(&[".tar.zst", ".tzst"]) {
            Self::TarZst
        } else if ends(&[".tar.bz2", ".tbz2", ".tbz"]) {
            Self::TarBz2
        } else if ends(&[".tar"]) {
            Self::Tar
        } else if ends(&[".gz"]) {
            Self::Gz
        } else if ends(&[".xz"]) {
            Self::Xz
        } else if ends(&[".zst"]) {
            Self::Zst
        } else if ends(&[".bz2"]) {
            Self::Bz2
        } else {
            return None;
        })
    }

    fn is_tar(self) -> bool {
        matches!(
            self,
            Self::Tar | Self::TarGz | Self::TarXz | Self::TarZst | Self::TarBz2
        )
    }

    /// What this format adds to a name.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Zip => ".zip",
            Self::Tar => ".tar",
            Self::TarGz => ".tar.gz",
            Self::TarXz => ".tar.xz",
            Self::TarZst => ".tar.zst",
            Self::TarBz2 => ".tar.bz2",
            Self::Gz => ".gz",
            Self::Xz => ".xz",
            Self::Zst => ".zst",
            Self::Bz2 => ".bz2",
        }
    }
}

/// `name` without its archive suffix: `photos.tar.gz` → `photos`.
pub fn stem(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for suffix in [
        ".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tgz", ".txz", ".tzst", ".tbz2", ".tbz",
        ".tar", ".zip", ".cbz", ".jar", ".epub", ".gz", ".xz", ".zst", ".bz2",
    ] {
        if lower.ends_with(suffix) && lower.len() > suffix.len() {
            return name[..name.len() - suffix.len()].to_owned();
        }
    }
    name.to_owned()
}

/// One thing in an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Its path inside, `/`-separated, without a leading `/`.
    pub path: String,
    pub is_dir: bool,
    /// Its size when taken out, where the archive says.
    pub size: u64,
}

/// Counts what passes through, for progress.
struct Counted<R> {
    inner: R,
    count: Arc<AtomicU64>,
}

impl<R: io::Seek> io::Seek for Counted<R> {
    fn seek(&mut self, to: io::SeekFrom) -> io::Result<u64> {
        self.inner.seek(to)
    }
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// The archive's contents, unsqueezed: a stream to read the tar (or the
/// one file) from. xz decodes on a thread of its own into a pipe.
fn unsqueezed(
    path: &Path,
    format: Format,
    count: Arc<AtomicU64>,
) -> io::Result<Box<dyn Read + Send>> {
    let file = Counted {
        inner: File::open(path)?,
        count,
    };
    Ok(match format {
        Format::Tar => Box::new(file),
        Format::TarGz | Format::Gz => {
            Box::new(flate2::read::MultiGzDecoder::new(BufReader::new(file)))
        }
        Format::TarZst | Format::Zst => Box::new(
            ruzstd::decoding::StreamingDecoder::new(BufReader::new(file))
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?,
        ),
        Format::TarBz2 | Format::Bz2 => {
            Box::new(bzip2_rs::DecoderReader::new(BufReader::new(file)))
        }
        Format::TarXz | Format::Xz => {
            let (reader, mut writer) = io::pipe()?;
            thread::Builder::new()
                .name("enso-unxz".to_owned())
                .spawn(move || {
                    let mut input = BufReader::new(file);
                    // A reader that stops early closes the pipe: done.
                    let _ = lzma_rs::xz_decompress(&mut input, &mut writer);
                })?;
            Box::new(reader)
        }
        Format::Zip => return Err(io::Error::other("a zip isn't a stream")),
    })
}

fn invalid(err: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

/// What's in the archive at `path`.
pub fn list(path: &Path) -> io::Result<Vec<Member>> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let format = Format::of(&name).ok_or_else(|| invalid("not an archive"))?;
    match format {
        Format::Zip => {
            let mut zip =
                zip::ZipArchive::new(BufReader::new(File::open(path)?)).map_err(invalid)?;
            (0..zip.len())
                .map(|index| {
                    let file = zip.by_index_raw(index).map_err(invalid)?;
                    Ok(Member {
                        path: file
                            .name()
                            .map_err(invalid)?
                            .trim_start_matches('/')
                            .trim_end_matches('/')
                            .to_owned(),
                        is_dir: file.is_dir(),
                        size: file.size(),
                    })
                })
                .collect()
        }
        _ if format.is_tar() => {
            let mut tar = tar::Archive::new(unsqueezed(path, format, Arc::default())?);
            let mut members = Vec::new();
            for entry in tar.entries()? {
                let entry = entry?;
                let header = entry.header();
                members.push(Member {
                    path: entry
                        .path()?
                        .to_string_lossy()
                        .trim_start_matches("./")
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                        .to_owned(),
                    is_dir: header.entry_type().is_dir(),
                    size: header.size().unwrap_or(0),
                });
            }
            members.retain(|member| !member.path.is_empty());
            Ok(members)
        }
        _ => Ok(vec![Member {
            path: stem(&name),
            is_dir: false,
            size: 0,
        }]),
    }
}

/// Take the one file `member` (its path inside) out of `archive`, into
/// the folder `into`, under its own name. Returns where it landed.
pub fn extract_member(archive: &Path, member: &str, into: &Path) -> io::Result<PathBuf> {
    let name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let format = Format::of(&name).ok_or_else(|| invalid("not an archive"))?;
    let own = Path::new(member)
        .file_name()
        .ok_or_else(|| invalid("no such file in it"))?;
    let target = into.join(own);
    fs::create_dir_all(into)?;
    match format {
        Format::Zip => {
            let mut zip =
                zip::ZipArchive::new(BufReader::new(File::open(archive)?)).map_err(invalid)?;
            let mut file = zip.by_name(member).map_err(invalid)?;
            io::copy(&mut file, &mut File::create(&target)?)?;
        }
        _ if format.is_tar() => {
            let mut tar = tar::Archive::new(unsqueezed(archive, format, Arc::default())?);
            let mut found = false;
            for entry in tar.entries()? {
                let mut entry = entry?;
                let path = entry
                    .path()?
                    .to_string_lossy()
                    .trim_start_matches("./")
                    .to_owned();
                if path == member {
                    io::copy(&mut entry, &mut File::create(&target)?)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(invalid("no such file in it"));
            }
        }
        _ => {
            io::copy(
                &mut unsqueezed(archive, format, Arc::default())?,
                &mut File::create(&target)?,
            )?;
        }
    }
    Ok(target)
}

/// Whether `name` (inside an archive) stays inside: relative, no `..`.
fn inside(name: &Path) -> bool {
    name.components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

/// Take everything out of `archive` into the folder `into`. The report's
/// `done` is what appeared there (one item).
pub fn extract(archive: &Path, into: &Path, watcher: &mut impl Watcher) -> Report {
    let mut report = Report::default();
    let name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(format) = Format::of(&name) else {
        report.failed.push((
            archive.to_owned(),
            "it isn't an archive this can read".to_owned(),
        ));
        return report;
    };
    let staging = into.join(format!(".enso-extracting-{}", std::process::id()));
    let result = fs::create_dir(&staging).and_then(|()| {
        let total = fs::metadata(archive).map(|meta| meta.len()).unwrap_or(0);
        let count = Arc::new(AtomicU64::new(0));
        let mut progress = |items: usize, current: &Path| {
            watcher.progress(&Progress {
                bytes: count.load(Ordering::Relaxed).min(total),
                total_bytes: total,
                items,
                total_items: 0,
                current: current.to_owned(),
            });
        };
        match format {
            Format::Zip => extract_zip(archive, &staging, &count, &mut progress, &mut report),
            _ if format.is_tar() => extract_tar(
                archive,
                format,
                &staging,
                &count,
                &mut progress,
                &mut report,
            ),
            _ => {
                let mut input = unsqueezed(archive, format, count.clone())?;
                let mut output = File::create(staging.join(stem(&name)))?;
                io::copy(&mut input, &mut output).map(drop)
            }
        }
    });
    let result = result.and_then(|()| {
        if watcher.stop() {
            report.stopped = true;
            return Ok(None);
        }
        settle(&staging, into, &stem(&name)).map(Some)
    });
    match result {
        Ok(Some(landed)) => report.done.push((archive.to_owned(), landed)),
        Ok(None) => {}
        Err(err) => report.failed.push((archive.to_owned(), err.to_string())),
    }
    let _ = fs::remove_dir_all(&staging);
    report
}

/// Move what was taken out of the staging folder to where it goes.
fn settle(staging: &Path, into: &Path, stem: &str) -> io::Result<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(staging)?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    match entries.as_slice() {
        // One thing: it, as it's called.
        [only] => {
            let name = only
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let landed = free_name(into, &name);
            fs::rename(only, &landed)?;
            Ok(landed)
        }
        // Several: a folder named after the archive, holding them.
        _ => {
            let landed = free_name(into, stem);
            fs::rename(staging, &landed)?;
            // The staging folder was named to be hidden; the folder is not.
            Ok(landed)
        }
    }
}

fn extract_zip(
    archive: &Path,
    staging: &Path,
    count: &Arc<AtomicU64>,
    progress: &mut impl FnMut(usize, &Path),
    report: &mut Report,
) -> io::Result<()> {
    let file = Counted {
        inner: File::open(archive)?,
        count: count.clone(),
    };
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(invalid)?;
    for index in 0..zip.len() {
        let mut member = zip.by_index(index).map_err(invalid)?;
        let Some(name) = member.enclosed_name().filter(|name| inside(name)) else {
            let shown = member
                .name()
                .map(|name| name.into_owned())
                .unwrap_or_default();
            report.skipped.push(PathBuf::from(shown));
            continue;
        };
        let target = staging.join(&name);
        progress(index, &name);
        if member.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if member.is_symlink() {
            // A link from a zip could lead anywhere: left out.
            report.skipped.push(name);
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = BufWriter::new(File::create(&target)?);
        io::copy(&mut member, &mut out)?;
        out.flush()?;
        if let Some(mode) = member.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777));
        }
    }
    Ok(())
}

fn extract_tar(
    archive: &Path,
    format: Format,
    staging: &Path,
    count: &Arc<AtomicU64>,
    progress: &mut impl FnMut(usize, &Path),
    report: &mut Report,
) -> io::Result<()> {
    let mut tar = tar::Archive::new(unsqueezed(archive, format, count.clone())?);
    tar.set_preserve_permissions(false);
    tar.set_overwrite(false);
    for (index, entry) in tar.entries()?.enumerate() {
        let mut entry = entry?;
        let name = entry.path()?.into_owned();
        let kind = entry.header().entry_type();
        let safe = inside(&name)
            && match kind {
                tar::EntryType::Regular
                | tar::EntryType::Directory
                | tar::EntryType::Continuous => true,
                // A link only if where it leads stays inside.
                tar::EntryType::Symlink => entry.link_name().ok().flatten().is_some_and(|target| {
                    let resolved = name.parent().unwrap_or(Path::new("")).join(target);
                    !resolved.is_absolute() && leads_inside(&resolved)
                }),
                _ => false,
            };
        if !safe {
            report.skipped.push(name);
            continue;
        }
        progress(index, &name);
        entry.unpack_in(staging)?;
    }
    Ok(())
}

/// Whether a relative path, `..` and all, stays at or below where it
/// starts.
fn leads_inside(path: &Path) -> bool {
    let mut depth = 0i32;
    for part in path.components() {
        match part {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}

/// Make the archive `out` (its format from its name: zip, `.tar.gz`, or
/// `.tar.xz`) of `sources`, each under its own name. It's written beside
/// itself and put in place when whole; the report's `done` holds it.
pub fn compress(sources: &[PathBuf], out: &Path, watcher: &mut impl Watcher) -> Report {
    let mut report = Report::default();
    let name = out
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let format = Format::of(&name);
    let part = out.with_file_name(format!(".{name}.part"));
    // Everything to put in, with the name it goes in under.
    let mut members: Vec<(PathBuf, PathBuf)> = Vec::new();
    for source in sources {
        let Some(base) = source.parent() else {
            continue;
        };
        let mut stack = vec![source.clone()];
        while let Some(path) = stack.pop() {
            let relative = path.strip_prefix(base).unwrap_or(&path).to_owned();
            if path.is_dir()
                && !path.is_symlink()
                && let Ok(entries) = fs::read_dir(&path)
            {
                let mut children: Vec<PathBuf> =
                    entries.flatten().map(|entry| entry.path()).collect();
                children.sort();
                stack.extend(children.into_iter().rev());
            }
            members.push((path, relative));
        }
    }
    let total: u64 = members
        .iter()
        .filter_map(|(path, _)| fs::symlink_metadata(path).ok())
        .filter(fs::Metadata::is_file)
        .map(|meta| meta.len())
        .sum();
    let mut done_bytes = 0u64;
    let total_items = members.len();
    let result = (|| -> io::Result<()> {
        let file = BufWriter::new(File::create(&part)?);
        let mut each = |index: usize,
                        path: &Path,
                        watcher: &mut dyn FnMut(&Progress) -> bool|
         -> io::Result<bool> {
            let size = fs::symlink_metadata(path)
                .map(|meta| if meta.is_file() { meta.len() } else { 0 })
                .unwrap_or(0);
            done_bytes += size;
            Ok(watcher(&Progress {
                bytes: done_bytes,
                total_bytes: total,
                items: index + 1,
                total_items,
                current: path.to_owned(),
            }))
        };
        let mut tell = |progress: &Progress| {
            watcher.progress(progress);
            !watcher.stop()
        };
        match format {
            Some(Format::Zip) => {
                let mut zip = zip::ZipWriter::new(file);
                for (index, (path, relative)) in members.iter().enumerate() {
                    if !each(index, path, &mut tell)? {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
                    }
                    let meta = fs::symlink_metadata(path)?;
                    let mode = {
                        use std::os::unix::fs::PermissionsExt;
                        meta.permissions().mode() & 0o777
                    };
                    let options = zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(mode);
                    let name = relative.to_string_lossy();
                    if meta.is_dir() {
                        zip.add_directory(name, options).map_err(invalid)?;
                    } else if meta.is_symlink() {
                        let target = fs::read_link(path)?;
                        zip.add_symlink(name, target.to_string_lossy(), options)
                            .map_err(invalid)?;
                    } else {
                        zip.start_file(name, options).map_err(invalid)?;
                        io::copy(&mut File::open(path)?, &mut zip)?;
                    }
                }
                zip.finish().map_err(invalid)?.flush()
            }
            Some(Format::TarGz) => {
                let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
                let mut tar = tar::Builder::new(gz);
                tar.follow_symlinks(false);
                for (index, (path, relative)) in members.iter().enumerate() {
                    if !each(index, path, &mut tell)? {
                        return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
                    }
                    tar.append_path_with_name(path, relative)?;
                }
                tar.into_inner()?.finish()?.flush()
            }
            Some(Format::TarXz) => {
                // The tar first, then squeezed: lzma-rs takes a whole stream.
                let plain = out.with_file_name(format!(".{name}.tar.part"));
                {
                    let mut tar = tar::Builder::new(BufWriter::new(File::create(&plain)?));
                    tar.follow_symlinks(false);
                    for (index, (path, relative)) in members.iter().enumerate() {
                        if !each(index, path, &mut tell)? {
                            let _ = fs::remove_file(&plain);
                            return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
                        }
                        tar.append_path_with_name(path, relative)?;
                    }
                    tar.into_inner()?.flush()?;
                }
                let squeezed = (|| {
                    let mut input = BufReader::new(File::open(&plain)?);
                    let mut output = file;
                    lzma_rs::xz_compress(&mut input, &mut output)?;
                    output.flush()
                })();
                let _ = fs::remove_file(&plain);
                squeezed
            }
            _ => Err(invalid("archives are made as .zip, .tar.gz, or .tar.xz")),
        }
    })();
    match result {
        Ok(()) => match fs::rename(&part, out) {
            Ok(()) => report.done.push((out.to_owned(), out.to_owned())),
            Err(err) => report.failed.push((out.to_owned(), err.to_string())),
        },
        Err(err) => {
            let _ = fs::remove_file(&part);
            if err.kind() == io::ErrorKind::Interrupted {
                report.stopped = true;
            } else {
                report.failed.push((out.to_owned(), err.to_string()));
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Resolve;

    struct Quiet;

    impl Watcher for Quiet {
        fn progress(&mut self, _: &Progress) {}
        fn conflict(&mut self, _: &Path, _: &Path) -> (Resolve, bool) {
            (Resolve::Skip, true)
        }
        fn stop(&self) -> bool {
            false
        }
    }

    fn tree(root: &Path) {
        fs::create_dir_all(root.join("album/inner")).unwrap();
        fs::write(root.join("album/a.txt"), "first").unwrap();
        fs::write(root.join("album/inner/b.txt"), "second".repeat(1000)).unwrap();
        std::os::unix::fs::symlink("a.txt", root.join("album/link")).unwrap();
    }

    #[test]
    fn made_listed_and_taken_out_in_every_format() {
        let root = std::env::temp_dir().join(format!("enso-fs-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        tree(&root);
        for suffix in [".zip", ".tar.gz", ".tar.xz"] {
            let out = root.join(format!("album{suffix}"));
            let report = compress(&[root.join("album")], &out, &mut Quiet);
            assert!(report.failed.is_empty(), "{suffix}: {:?}", report.failed);
            assert!(out.exists());
            let mut names: Vec<String> = list(&out)
                .unwrap()
                .into_iter()
                .map(|member| member.path)
                .collect();
            names.sort();
            assert_eq!(
                names,
                [
                    "album",
                    "album/a.txt",
                    "album/inner",
                    "album/inner/b.txt",
                    "album/link"
                ],
                "{suffix}"
            );

            // Out again, beside a folder already called album: it lands
            // under a free name, whole.
            let into = root.join(format!("out{}", suffix.replace('.', "-")));
            fs::create_dir_all(into.join("album")).unwrap();
            let report = extract(&out, &into, &mut Quiet);
            assert!(report.failed.is_empty(), "{suffix}: {:?}", report.failed);
            let landed = &report.done[0].1;
            assert_eq!(landed, &into.join("album 2"), "{suffix}");
            assert_eq!(
                fs::read_to_string(landed.join("inner/b.txt")).unwrap(),
                "second".repeat(1000)
            );
            // A link that stays inside comes out as a link (from a tar).
            if suffix != ".zip" {
                assert_eq!(
                    fs::read_link(landed.join("link")).unwrap(),
                    Path::new("a.txt")
                );
            }
            assert!(
                fs::read_dir(&into)
                    .unwrap()
                    .flatten()
                    .all(|entry| !entry.file_name().to_string_lossy().starts_with(".enso"))
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nothing_lands_outside() {
        let root =
            std::env::temp_dir().join(format!("enso-fs-archive-evil-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("into")).unwrap();
        // A tar naming ../escape, and a link to /etc.
        let evil = root.join("evil.tar");
        {
            let mut tar = tar::Builder::new(File::create(&evil).unwrap());
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            // Written by hand: the builder refuses `..` itself.
            let name = b"../escape";
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_cksum();
            tar.append(&header, &b"evil"[..]).unwrap();
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            tar.append_link(&mut link, "out", "/etc").unwrap();
            let mut ok = tar::Header::new_gnu();
            ok.set_size(2);
            ok.set_mode(0o644);
            tar.append_data(&mut ok, "fine.txt", &b"ok"[..]).unwrap();
            tar.finish().unwrap();
        }
        let report = extract(&evil, &root.join("into"), &mut Quiet);
        assert!(!root.join("escape").exists());
        assert_eq!(report.skipped.len(), 2, "{:?}", report.skipped);
        assert_eq!(
            fs::read_to_string(root.join("into/fine.txt")).unwrap(),
            "ok"
        );
        assert!(inside(Path::new("a/b")) && !inside(Path::new("../a")) && !inside(Path::new("/a")));
        assert!(leads_inside(Path::new("a/../b")) && !leads_inside(Path::new("a/../../b")));
        assert_eq!(stem("Photos.TAR.GZ"), "Photos");
        assert_eq!(Format::of("x.tgz"), Some(Format::TarGz));
        fs::remove_dir_all(root).unwrap();
    }

    /// Archives other programs made: `ENSO_ARCHIVES` names a folder of them.
    #[test]
    #[ignore = "needs archives made elsewhere"]
    fn reads_what_others_made() {
        let dir = PathBuf::from(std::env::var("ENSO_ARCHIVES").unwrap());
        for name in ["t.tar.zst", "t.tar.bz2", "one.txt.gz"] {
            let archive = dir.join(name);
            let members = list(&archive).unwrap();
            assert!(!members.is_empty(), "{name}");
            let into = dir.join(format!("out-{name}"));
            let _ = fs::remove_dir_all(&into);
            fs::create_dir_all(&into).unwrap();
            let report = extract(&archive, &into, &mut Quiet);
            assert!(report.failed.is_empty(), "{name}: {:?}", report.failed);
            eprintln!(
                "{name}: {:?} -> {:?}",
                members.iter().map(|m| &m.path).collect::<Vec<_>>(),
                report.done
            );
        }
    }

    /// Archives made here, left in `ENSO_ARCHIVES` for other programs to read.
    #[test]
    #[ignore = "for checking by hand"]
    fn makes_what_others_read() {
        let dir = PathBuf::from(std::env::var("ENSO_ARCHIVES").unwrap());
        for suffix in [".zip", ".tar.gz", ".tar.xz"] {
            let report = compress(
                &[dir.join("src")],
                &dir.join(format!("ours{suffix}")),
                &mut Quiet,
            );
            assert!(report.failed.is_empty());
        }
    }
}
