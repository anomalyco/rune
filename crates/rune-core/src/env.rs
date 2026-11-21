//! Process environment access. Returns `None`/`false` on `wasm32`, where there is no process
//! environment, and keeps native `std::env` lookups in one place.

/// Reads the UTF-8 environment variable `name`, or `None` if unset, non-UTF-8, or on `wasm32`.
pub fn var(name: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        #[allow(clippy::disallowed_methods)]
        std::env::var(name).ok()
    }
}

/// Reads the OS-string environment variable `name`, or `None` if unset or on `wasm32`.
pub fn var_os(name: &str) -> Option<std::ffi::OsString> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var_os(name)
    }
}

/// Returns `true` when `name` is set to `"1"`, `"true"`, `"TRUE"`, or `"yes"`.
pub fn flag(name: &str) -> bool {
    matches!(var(name).as_deref(), Some("1" | "true" | "TRUE" | "yes"))
}
