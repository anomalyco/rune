# rune-notify

System notifications for Rune apps: `UNUserNotificationCenter` on macOS, toast notifications on
Windows. Off those, `available()` is false and every call does nothing. Notifications are silent.

| Call | What it does |
| --- | --- |
| `register(&Identity::new(id, name).icon(path))` | Windows: gives the process its `AppUserModelID` (which also groups its windows on the taskbar) and registers the name and icon its toasts show, under `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\<id>`. Call it first thing in `main`, before a window opens. macOS reads both from the bundle. |
| `available()` | Whether the app can post: a bundled app on macOS, a registered one on Windows. |
| `post(&Notification)`, `remove(ids)` | Posts a notification with an id, title, body and string user info, or takes it back out of the system's list. On macOS the first post asks for permission. |
| `on_click(f)` | Calls `f` with the clicked notification's id and user info, from any thread. Install it at launch. On Windows a click reaches the process that posted the toast while it runs. |
| `app_focused()`, `bring_to_front()` | Whether one of the app's windows is in front (skip notifying then), and bringing the frontmost window forward and focusing it, restoring it if minimized, for a click. |

```rust
fn main() {
    let _ = rune_notify::register(&rune_notify::Identity::new("com.example.app", "Example").icon(icon_png));
    rune_notify::on_click(|click| open(click.get("session")));
    // Later, while the window isn't focused:
    rune_notify::post(&rune_notify::Notification::new("n-1", "Response ready").body("Fix the build").info("session", "ses_1"));
}
```

`rune_mac::notify` re-exports this crate.
