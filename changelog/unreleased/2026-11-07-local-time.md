- **`LocalTime` reads an instant off the user's wall clock on every platform.**
  `LocalTime::at(epoch_ms)` gives the year, month, day, hour, minute and second the OS time zone
  shows for an instant, and its offset from UTC then, daylight saving included: `localtime_r` on
  Unix, the dynamic time zone on Windows (so a past or future instant gets its own year's rules),
  and `Date` on the web. Instants the OS can't place, such as one before 1601 on Windows or past
  `Date`'s range, read as UTC instead of overflowing. `LocalTime::at_offset` fixes the offset for
  stable test and headless output, so apps stop hand-rolling libc and Win32 time calls.
