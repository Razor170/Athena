//! Opening things outside Athena: a folder in Explorer or a web page in the
//! default browser. Windows starts them as their own programs, so these
//! return at once. Elsewhere (the Linux sandbox) they report that they
//! only work on Windows.

use std::path::Path;

/// Opens a folder in Explorer.
pub fn open_folder(folder: &Path) -> Result<(), String> {
    open(folder.as_os_str()).map_err(|e| format!("Can't open {}: {e}", folder.display()))
}

/// Opens an https link in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    // Only web links: Explorer would also run other kinds of paths.
    if !url.starts_with("https://") {
        return Err(format!("Not a web link: {url}"));
    }
    open(std::ffi::OsStr::new(url)).map_err(|e| format!("Can't open {url}: {e}"))
}

/// Opens an address on Athena's own local login server
/// (`http://127.0.0.1:<port>/...`) in the default browser.
pub fn open_local_url(url: &str) -> Result<(), String> {
    // Only our own local server, never anything else over plain http.
    if !url.starts_with("http://127.0.0.1:") {
        return Err(format!("Not a local Athena link: {url}"));
    }
    open(std::ffi::OsStr::new(url)).map_err(|e| format!("Can't open {url}: {e}"))
}

#[cfg(windows)]
fn open(target: &std::ffi::OsStr) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn open(_target: &std::ffi::OsStr) -> Result<(), String> {
    Err("this works on Windows only".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_links_open() {
        assert!(open_url("file:///C:/Windows").is_err());
        assert!(open_url("C:\\Windows\\notepad.exe").is_err());
        assert!(open_local_url("http://example.com/").is_err());
    }
}
