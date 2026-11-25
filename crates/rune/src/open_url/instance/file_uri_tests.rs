use std::path::PathBuf;

use super::file_path;
use crate::open_url::instance::sort_with;

#[test]
fn file_uris_become_local_paths() {
    assert_eq!(file_path("file:///home/me/a%20b.txt"), Some(PathBuf::from("/home/me/a b.txt")));
    assert_eq!(file_path("FILE://localhost/tmp/x"), Some(PathBuf::from("/tmp/x")));
    assert_eq!(file_path("file:///tmp/x?query#frag"), Some(PathBuf::from("/tmp/x")));
    assert_eq!(file_path("file://other-host/tmp/x"), None, "another host");
    assert_eq!(file_path("file:///tmp/%zz"), None, "bad escape");
    assert_eq!(file_path("file:///tmp/%00"), None, "NUL");
    assert_eq!(file_path("myapp://open"), None);
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn launch_arguments_and_open_uris_sort_alike() {
    // A desktop entry's `Exec=myapp %U`, a later launch's socket message and `Open` all come here.
    let launch = args(&["myapp://open?id=1", "file:///tmp/a.torrent", "file://elsewhere/b", "/tmp/plain", "--flag", "https://example.com/"]);
    let (links, files) = sort_with(&launch, None);
    assert_eq!(links, ["myapp://open?id=1", "https://example.com/"], "no file: URI is a link");
    assert_eq!(files, [PathBuf::from("/tmp/a.torrent")], "a plain path stays an argument");
}

#[test]
fn a_file_uri_is_a_file_whatever_schemes_the_app_named() {
    let own = vec!["myapp".to_owned()];
    let (links, files) = sort_with(&args(&["file:///tmp/a.torrent", "https://example.com/", "myapp://x"]), Some(&own));
    assert_eq!(links, ["myapp://x"]);
    assert_eq!(files, [PathBuf::from("/tmp/a.torrent")], "so it doesn't make the launch a reopen");
}
