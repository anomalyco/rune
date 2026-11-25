use std::path::PathBuf;

#[test]
fn uri_list_round_trips_through_the_drop_parser() {
    let paths = vec![PathBuf::from("/tmp/My Export 录.csv"), PathBuf::from("/home/a/100%#?.txt")];
    let list = super::uri_list(&paths);
    assert_eq!(list, "file:///tmp/My%20Export%20%E5%BD%95.csv\r\nfile:///home/a/100%25%23%3F.txt\r\n");
    assert_eq!(crate::platform::wayland_drop::machine::parse_uri_list(list.as_bytes()), paths);
}
