//! The trash, as the freedesktop Trash specification has it, so files
//! trashed here can be restored from any other file manager and the other
//! way round.
//!
//! A trash is a folder with `files/` (what was trashed, under a unique
//! name) and `info/` (a `.trashinfo` per item: where it was, and when it
//! was trashed). Files in the home filesystem go to `$XDG_DATA_HOME/Trash`.
//! Files on another filesystem — a USB stick, a second disk — stay on it,
//! in that filesystem's own trash at its top: `.Trash/$uid` if the
//! administrator made a shared `.Trash` (sticky, not a link), else
//! `.Trash-$uid`. Moving a file into a trash is then a rename, never a copy.

use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use jiff::Zoned;

/// One trash: its folder, and the folder its `Path=` entries are relative
/// to (`None` for the home trash, whose paths are absolute).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trash {
    pub dir: PathBuf,
    pub top: Option<PathBuf>,
}

/// Something in a trash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Its name in the trash, unique there.
    pub name: String,
    /// Where it was.
    pub original: PathBuf,
    /// When it was trashed, as written (`2026-11-18T12:30:05`).
    pub deleted: String,
    pub trash: Trash,
}

impl Item {
    /// Where it is now.
    pub fn path(&self) -> PathBuf {
        self.trash.dir.join("files").join(&self.name)
    }

    fn info(&self) -> PathBuf {
        self.trash
            .dir
            .join("info")
            .join(format!("{}.trashinfo", self.name))
    }
}

/// The home trash.
pub fn home_trash() -> Trash {
    let home = crate::places::home();
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    Trash {
        dir: data.join("Trash"),
        top: None,
    }
}

/// The trash `path` goes to: the home trash when it's on the same
/// filesystem, else its filesystem's own.
pub fn trash_for(path: &Path) -> io::Result<Trash> {
    let home = home_trash();
    let device = fs::symlink_metadata(path)?.dev();
    let home_device = fs::metadata(nearest_existing(&home.dir))?.dev();
    if device == home_device {
        return Ok(home);
    }
    let top = mount_top(path, device)?;
    let uid = rustix::process::getuid().as_raw();
    let shared = top.join(".Trash");
    if let Ok(meta) = fs::symlink_metadata(&shared)
        && meta.is_dir()
        && meta.permissions().mode() & 0o1000 != 0
    {
        return Ok(Trash {
            dir: shared.join(uid.to_string()),
            top: Some(top),
        });
    }
    Ok(Trash {
        dir: top.join(format!(".Trash-{uid}")),
        top: Some(top),
    })
}

fn nearest_existing(path: &Path) -> &Path {
    path.ancestors()
        .find(|dir| dir.exists())
        .unwrap_or(Path::new("/"))
}

/// The top of the filesystem `path` (on `device`) is on: the highest folder
/// above it on the same device.
fn mount_top(path: &Path, device: u64) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let mut top = path.parent().unwrap_or(Path::new("/")).to_owned();
    while let Some(up) = top.parent() {
        if fs::metadata(up)?.dev() != device {
            break;
        }
        top = up.to_owned();
    }
    Ok(top)
}

/// Move `path` to its trash. Returns what it became there.
pub fn trash(path: &Path) -> io::Result<Item> {
    let path = std::path::absolute(path)?;
    if path.parent().is_none() {
        return Err(io::Error::other("the root can't be trashed"));
    }
    let trash = trash_for(&path)?;
    trash_into(&path, &trash)
}

fn trash_into(path: &Path, trash: &Trash) -> io::Result<Item> {
    let files = trash.dir.join("files");
    let info = trash.dir.join("info");
    for dir in [&files, &info] {
        fs::create_dir_all(dir)?;
        // A trash is private.
        fs::set_permissions(&trash.dir, fs::Permissions::from_mode(0o700))?;
    }
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| io::Error::other("nothing to trash"))?;
    let written = match &trash.top {
        Some(top) => path.strip_prefix(top).unwrap_or(path).to_owned(),
        None => path.to_owned(),
    };
    let deleted = Zoned::now().strftime("%Y-%m-%dT%H:%M:%S").to_string();
    let text = format!(
        "[Trash Info]\nPath={}\nDeletionDate={deleted}\n",
        encode(&written)
    );
    // Claim a name by creating its info file exclusively: two programs
    // trashing at once can't both get it.
    for n in 1.. {
        let name = if n == 1 {
            base.clone()
        } else {
            numbered(&base, n)
        };
        let info_path = info.join(format!("{name}.trashinfo"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&info_path)
        {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                let target = files.join(&name);
                if target.exists() || target.is_symlink() {
                    // A stray left by something else: try the next name.
                    let _ = fs::remove_file(&info_path);
                    continue;
                }
                if let Err(err) = fs::rename(path, &target) {
                    let _ = fs::remove_file(&info_path);
                    return Err(err);
                }
                return Ok(Item {
                    name,
                    original: path.to_owned(),
                    deleted,
                    trash: trash.clone(),
                });
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    unreachable!("names run out after every number")
}

/// `name` with a number before its extension: `photo 2.png`.
fn numbered(name: &str, n: usize) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} {n}{}", &name[..dot], &name[dot..]),
        _ => format!("{name} {n}"),
    }
}

/// Put `item` back where it was. Fails, touching nothing, if something is
/// there now.
pub fn restore(item: &Item) -> io::Result<PathBuf> {
    let to = item.original.clone();
    if to.exists() || to.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is there again", to.display()),
        ));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(item.path(), &to)?;
    let _ = fs::remove_file(item.info());
    Ok(to)
}

/// Delete `item` for good.
pub fn delete(item: &Item) -> io::Result<()> {
    let path = item.path();
    if path.is_dir() && !path.is_symlink() {
        fs::remove_dir_all(&path)?;
    } else {
        fs::remove_file(&path)?;
    }
    let _ = fs::remove_file(item.info());
    Ok(())
}

/// What's in `trash`, newest first. Items whose info is unreadable are
/// left out, as the spec says.
pub fn list(trash: &Trash) -> Vec<Item> {
    let Ok(entries) = fs::read_dir(trash.dir.join("info")) else {
        return Vec::new();
    };
    let mut items: Vec<Item> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let file = entry.file_name().into_string().ok()?;
            let name = file.strip_suffix(".trashinfo")?.to_owned();
            let text = fs::read_to_string(entry.path()).ok()?;
            let (path, deleted) = parse(&text)?;
            let original = match &trash.top {
                Some(top) if path.is_relative() => top.join(path),
                _ => path,
            };
            let item = Item {
                name,
                original,
                deleted,
                trash: trash.clone(),
            };
            (item.path().exists() || item.path().is_symlink()).then_some(item)
        })
        .collect();
    items.sort_by(|a, b| b.deleted.cmp(&a.deleted).then_with(|| a.name.cmp(&b.name)));
    items
}

/// Delete everything in `trash`.
pub fn empty(trash: &Trash) -> io::Result<()> {
    for dir in ["files", "info", "directorysizes"] {
        let path = trash.dir.join(dir);
        if dir == "directorysizes" {
            let _ = fs::remove_file(&path);
            continue;
        }
        if path.exists() {
            fs::remove_dir_all(&path)?;
            fs::create_dir_all(&path)?;
        }
    }
    Ok(())
}

/// The `Path=` and `DeletionDate=` of a `.trashinfo`.
fn parse(text: &str) -> Option<(PathBuf, String)> {
    let mut in_section = false;
    let (mut path, mut deleted) = (None, String::new());
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Trash Info]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(value) = line.strip_prefix("Path=") {
            path = decode(value);
        } else if let Some(value) = line.strip_prefix("DeletionDate=") {
            deleted = value.to_owned();
        }
    }
    path.map(|path| (path, deleted))
}

/// A path as `Path=` wants it: escaped like a URI's, `/` kept.
fn encode(path: &Path) -> String {
    let uri = crate::uri::from_path(path);
    // `from_path` writes a whole URI; this wants just the path part, and
    // relative paths too.
    let encoded = uri.trim_start_matches("file://");
    if path.is_relative() {
        encoded.trim_start_matches('/').to_owned()
    } else {
        encoded.to_owned()
    }
}

fn decode(value: &str) -> Option<PathBuf> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = (input.next()? as char).to_digit(16)?;
            let low = (input.next()? as char).to_digit(16)?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(OsStr::from_bytes(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> (PathBuf, Trash) {
        let root =
            std::env::temp_dir().join(format!("enso-fs-trash-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("work")).unwrap();
        let trash = Trash {
            dir: root.join("Trash"),
            top: None,
        };
        (root, trash)
    }

    #[test]
    fn trashed_files_can_come_back() {
        let (root, trash) = sandbox("restore");
        let file = root.join("work/report final.txt");
        fs::write(&file, "draft").unwrap();
        let item = trash_into(&file, &trash).unwrap();
        assert!(!file.exists());
        assert_eq!(fs::read_to_string(item.path()).unwrap(), "draft");
        let info = fs::read_to_string(item.info()).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="), "{info}");
        assert!(info.contains("report%20final.txt"), "{info}");

        let listed = list(&trash);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].original, file);

        assert_eq!(restore(&listed[0]).unwrap(), file);
        assert_eq!(fs::read_to_string(&file).unwrap(), "draft");
        assert!(list(&trash).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn names_never_collide_in_the_trash() {
        let (root, trash) = sandbox("names");
        for content in ["one", "two", "three"] {
            fs::write(root.join("work/photo.png"), content).unwrap();
            trash_into(&root.join("work/photo.png"), &trash).unwrap();
        }
        let mut names: Vec<String> = list(&trash).into_iter().map(|item| item.name).collect();
        names.sort();
        assert_eq!(names, ["photo 2.png", "photo 3.png", "photo.png"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restoring_over_something_new_is_refused() {
        let (root, trash) = sandbox("refuse");
        let file = root.join("work/a.txt");
        fs::write(&file, "old").unwrap();
        let item = trash_into(&file, &trash).unwrap();
        fs::write(&file, "new").unwrap();
        assert!(restore(&item).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
        assert!(item.path().exists(), "still in the trash");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folders_go_whole_and_emptying_empties() {
        let (root, trash) = sandbox("empty");
        fs::create_dir_all(root.join("work/project/src")).unwrap();
        fs::write(root.join("work/project/src/main.rs"), "fn main() {}").unwrap();
        let item = trash_into(&root.join("work/project"), &trash).unwrap();
        assert!(item.path().join("src/main.rs").exists());
        fs::write(root.join("work/b"), "").unwrap();
        let other = trash_into(&root.join("work/b"), &trash).unwrap();
        delete(&other).unwrap();
        assert_eq!(list(&trash).len(), 1);
        empty(&trash).unwrap();
        assert!(list(&trash).is_empty());
        assert!(trash.dir.join("files").is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn info_files_read_both_ways() {
        let (path, deleted) =
            parse("[Trash Info]\nPath=/home/me/caf%C3%A9.txt\nDeletionDate=2026-11-18T12:00:00\n")
                .unwrap();
        assert_eq!(path, Path::new("/home/me/café.txt"));
        assert_eq!(deleted, "2026-11-18T12:00:00");
        assert_eq!(encode(Path::new("/home/me/a b")), "/home/me/a%20b");
        assert_eq!(encode(Path::new("media/a b")), "media/a%20b");
        assert!(parse("[Other]\nPath=/x\n").is_none());
    }

    #[test]
    fn the_home_trash_takes_home_files() {
        let home_file = std::env::temp_dir();
        // Wherever this runs, a trash is found for a real path.
        assert!(trash_for(&home_file).is_ok());
    }
}
