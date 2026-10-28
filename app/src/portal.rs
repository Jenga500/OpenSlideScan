//! Native folder selection through the XDG desktop portal (FileChooser). This gives the
//! desktop's own dialog (GNOME, KDE, …) both inside the Flatpak sandbox and natively.

use std::path::{Path, PathBuf};

use ashpd::desktop::file_chooser::SelectedFiles;
use ashpd::desktop::ResponseError;

pub enum Pick {
    Selected(PathBuf),
    Cancelled,
    /// No portal available; the caller should fall back to Qt's own dialog.
    Unavailable(String),
}

/// Decodes a `file://` URI (percent-encoded) into a path.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(Ok(v)) = rest.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16)) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

/// Shows the desktop's folder chooser (blocks until the user closes it).
pub fn pick_folder(title: &str, current: &Path) -> Pick {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => return Pick::Unavailable(e.to_string()),
    };
    rt.block_on(async {
        let base = || SelectedFiles::open_file().title(title).directory(true).modal(true);
        // start in the configured folder, or its nearest existing parent if not created yet
        let start = current.ancestors().find(|p| p.is_dir() && p != &Path::new("/"));
        let req = match start {
            Some(dir) => base().current_folder::<&Path>(Some(dir)).unwrap_or_else(|_| base()),
            None => base(),
        };
        let request = match req.send().await {
            Ok(r) => r,
            Err(e) => return Pick::Unavailable(e.to_string()),
        };
        match request.response() {
            Ok(files) => match files.uris().first().and_then(|u| uri_to_path(u.as_str())) {
                Some(p) => Pick::Selected(p),
                None => Pick::Cancelled,
            },
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => Pick::Cancelled,
            Err(e) => Pick::Unavailable(e.to_string()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_uris() {
        assert_eq!(uri_to_path("file:///home/a%20b/Bilder%C3%A4").unwrap(), PathBuf::from("/home/a b/Bilderä"));
        assert_eq!(uri_to_path("file:///tmp/x").unwrap(), PathBuf::from("/tmp/x"));
        assert!(uri_to_path("https://x").is_none());
    }
}
