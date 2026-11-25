use super::*;

#[test]
fn the_message_names_the_core_pointer_and_the_root_pixel_as_gtk_does() {
    assert_eq!(show_window_menu_data(640, 32), [CORE_POINTER, 640, 32, 0, 0]);
}

#[test]
fn a_point_left_of_or_above_the_root_keeps_its_twos_complement_words() {
    assert_eq!(show_window_menu_data(-5, -1), [CORE_POINTER, (-5i32) as u32, u32::MAX, 0, 0]);
}

#[test]
fn the_menu_is_shown_only_where_the_window_manager_advertises_it() {
    assert!(advertised(&[301, 412, 77], 412));
    assert!(!advertised(&[301, 77], 412));
    assert!(!advertised(&[], 412));
}

#[test]
fn an_atom_the_server_never_interned_is_never_advertised() {
    assert!(!advertised(&[0, 301], 0));
}
