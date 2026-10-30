- **A launcher can wait for readiness.** With `RUNE_READY_DIR` set, the process writes
  `$RUNE_READY_DIR/<bundle id>.<pid>` right after its first `on_ready` returns (the main bundle's
  identifier on macOS, else the executable's name), as one line of JSON:
  `{"pid":123,"bundle_id":"com.example.app","ready_ms":412,"rune":"0.1.0"}`, `ready_ms` counted
  from the process's start. It makes the directory 0700 if missing, writes through a renamed
  temporary file, and removes the file when the process exits normally; one left by a killed
  process names a PID that isn't running. Without the variable nothing is written.
