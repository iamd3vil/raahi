//! Local file responses. Capability-relative opens confine symlinks to the site
//! root, including during concurrent filesystem changes. No extra listener.

use std::io::{self, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use cap_std::fs::{Dir, OpenOptions};
use http::{HeaderMap, Method, StatusCode};
use percent_encoding::percent_decode_str;
use raahi_core::Service;
use tokio::fs::File;

pub(crate) struct FileResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub file: Option<File>,
    pub remaining: u64,
}

impl FileResponse {
    fn empty(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            file: None,
            remaining: 0,
        }
    }
}

/// Do blocking filesystem opens on Tokio's blocking pool. The returned file is
/// already open and confined; streaming never reopens a user-controlled path.
pub(crate) async fn respond(
    service: &Service,
    method: Method,
    path: String,
    headers: HeaderMap,
) -> FileResponse {
    if method != Method::GET && method != Method::HEAD {
        let mut response = FileResponse::empty(StatusCode::METHOD_NOT_ALLOWED);
        response
            .headers
            .insert("allow", "GET, HEAD".parse().unwrap());
        return response;
    }
    let root = service.root.clone().unwrap_or_default();
    let spa = service.spa_fallback;
    match tokio::task::spawn_blocking(move || open_response(&root, spa, &method, &path, &headers))
        .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            let status = match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => StatusCode::NOT_FOUND,
                io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
                _ => {
                    tracing::warn!(%error, "static file open failed");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
            FileResponse::empty(status)
        }
        Err(error) => {
            tracing::error!(%error, "static file task failed");
            FileResponse::empty(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn relative_path(path: &str) -> Option<PathBuf> {
    // Decode exactly once. Reject malformed escapes instead of accepting a
    // literal '%' that another layer might interpret differently. A remaining
    // encoded sequence is just a filename; no filesystem operation decodes it.
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            for _ in 0..2 {
                if !bytes.next()?.is_ascii_hexdigit() {
                    return None;
                }
            }
        }
    }
    let decoded = percent_decode_str(path).decode_utf8().ok()?;
    if decoded.contains('\\') || decoded.chars().any(char::is_control) {
        return None;
    }
    let relative = Path::new(decoded.trim_start_matches('/'));
    if relative.components().any(|part| match part {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        // Reject dot segments rather than normalizing them into another file.
        _ => true,
    }) || decoded.split('/').any(|part| part == "." || part == "..")
    {
        return None;
    }
    Some(relative.to_path_buf())
}

fn open_response(
    root: &str,
    spa: bool,
    method: &Method,
    path: &str,
    request: &HeaderMap,
) -> io::Result<FileResponse> {
    let Some(mut relative) = relative_path(path) else {
        return Ok(FileResponse::empty(StatusCode::NOT_FOUND));
    };
    // SECURITY: this is the only ambient open, and root comes from trusted
    // administrator configuration, never the request. Every metadata lookup,
    // index open and SPA fallback below must stay relative to this SAME Dir.
    // Do not replace these with canonicalize/check-prefix/std::fs::open: that
    // separates validation from opening and introduces symlink-swap races.
    let dir = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    let is_dir = relative.as_os_str().is_empty()
        || match dir.metadata(&relative) {
            Ok(metadata) => {
                if !metadata.is_file() && !metadata.is_dir() {
                    return Ok(FileResponse::empty(StatusCode::NOT_FOUND));
                }
                metadata.is_dir()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
    if is_dir {
        if !path.ends_with('/') {
            // The caller translates this location back to the public route path.
            let mut response = FileResponse::empty(StatusCode::PERMANENT_REDIRECT);
            response.headers.insert(
                "location",
                format!("{path}/").parse().map_err(io::Error::other)?,
            );
            return Ok(response);
        }
        relative.push("index.html");
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        // Opening a FIFO must not pin a blocking worker waiting for a writer.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = match open_regular_file(&dir, &relative, &options) {
        Ok(file) => file,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && spa && relative.extension().is_none() =>
        {
            relative = PathBuf::from("index.html");
            open_regular_file(&dir, &relative, &options)?
        }
        Err(error) => return Err(error),
    };
    let Some(mut file) = file else {
        return Ok(FileResponse::empty(StatusCode::NOT_FOUND));
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(FileResponse::empty(StatusCode::NOT_FOUND));
    }
    let size = metadata.len();
    let modified = metadata.modified().ok().map(|time| time.into_std());
    // A weak validator: metadata cannot prove byte-for-byte identity.
    let etag = modified
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|time| {
            format!(
                "W/\"{size:x}-{:x}-{:x}\"",
                time.as_secs(),
                time.subsec_nanos()
            )
        });
    let mut response = FileResponse::empty(StatusCode::OK);
    response.headers.insert(
        "content-type",
        mime_guess::from_path(&relative)
            .first_or_octet_stream()
            .as_ref()
            .parse()
            .unwrap(),
    );
    response
        .headers
        .insert("accept-ranges", "bytes".parse().unwrap());
    response
        .headers
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    if let Some(time) = modified {
        response.headers.insert(
            "last-modified",
            httpdate::fmt_http_date(time).parse().unwrap(),
        );
    }
    if let Some(tag) = &etag {
        response.headers.insert("etag", tag.parse().unwrap());
    }

    let precondition_failed = if let Some(condition) = request.get("if-match") {
        // We only emit weak validators, so only the wildcard can strongly match.
        condition.to_str().ok() != Some("*")
    } else {
        request
            .get("if-unmodified-since")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| httpdate::parse_http_date(v).ok())
            .zip(modified)
            .is_some_and(|(since, modified)| seconds(modified) > seconds(since))
    };
    if precondition_failed {
        response.status = StatusCode::PRECONDITION_FAILED;
        response
            .headers
            .insert("content-length", "0".parse().unwrap());
        return Ok(response);
    }

    let not_modified = if let Some(condition) = request.get("if-none-match") {
        condition.to_str().ok().is_some_and(|value| {
            value.split(',').any(|tag| {
                let tag = tag.trim();
                tag == "*"
                    || etag.as_ref().is_some_and(|etag| {
                        tag.trim_start_matches("W/") == etag.trim_start_matches("W/")
                    })
            })
        })
    } else {
        request
            .get("if-modified-since")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| httpdate::parse_http_date(v).ok())
            .zip(modified)
            .is_some_and(|(since, modified)| seconds(modified) <= seconds(since))
    };
    if not_modified {
        response.status = StatusCode::NOT_MODIFIED;
        return Ok(response);
    }

    let mut start = 0;
    let mut length = size;
    // RFC 9110: Range applies only to GET. Weak ETags cannot satisfy If-Range.
    let range_allowed = request.get("if-range").is_none_or(|value| {
        value
            .to_str()
            .ok()
            .and_then(|v| httpdate::parse_http_date(v).ok())
            .zip(modified)
            .is_some_and(|(date, modified)| seconds(date) == seconds(modified))
    });
    if method == Method::GET
        && range_allowed
        && let Some(range) = request.get("range").and_then(|value| value.to_str().ok())
    {
        match byte_range(range, size) {
            Range::Slice(first, last) => {
                start = first;
                length = last - first + 1;
                response.status = StatusCode::PARTIAL_CONTENT;
                response.headers.insert(
                    "content-range",
                    format!("bytes {first}-{last}/{size}").parse().unwrap(),
                );
            }
            Range::Unsatisfiable => {
                response.status = StatusCode::RANGE_NOT_SATISFIABLE;
                response
                    .headers
                    .insert("content-range", format!("bytes */{size}").parse().unwrap());
                response
                    .headers
                    .insert("content-length", "0".parse().unwrap());
                return Ok(response);
            }
            Range::Ignore => {}
        }
    }
    response
        .headers
        .insert("content-length", length.to_string().parse().unwrap());
    if method == Method::GET && length != 0 {
        file.seek(SeekFrom::Start(start))?;
        response.file = Some(File::from_std(file.into_std()));
        response.remaining = length;
    }
    Ok(response)
}

/// Avoid opening known devices, sockets and FIFOs at all, including implicit
/// index and fallback paths. The post-open metadata check remains necessary for
/// local replacement races; only trusted users may modify the site tree.
fn open_regular_file(
    dir: &Dir,
    path: &Path,
    options: &OpenOptions,
) -> io::Result<Option<cap_std::fs::File>> {
    if !dir.metadata(path)?.is_file() {
        return Ok(None);
    }
    dir.open_with(path, options).map(Some)
}

fn seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs())
}

#[derive(Debug, PartialEq)]
enum Range {
    Slice(u64, u64),
    Unsatisfiable,
    Ignore,
}

fn byte_range(value: &str, size: u64) -> Range {
    let Some(value) = value.strip_prefix("bytes=") else {
        return Range::Ignore;
    };
    // Multi-range requests are permitted to receive the complete representation.
    if value.contains(',') {
        return Range::Ignore;
    }
    let Some((first, last)) = value.trim().split_once('-') else {
        return Range::Ignore;
    };
    if first.is_empty() {
        let Ok(suffix) = last.parse::<u64>() else {
            return Range::Ignore;
        };
        if suffix == 0 || size == 0 {
            return Range::Unsatisfiable;
        }
        return Range::Slice(size.saturating_sub(suffix), size - 1);
    }
    let Ok(first) = first.parse::<u64>() else {
        return Range::Ignore;
    };
    let last = if last.is_empty() {
        size.saturating_sub(1)
    } else {
        let Ok(last) = last.parse::<u64>() else {
            return Range::Ignore;
        };
        if last < first {
            return Range::Ignore;
        }
        last.min(size.saturating_sub(1))
    };
    if first >= size {
        Range::Unsatisfiable
    } else {
        Range::Slice(first, last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct Site(PathBuf);
    impl Site {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("raahi-static-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(path.join("nested")).unwrap();
            std::fs::write(path.join("index.html"), "<h1>home</h1>").unwrap();
            std::fs::write(path.join("app.js"), "0123456789").unwrap();
            std::fs::write(path.join("nested/index.html"), "nested").unwrap();
            std::fs::create_dir(path.join("empty")).unwrap();
            Self(path)
        }
        fn get(
            &self,
            path: &str,
            spa: bool,
            method: Method,
            headers: HeaderMap,
        ) -> io::Result<FileResponse> {
            open_response(self.0.to_str().unwrap(), spa, &method, path, &headers)
        }
    }
    impl Drop for Site {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn rejects_dotfiles_encoded_traversal_and_backslashes() {
        for path in [
            "/../secret",
            "/a/./b",
            "/%2e%2e/secret",
            "/.env",
            "/a/%2Egit/config",
            "/a%5cb",
            "/%00",
            "/%ff",
            "/%",
            "/%2",
            "/%GG",
            "/%0a",
            "/%7f",
            "/%c0%ae%c0%ae/secret",
            "/%e0%80%afsecret",
            "/..%2fsecret",
            "/nested%2f..%2f..%2fsecret",
            "/nested%5c..%5csecret",
            "/C:%5cWindows%5cwin.ini",
            "/%5c%5cserver%5cshare",
        ] {
            assert!(relative_path(path).is_none(), "{path}");
        }
        assert_eq!(
            relative_path("/hello%20world.txt").unwrap(),
            PathBuf::from("hello world.txt")
        );
        assert_eq!(relative_path("/").unwrap(), PathBuf::new());
    }

    #[test]
    fn all_mixed_encodings_of_parent_segments_are_rejected() {
        for first in [".", "%2e", "%2E"] {
            for second in [".", "%2e", "%2E"] {
                for slash in ["/", "%2f", "%2F"] {
                    for prefix in ["/", "/nested/", "//nested//"] {
                        let path = format!("{prefix}{first}{second}{slash}secret.txt");
                        assert!(relative_path(&path).is_none(), "{path}");
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn double_encoding_is_a_literal_filename_not_a_second_traversal() {
        let site = Site::new();
        let name = "%2e%2e%2fsecret.txt";
        std::fs::write(site.0.join(name), "literal filename inside root").unwrap();
        let mut response = site
            .get(
                "/%252e%252e%252fsecret.txt",
                false,
                Method::GET,
                HeaderMap::new(),
            )
            .unwrap();
        let mut body = String::new();
        response
            .file
            .take()
            .unwrap()
            .into_std()
            .await
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "literal filename inside root");
    }

    #[tokio::test]
    async fn invalid_paths_do_not_trigger_spa_fallback_or_conditional_responses() {
        let site = Site::new();
        let mut headers = HeaderMap::new();
        headers.insert("range", "bytes=0-4".parse().unwrap());
        headers.insert("if-none-match", "*".parse().unwrap());
        for method in [Method::GET, Method::HEAD] {
            for path in [
                "/../index.html",
                "/nested/%2e%2e/index.html",
                "/%2eenv",
                "/%00",
                "/%0d%0a",
                "/%",
                "/%GG",
            ] {
                let response = site
                    .get(path, true, method.clone(), headers.clone())
                    .unwrap();
                assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
                assert!(response.file.is_none());
                assert_eq!(response.remaining, 0);
            }
        }
    }

    #[test]
    fn ranges_cover_suffix_open_ended_invalid_and_empty_files() {
        assert_eq!(byte_range("bytes=2-4", 10), Range::Slice(2, 4));
        assert_eq!(byte_range("bytes=2-", 10), Range::Slice(2, 9));
        assert_eq!(byte_range("bytes=-3", 10), Range::Slice(7, 9));
        assert_eq!(byte_range("bytes=-20", 10), Range::Slice(0, 9));
        assert_eq!(byte_range("bytes=0-100", 10), Range::Slice(0, 9));
        assert_eq!(byte_range("bytes=10-", 10), Range::Unsatisfiable);
        assert_eq!(byte_range("bytes=0-", 0), Range::Unsatisfiable);
        assert_eq!(byte_range("bytes=-0", 10), Range::Unsatisfiable);
        assert_eq!(byte_range("bytes=4-2", 10), Range::Ignore);
        assert_eq!(byte_range("bytes=0-1,4-5", 10), Range::Ignore);
    }

    #[tokio::test]
    async fn index_mime_head_redirect_spa_and_missing_assets() {
        let site = Site::new();
        let mut index = site.get("/", false, Method::GET, HeaderMap::new()).unwrap();
        assert_eq!(index.status, StatusCode::OK);
        assert_eq!(index.headers["content-type"], "text/html");
        assert_eq!(index.headers["content-length"], "13");
        let mut body = String::new();
        index
            .file
            .take()
            .unwrap()
            .into_std()
            .await
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "<h1>home</h1>");
        let head = site
            .get("/app.js", false, Method::HEAD, HeaderMap::new())
            .unwrap();
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(head.headers["content-length"], "10");
        assert!(head.file.is_none());
        let redirect = site
            .get("/nested", false, Method::GET, HeaderMap::new())
            .unwrap();
        assert_eq!(redirect.status, StatusCode::PERMANENT_REDIRECT);
        assert_eq!(redirect.headers["location"], "/nested/");
        assert_eq!(
            site.get("/nested/", false, Method::GET, HeaderMap::new())
                .unwrap()
                .status,
            StatusCode::OK
        );
        assert_eq!(
            site.get("/client/route", true, Method::GET, HeaderMap::new())
                .unwrap()
                .status,
            StatusCode::OK
        );
        for path in ["/missing.js", "/missing.css", "/empty/"] {
            assert_eq!(
                site.get(path, true, Method::GET, HeaderMap::new())
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::NotFound
            );
        }
        assert!(
            site.get("/missing", false, Method::GET, HeaderMap::new())
                .is_err()
        );
    }

    #[tokio::test]
    async fn validators_ranges_and_preconditions() {
        let site = Site::new();
        let original = site
            .get("/app.js", false, Method::GET, HeaderMap::new())
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("if-none-match", original.headers["etag"].clone());
        let cached = site
            .get("/app.js", false, Method::GET, headers.clone())
            .unwrap();
        assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
        assert!(cached.file.is_none());
        headers.clear();
        headers.insert(
            "if-modified-since",
            original.headers["last-modified"].clone(),
        );
        assert_eq!(
            site.get("/app.js", false, Method::HEAD, headers.clone())
                .unwrap()
                .status,
            StatusCode::NOT_MODIFIED
        );
        headers.insert("if-none-match", "\"not-this-file\"".parse().unwrap());
        assert_eq!(
            site.get("/app.js", false, Method::GET, headers.clone())
                .unwrap()
                .status,
            StatusCode::OK
        );
        headers.clear();
        headers.insert("range", "bytes=2-4".parse().unwrap());
        let mut partial = site
            .get("/app.js", false, Method::GET, headers.clone())
            .unwrap();
        assert_eq!(partial.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers["content-range"], "bytes 2-4/10");
        assert_eq!(partial.remaining, 3);
        let mut bytes = [0; 3];
        partial
            .file
            .take()
            .unwrap()
            .into_std()
            .await
            .read_exact(&mut bytes)
            .unwrap();
        assert_eq!(&bytes, b"234");
        assert_eq!(
            site.get("/app.js", false, Method::HEAD, headers.clone())
                .unwrap()
                .status,
            StatusCode::OK
        );
        headers.insert("if-range", original.headers["etag"].clone());
        assert_eq!(
            site.get("/app.js", false, Method::GET, headers.clone())
                .unwrap()
                .status,
            StatusCode::OK
        );
        headers.remove("if-range");
        headers.insert("range", "bytes=100-".parse().unwrap());
        let invalid = site
            .get("/app.js", false, Method::GET, headers.clone())
            .unwrap();
        assert_eq!(invalid.status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(invalid.headers["content-range"], "bytes */10");
        headers.clear();
        headers.insert("if-match", original.headers["etag"].clone());
        assert_eq!(
            site.get("/app.js", false, Method::GET, headers.clone())
                .unwrap()
                .status,
            StatusCode::PRECONDITION_FAILED
        );
        headers.insert("if-match", "*".parse().unwrap());
        assert_eq!(
            site.get("/app.js", false, Method::GET, headers)
                .unwrap()
                .status,
            StatusCode::OK
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_cannot_escape_root_but_internal_links_work() {
        let site = Site::new();
        std::os::unix::fs::symlink("app.js", site.0.join("linked.js")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", site.0.join("external.txt")).unwrap();
        std::os::unix::fs::symlink("..", site.0.join("parent")).unwrap();
        assert_eq!(
            site.get("/linked.js", false, Method::GET, HeaderMap::new())
                .unwrap()
                .status,
            StatusCode::OK
        );
        assert_eq!(
            site.get("/external.txt", true, Method::GET, HeaderMap::new())
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(
            site.get("/parent/secret", true, Method::GET, HeaderMap::new())
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_regular_files_and_implicit_indexes_are_not_opened() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::net::UnixListener;
        let site = Site::new();
        let socket = UnixListener::bind(site.0.join("socket")).unwrap();
        for name in ["fifo", "nested/index.html", "index.html"] {
            let path = site.0.join(name);
            if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
            let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: path is a valid NUL-terminated string for our test fixture.
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        }
        for path in ["/fifo", "/socket", "/nested/", "/", "/missing-route"] {
            let response = site.get(path, true, Method::GET, HeaderMap::new()).unwrap();
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
            assert!(response.file.is_none());
        }
        // If running privileged, also exercise an actual harmless device node.
        let path = std::ffi::CString::new(site.0.join("device").as_os_str().as_bytes()).unwrap();
        // SAFETY: valid fixture path; create a /dev/null-style device, never open it.
        let created =
            unsafe { libc::mknod(path.as_ptr(), libc::S_IFCHR | 0o600, libc::makedev(1, 3)) };
        if created == 0 {
            assert_eq!(
                site.get("/device", true, Method::GET, HeaderMap::new())
                    .unwrap()
                    .status,
                StatusCode::NOT_FOUND
            );
        } else {
            assert_eq!(
                io::Error::last_os_error().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        drop(socket);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn escaping_link_chains_directories_indexes_and_spa_fallback_are_denied() {
        use std::os::unix::fs::symlink;

        let site = Site::new();
        // A shared path prefix must not make a sibling directory look confined.
        let outside = Site(PathBuf::from(format!("{}-outside", site.0.display())));
        std::fs::create_dir(&outside.0).unwrap();
        std::fs::write(outside.0.join("secret.txt"), "outside secret").unwrap();
        let sibling = outside.0.file_name().unwrap().to_str().unwrap();
        symlink(
            format!("../{sibling}/secret.txt"),
            site.0.join("relative.txt"),
        )
        .unwrap();
        symlink(outside.0.join("secret.txt"), site.0.join("absolute.txt")).unwrap();
        symlink(&outside.0, site.0.join("directory")).unwrap();
        symlink(
            format!("../../{sibling}/secret.txt"),
            site.0.join("nested/second"),
        )
        .unwrap();
        symlink("nested/second", site.0.join("chain.txt")).unwrap();
        symlink("cycle-b", site.0.join("cycle-a")).unwrap();
        symlink("cycle-a", site.0.join("cycle-b")).unwrap();

        for spa in [false, true] {
            for method in [Method::GET, Method::HEAD] {
                for path in [
                    "/relative.txt",
                    "/absolute.txt",
                    "/directory/secret.txt",
                    "/chain.txt",
                    "/cycle-a",
                ] {
                    assert!(
                        site.get(path, spa, method.clone(), HeaderMap::new())
                            .is_err(),
                        "{path}, spa={spa}"
                    );
                }
            }
        }

        // The auto-selected index and SPA fallback must use the confined open
        // too, not an unrestricted open of a supposedly trusted index path.
        std::fs::remove_file(site.0.join("nested/index.html")).unwrap();
        symlink(
            outside.0.join("secret.txt"),
            site.0.join("nested/index.html"),
        )
        .unwrap();
        assert!(
            site.get("/nested/", true, Method::GET, HeaderMap::new())
                .is_err()
        );
        std::fs::remove_file(site.0.join("index.html")).unwrap();
        symlink(outside.0.join("secret.txt"), site.0.join("index.html")).unwrap();
        for path in ["/", "/missing-client-route"] {
            assert!(
                site.get(path, true, Method::GET, HeaderMap::new()).is_err(),
                "{path}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn proc_magic_links_cannot_escape_root() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::symlink;

        let site = Site::new();
        let outside = Site::new();
        let secret = std::fs::File::open(outside.0.join("app.js")).unwrap();
        symlink(
            format!("/proc/self/fd/{}", secret.as_raw_fd()),
            site.0.join("magic.txt"),
        )
        .unwrap();
        assert!(
            site.get("/magic.txt", true, Method::GET, HeaderMap::new())
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_symlink_swaps_never_serve_outside_bytes() {
        use std::os::unix::fs::symlink;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        let site = Site::new();
        let outside = Site::new();
        std::fs::write(outside.0.join("app.js"), "OUTSIDE SECRET").unwrap();
        symlink("app.js", site.0.join("race.txt")).unwrap();
        symlink("nested", site.0.join("race-dir")).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let swaps = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let root = site.0.clone();
        let outside_root = outside.0.clone();
        let writer = {
            let stop = stop.clone();
            let swaps = swaps.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                while !stop.load(Ordering::Relaxed) {
                    for escape in [true, false] {
                        let file_target = if escape {
                            PathBuf::from("..")
                                .join(outside_root.file_name().unwrap())
                                .join("app.js")
                        } else {
                            PathBuf::from("app.js")
                        };
                        let dir_target = if escape {
                            PathBuf::from("..").join(outside_root.file_name().unwrap())
                        } else {
                            PathBuf::from("nested")
                        };
                        for (name, target) in [("race.txt", file_target), ("race-dir", dir_target)]
                        {
                            let next = root.join("next-link");
                            symlink(target, &next).unwrap();
                            std::fs::rename(&next, root.join(name)).unwrap();
                        }
                        swaps.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        };
        barrier.wait();
        let mut served = 0;
        let mut denied = 0;
        // Join the writer even if an assertion fails, before fixture cleanup.
        let result = std::panic::AssertUnwindSafe(async {
            for _ in 0..1000 {
                for (path, allowed) in [
                    ("/race.txt", "0123456789"),
                    ("/race-dir/index.html", "nested"),
                ] {
                    match site.get(path, true, Method::GET, HeaderMap::new()) {
                        Ok(mut response) => {
                            assert_eq!(response.status, StatusCode::OK);
                            let mut body = String::new();
                            response
                                .file
                                .take()
                                .unwrap()
                                .into_std()
                                .await
                                .read_to_string(&mut body)
                                .unwrap();
                            assert_eq!(body, allowed, "outside bytes served through {path}");
                            served += 1;
                        }
                        Err(_) => denied += 1,
                    }
                }
            }
        });
        use futures::FutureExt;
        let result = result.catch_unwind().await;
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        assert!(swaps.load(Ordering::Relaxed) > 0);
        assert!(served > 0, "test never opened an allowed target");
        assert!(denied > 0, "test never observed an escaping target");
    }
}
