use std::path::PathBuf;
use std::time::Duration;

use super::handoff::{KeptToken, TOKEN_ARG, launch, reopens};

#[test]
fn a_launch_token_leaves_the_arguments() {
    let got = launch(vec![format!("{TOKEN_ARG}compositor-token"), "myapp://open?id=1".into(), "--flag".into()]);
    assert_eq!(got.token.as_deref(), Some("compositor-token"));
    assert_eq!(got.args, ["myapp://open?id=1", "--flag"], "a relaunch doesn't pass the token on");
    assert_eq!(got.links, ["myapp://open?id=1"]);
    assert!(got.files.is_empty());

    let file = launch(vec!["file:///tmp/a.torrent".into()]);
    assert_eq!((file.links.len(), file.files.as_slice()), (0, &[PathBuf::from("/tmp/a.torrent")][..]), "a later launch's %U file reaches on_open_files");
    assert!(!reopens(&file.links, &file.files));

    let empty = launch(vec![TOKEN_ARG.to_owned()]);
    assert_eq!((empty.token, empty.args.len()), (None, 0), "an empty token is none");
}

#[test]
fn a_kept_token_is_used_once_and_lapses() {
    let now = crate::time::now();
    let mut kept = KeptToken::default();
    kept.keep(Some(String::new()), now);
    assert!(!kept.waiting(now), "an empty token isn't kept");

    kept.keep(Some("t1".into()), now);
    assert!(kept.waiting(now + Duration::from_secs(19)));
    assert_eq!(kept.take(now + Duration::from_secs(1)).as_deref(), Some("t1"));
    assert_eq!(kept.take(now + Duration::from_secs(1)), None, "used once");

    kept.keep(Some("t2".into()), now);
    assert!(!kept.waiting(now + Duration::from_secs(20)), "lapsed");
    assert_eq!(kept.take(now + Duration::from_secs(20)), None);
}

#[test]
fn files_alone_dont_reopen() {
    let files = [PathBuf::from("/tmp/a.torrent")];
    assert!(!reopens(&[], &files), "files go to on_open_files, not a reopen");
    assert!(!reopens(&["myapp://open".to_owned()], &[]), "links go to on_open_url");
    assert!(reopens(&[], &[]), "a launch with neither reopens");
}
