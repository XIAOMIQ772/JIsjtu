use std::path::{Path, PathBuf};

pub(super) fn directory() -> PathBuf {
    #[cfg(windows)]
    if let Some(folder) = windows_downloads_folder() {
        return folder;
    }
    std::env::home_dir()
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(windows)]
fn windows_downloads_folder() -> Option<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr};
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath},
    };

    let mut raw = ptr::null_mut();
    // The Shell allocates a NUL-terminated UTF-16 path. Copy it before freeing it.
    unsafe {
        let status = SHGetKnownFolderPath(&FOLDERID_Downloads, 0, ptr::null_mut(), &mut raw);
        let folder = (status >= 0 && !raw.is_null()).then(|| {
            let mut len = 0;
            while *raw.add(len) != 0 {
                len += 1;
            }
            PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(raw, len)))
        });
        CoTaskMemFree(raw.cast());
        folder
    }
}

pub(super) fn file_name(name: &str, fallback: String) -> String {
    let leaf = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    #[cfg(windows)]
    let leaf = windows_file_name(leaf);
    if leaf.is_empty() {
        fallback
    } else {
        leaf.to_owned()
    }
}

#[cfg(any(windows, test))]
fn windows_file_name(name: &str) -> String {
    let cleaned = name.replace(
        |ch: char| ch.is_control() || r#"<>:"/\|?*"#.contains(ch),
        "_",
    );
    let name = cleaned.trim_end_matches(['.', ' ']);
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let numbered_device = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    if numbered_device
        || matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
    {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_attachment_names_are_valid_local_files() {
        for (input, expected) in [
            ("实验:1?.pdf ", "实验_1_.pdf"),
            ("con.txt", "_con.txt"),
            ("LPT² .pdf", "_LPT² .pdf"),
            ("COM12.txt", "COM12.txt"),
            ("... ", ""),
            ("讲义 & 附录.pdf", "讲义 & 附录.pdf"),
        ] {
            assert_eq!(windows_file_name(input), expected);
        }
    }
}
