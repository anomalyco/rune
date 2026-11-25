use super::*;
use crate::app::{AppHandle, Shared};
use crate::el::el;
use crate::runtime::{Cx, Env, Runtime};

fn ready(mut f: RegisterUrlScheme) -> Option<Result<(), UrlSchemeError>> {
    match Pin::new(&mut f).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(r) => Some(r),
        Poll::Pending => None,
    }
}

#[test]
fn only_url_schemes_are_claimed() {
    assert_eq!(check("rork-companion").as_deref(), Ok("rork-companion"));
    assert_eq!(check("MyApp+x.y-1").as_deref(), Ok("myapp+x.y-1"));
    for bad in ["", "1app", "-app", "my app", "myapp:", "myapp://", "my/app", "my_app", "ü", &"a".repeat(MAX_LEN + 1)] {
        assert_eq!(check(bad), Err(UrlSchemeError::Invalid), "{bad:?}");
    }
}

#[test]
fn headless_runs_record_the_scheme_and_claim_nothing() {
    let shared = Arc::new(Shared::default());
    let handle = AppHandle { shared: shared.clone() };
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::default());
    let mut claim = handle.register_url_scheme("Rork-Companion");
    assert_eq!(Pin::new(&mut claim).poll(&mut Context::from_waker(Waker::noop())), Poll::Pending, "answers once the UI thread runs it");
    assert_eq!(ready(handle.register_url_scheme("not a scheme")), Some(Err(UrlSchemeError::Invalid)));
    shared.drain_window(&mut rt, |_| {});
    assert_eq!(ready(claim), Some(Ok(())));
    assert_eq!(rt.take_url_scheme_requests(), ["rork-companion"]);
    assert!(rt.take_url_scheme_requests().is_empty());
}

#[test]
fn a_dropped_request_answers_instead_of_hanging() {
    let shared = Arc::new(Shared::<()>::default());
    let handle = AppHandle { shared: shared.clone() };
    let claim = handle.register_url_scheme("myapp");
    drop(shared.take_updates());
    assert!(matches!(ready(claim), Some(Err(UrlSchemeError::Failed(_)))));
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn a_live_window_elsewhere_answers_unsupported() {
    let shared = Arc::new(Shared::default());
    let handle = AppHandle { shared: shared.clone() };
    let mut rt = Runtime::new((), |_: &(), _: &mut Cx<()>| el(), (100.0, 100.0), Env::default());
    rt.host.open_urls_natively = true;
    let claim = handle.register_url_scheme("myapp");
    shared.drain_window(&mut rt, |_| {});
    assert!(matches!(ready(claim), Some(Err(UrlSchemeError::Unsupported(_)))));
    assert!(rt.take_url_scheme_requests().is_empty());
}
