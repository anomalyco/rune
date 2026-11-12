//! Name order as a person reads it, in the interface's language (`set_language`): the language's
//! alphabet (German ä beside a, Swedish ä after z), case and width aside, and runs of digits by
//! their value, so `file2` comes before `file10`. The OS's collator does it with its own data:
//! Windows' `CompareStringEx` (`SORT_DIGITSASNUMBERS`), macOS's
//! `CFStringCompareWithOptionsAndLocale` (`kCFCompareNumerically`, as Finder sorts). Elsewhere
//! (Linux, the web) a natural order by case-folded characters and digit runs stands in. No
//! collation data of Rune's own.

use std::cmp::Ordering;

/// `a` against `b` as names sort for a person in the interface's language. Names the collator
/// calls equal fall back to code points, so the order is total and stable.
pub fn compare_names(a: &str, b: &str) -> Ordering {
    collate(a, b).then_with(|| a.cmp(b))
}

#[cfg(windows)]
fn collate(a: &str, b: &str) -> Ordering {
    use std::cell::RefCell;

    use windows_sys::Win32::Globalization::{CompareStringEx, LINGUISTIC_IGNORECASE, NORM_LINGUISTIC_CASING, SORT_DIGITSASNUMBERS};

    thread_local! {
        /// The locale name (null-terminated UTF-16) for the language epoch it was made in, and
        /// the two strings' buffers, kept so a sort allocates nothing per comparison.
        static STATE: RefCell<(u64, Vec<u16>, Vec<u16>, Vec<u16>)> = const { RefCell::new((u64::MAX, Vec::new(), Vec::new(), Vec::new())) };
    }
    STATE.with_borrow_mut(|(epoch, locale, x, y)| {
        let now = super::language::epoch();
        if *epoch != now {
            *epoch = now;
            locale.clear();
            if let Some(tag) = super::language::chosen_tag() {
                locale.extend(tag.encode_utf16());
                locale.push(0);
            }
        }
        x.clear();
        x.extend(a.encode_utf16());
        y.clear();
        y.extend(b.encode_utf16());
        // A null locale name is the user's default.
        let name = if locale.is_empty() { std::ptr::null() } else { locale.as_ptr() };
        // SAFETY: both buffers live through the call, with the lengths given; the locale name is
        // null-terminated or null.
        let result = unsafe {
            CompareStringEx(name, LINGUISTIC_IGNORECASE | NORM_LINGUISTIC_CASING | SORT_DIGITSASNUMBERS, x.as_ptr(), x.len() as i32, y.as_ptr(), y.len() as i32, std::ptr::null(), std::ptr::null(), 0)
        };
        match result {
            1 => Ordering::Less,
            2 => Ordering::Equal,
            3 => Ordering::Greater,
            _ => natural(a, b),
        }
    })
}

#[cfg(target_os = "macos")]
fn collate(a: &str, b: &str) -> Ordering {
    use std::cell::RefCell;

    use objc2_core_foundation::{CFLocale, CFRange, CFRetained, CFString, CFStringCompareFlags};

    thread_local! {
        static LOCALE: RefCell<(u64, Option<CFRetained<CFLocale>>)> = const { RefCell::new((u64::MAX, None)) };
    }
    let (x, y) = (CFString::from_str(a), CFString::from_str(b));
    let flags = CFStringCompareFlags::CompareCaseInsensitive | CFStringCompareFlags::CompareNumerically | CFStringCompareFlags::CompareLocalized | CFStringCompareFlags::CompareWidthInsensitive;
    LOCALE.with_borrow_mut(|(epoch, locale)| {
        let now = super::language::epoch();
        if *epoch != now {
            *epoch = now;
            *locale = super::language::chosen_tag().and_then(|tag| CFLocale::new(None, Some(&CFString::from_str(&tag))));
        }
        // SAFETY: both strings are live CFStrings, the range covers all of the first.
        let result = unsafe { x.compare_with_options_and_locale(Some(&y), CFRange { location: 0, length: x.length() }, flags, locale.as_deref()) };
        result.0.cmp(&0)
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn collate(a: &str, b: &str) -> Ordering {
    natural(a, b)
}

/// Characters case-folded, and each run of ASCII digits by its value (leading zeros aside).
fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s.trim_start_matches('0').to_owned()
                };
                let (m, n) = (run(&mut x), run(&mut y));
                let by_value = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if by_value != Ordering::Equal { return by_value; }
            }
            (Some(c), Some(d)) => {
                let by_char = c.to_lowercase().cmp(d.to_lowercase());
                if by_char != Ordering::Equal { return by_char; }
                x.next();
                y.next();
            }
        }
    }
}
