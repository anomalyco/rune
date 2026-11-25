use super::{CredentialError, Credentials};

#[test]
fn memory_store_writes_reads_replaces_and_deletes() {
    let store = Credentials::memory();
    assert!(!store.is_persistent());
    assert_eq!(store.read("com.example.api"), Ok(None));

    store.write("com.example.api", "me", b"first").unwrap();
    let found = store.read("com.example.api").unwrap().unwrap();
    assert_eq!((found.account(), found.secret()), ("me", &b"first"[..]));

    store.write("com.example.api", "you", b"second").unwrap();
    let found = store.read("com.example.api").unwrap().unwrap();
    assert_eq!((found.account(), found.secret()), ("you", &b"second"[..]));

    store.delete("com.example.api").unwrap();
    assert_eq!(store.read("com.example.api"), Ok(None));
    store.delete("com.example.api").unwrap();
}

#[test]
fn clones_share_a_store_and_separate_stores_do_not() {
    let store = Credentials::memory();
    store.clone().write("svc", "a", b"x").unwrap();
    assert!(store.read("svc").unwrap().is_some());
    assert_eq!(Credentials::memory().read("svc"), Ok(None));
}

#[test]
fn refuses_names_a_store_cannot_hold() {
    let store = Credentials::memory();
    assert_eq!(store.write("", "a", b"x"), Err(CredentialError::Empty));
    assert_eq!(store.read(""), Err(CredentialError::Empty));
    assert_eq!(store.delete(""), Err(CredentialError::Empty));
    assert_eq!(store.write("s\0", "a", b"x"), Err(CredentialError::Invalid));
    assert_eq!(store.write("s", "a\0", b"x"), Err(CredentialError::Invalid));
}

#[test]
fn debug_never_shows_the_secret() {
    let store = Credentials::memory();
    store.write("svc", "me", b"hunter2").unwrap();
    let shown = format!("{:?}", store.read("svc").unwrap());
    assert!(shown.contains("me") && shown.contains("<redacted>") && !shown.contains("104, 117"), "{shown}");
}

#[test]
fn a_refused_write_keeps_the_old_credential() {
    let store = Credentials::memory();
    store.write("svc", "me", b"old").unwrap();
    assert_eq!(store.write("svc", "bad\0", b"new"), Err(CredentialError::Invalid));
    let found = store.read("svc").unwrap().unwrap();
    assert_eq!((found.account(), found.secret()), ("me", &b"old"[..]));
}

#[test]
fn windows_and_an_apps_own_runtimes_keep_the_system_store() {
    use crate::{AppSpec, Cx, El, Env, Runtime, el};
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(20.0, 20.0) }
    // A thread of its own: the flag is per thread in tests, and other tests make headless runs.
    let noted = std::thread::spawn(|| {
        let spec = AppSpec::new("credentials", (20.0, 20.0), || (), view);
        let _window = crate::tooling::runtime_for(&spec, (), spec.size, Env::new(true, 1.0), std::sync::Arc::new(|| {}));
        let _offscreen = Runtime::new((), view, (20.0, 20.0), Env::new(true, 1.0));
        super::headless_runtime_noted()
    });
    assert!(!noted.join().unwrap());
}

#[test]
fn an_apps_headless_runtime_keeps_the_system_store_in_memory() {
    use crate::{App, Cx, El, Env, el};
    fn view(_: &(), _: &mut Cx<()>) -> El { el().size(20.0, 20.0) }
    let spec = crate::AppSpec::new("credentials", (20.0, 20.0), || (), view);
    let _rt = App::new(spec).headless_runtime((20.0, 20.0), Env::new(true, 1.0));
    assert!(super::headless_runtime_noted());
}
