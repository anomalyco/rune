use super::object_path;

#[test]
fn the_object_path_follows_the_app_id() {
    assert_eq!(object_path("dev.example.App").as_deref(), Some("/dev/example/App"));
    assert_eq!(object_path("dev.my-app.Viewer").as_deref(), Some("/dev/my_app/Viewer"));
    for bad in ["", "app", ".dev.app", "dev..app", "dev.1app", "dev.app/x", "dev.app.", "dev.äpp"] {
        assert_eq!(object_path(bad), None, "{bad:?}");
    }
}

#[test]
fn an_open_restarts_the_app_with_its_uris_so_they_sort_the_same() {
    let uris: Vec<String> = ["myapp://open?id=1", "file:///tmp/a%20b.torrent", "file://elsewhere/b", "not a uri"].map(str::to_owned).into();
    let args = super::restart_arguments(&uris);
    assert_eq!(args, ["myapp://open?id=1", "file:///tmp/a%20b.torrent"], "file URIs, not plain paths");
    let again = crate::open_url::instance::sort_arguments(&args);
    assert_eq!(again, crate::open_url::instance::sort_arguments(&uris), "the new process gets the same links and files");
    assert_eq!(again.1, [std::path::PathBuf::from("/tmp/a b.torrent")]);
}
