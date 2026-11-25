use super::{is_link, opens};

fn schemes(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_ascii_lowercase()).collect()
}

#[test]
fn a_link_has_a_scheme_and_a_path_or_a_flag_is_none() {
    assert!(is_link("myapp://open?id=1"));
    assert!(is_link("http://localhost:4096"));
    assert!(!is_link(r"C:\Users\me\notes.txt"));
    assert!(!is_link("--server"));
    assert!(!is_link("--server=http://localhost:4096"));
    assert!(!is_link("://nothing"));
}

#[test]
fn every_link_opens_until_the_app_names_its_schemes() {
    assert!(opens("myapp://open?id=1", None));
    assert!(opens("http://localhost:4096", None));
    assert!(!opens("--server", None));
}

#[test]
fn a_flags_url_value_is_not_a_link_once_the_app_names_its_schemes() {
    let own = schemes(&["myapp"]);
    let launch = ["--server", "http://localhost:4096", "myapp://open?id=1", "--verbose"];
    let links: Vec<&str> = launch.iter().copied().filter(|a| opens(a, Some(&own))).collect();
    assert_eq!(links, ["myapp://open?id=1"]);
}

#[test]
fn a_launch_with_only_a_flags_url_has_no_links_so_it_reopens_the_app() {
    let own = schemes(&["myapp"]);
    assert!(!["--server", "http://localhost:4096"].iter().any(|a| opens(a, Some(&own))));
}

#[test]
fn the_apps_schemes_match_whole_and_without_case() {
    let own = schemes(&["MyApp", "myapp-dev"]);
    assert!(opens("myapp://a", Some(&own)));
    assert!(opens("MYAPP://a", Some(&own)));
    assert!(opens("myapp-dev://a", Some(&own)));
    assert!(!opens("myapp-beta://a", Some(&own)));
    assert!(!opens("my://a", Some(&own)));
    assert!(!opens("https://example.com/?next=myapp://a", Some(&own)));
}

#[test]
fn an_app_that_names_no_scheme_opens_no_link() {
    assert!(!opens("myapp://a", Some(&[])));
}
