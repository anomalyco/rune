//! The module's imports: whether the UTF-8 character at `ptr` (`len` bytes) may start or
//! continue an identifier, as JavaScript's `\p{ID_Start}` and `[\u200C\u200D\p{ID_Continue}]`
//! answer in Highlights' own host.

/// `ID_Start` characters that `XID_Start` (what `unicode-ident` knows) leaves out.
const ID_START_ONLY: &[u32] = &[
    0x037a, 0x0e33, 0x0eb3, 0x309b, 0x309c, 0xfc5e, 0xfc5f, 0xfc60, 0xfc61, 0xfc62, 0xfc63, 0xfdfa, 0xfdfb, 0xfe70,
    0xfe72, 0xfe74, 0xfe76, 0xfe78, 0xfe7a, 0xfe7c, 0xfe7e, 0xff9e, 0xff9f,
];

/// `ID_Continue` characters that `XID_Continue` leaves out.
const ID_CONTINUE_ONLY: &[u32] = &[
    0x037a, 0x309b, 0x309c, 0xfc5e, 0xfc5f, 0xfc60, 0xfc61, 0xfc62, 0xfc63, 0xfdfa, 0xfdfb, 0xfe70, 0xfe72, 0xfe74,
    0xfe76, 0xfe78, 0xfe7a, 0xfe7c, 0xfe7e,
];

pub fn is_id_start(mem: &mut [u8], ptr: i32, len: i32) -> i32 {
    char_at(mem, ptr, len).is_some_and(|c| unicode_ident::is_xid_start(c) || ID_START_ONLY.contains(&(c as u32))) as i32
}

pub fn is_id_continue(mem: &mut [u8], ptr: i32, len: i32) -> i32 {
    char_at(mem, ptr, len)
        .is_some_and(|c| matches!(c, '\u{200c}' | '\u{200d}') || unicode_ident::is_xid_continue(c) || ID_CONTINUE_ONLY.contains(&(c as u32)))
        as i32
}

fn char_at(mem: &[u8], ptr: i32, len: i32) -> Option<char> {
    let at = ptr as u32 as usize;
    std::str::from_utf8(mem.get(at..at + len as u32 as usize)?).ok()?.chars().next()
}
