//! What sort of file something is, at a glance, for its colour: images,
//! video, sound, archives, code, documents, programs — from the name, so
//! it costs nothing to ask for every row on screen.

use enso_core::{Palette, Rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Document,
    Program,
    Other,
}

impl Kind {
    /// The kind of a file called `name` (a folder if `is_dir`).
    pub fn of(name: &str, is_dir: bool, executable: bool) -> Self {
        if is_dir {
            return Self::Folder;
        }
        let lower = name.to_ascii_lowercase();
        let ext = lower.rsplit_once('.').map_or("", |(_, ext)| ext);
        match ext {
            "png" | "jpg" | "jpeg" | "jpe" | "gif" | "webp" | "svg" | "bmp" | "ico" | "avif"
            | "heic" | "heif" | "tif" | "tiff" | "qoi" | "jxl" | "xcf" | "kra" => Self::Image,
            "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "flv" | "wmv" | "mpg" | "mpeg"
            | "ogv" => Self::Video,
            "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wma" | "aiff" | "mid" => {
                Self::Audio
            }
            "zip" | "tar" | "gz" | "tgz" | "xz" | "txz" | "zst" | "bz2" | "7z" | "rar" | "iso"
            | "deb" | "rpm" | "zstd" | "lz4" | "lzma" => Self::Archive,
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cc" | "go"
            | "java" | "kt" | "rb" | "lua" | "sh" | "bash" | "zsh" | "fish" | "toml" | "yaml"
            | "yml" | "json" | "kdl" | "css" | "html" | "xml" | "sql" | "zig" | "nix" | "vim"
            | "conf" | "ini" | "lock" => Self::Code,
            "pdf" | "md" | "txt" | "odt" | "ods" | "odp" | "doc" | "docx" | "xls" | "xlsx"
            | "ppt" | "pptx" | "epub" | "rtf" | "csv" | "tex" | "org" => Self::Document,
            _ if executable => Self::Program,
            _ => match lower.as_str() {
                "makefile" | "dockerfile" | "justfile" | "license" | "readme" => Self::Code,
                _ => Self::Other,
            },
        }
    }

    /// Its colour, from the palette: the same everywhere a file is shown.
    pub fn color(self, palette: &Palette, accent: Rgba) -> Rgba {
        match self {
            Self::Folder => accent,
            Self::Image => palette.iris,
            Self::Video | Self::Program => palette.love,
            Self::Audio => palette.rose,
            Self::Archive => palette.gold,
            Self::Code => palette.foam,
            Self::Document => palette.pine,
            Self::Other => palette.subtle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_names() {
        assert_eq!(Kind::of("Photo.JPG", false, false), Kind::Image);
        assert_eq!(Kind::of("backup.tar.gz", false, false), Kind::Archive);
        assert_eq!(Kind::of("main.rs", false, false), Kind::Code);
        assert_eq!(Kind::of("Makefile", false, false), Kind::Code);
        assert_eq!(Kind::of("notes.md", false, false), Kind::Document);
        assert_eq!(Kind::of("src", true, false), Kind::Folder);
        assert_eq!(Kind::of("enso-files", false, true), Kind::Program);
        assert_eq!(Kind::of("enso-files", false, false), Kind::Other);
        // A script is code, executable or not.
        assert_eq!(Kind::of("install.sh", false, true), Kind::Code);
    }
}
