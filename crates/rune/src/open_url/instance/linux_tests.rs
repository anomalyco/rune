use super::*;

#[test]
fn a_macos_socket_fits_its_path_in_tmp() {
    let id = "com.example.a-long-app-name-0123456789abcdef";
    let path = socket_path(id).expect("a socket path");
    // SAFETY: getuid has no preconditions and can't fail.
    #[allow(unsafe_code)]
    let uid = unsafe { libc::getuid() };
    assert_eq!(path.parent(), Some(std::path::Path::new(&format!("/tmp/rune-{uid}"))));
    assert!(path.as_os_str().len() <= PATH_MAX && PATH_MAX < 104, "sun_path holds 104 bytes on macOS, its NUL included");
    let meta = std::fs::metadata(path.parent().unwrap()).unwrap();
    assert_eq!(meta.permissions().mode() & 0o077, 0, "only the user can reach it");
}
