- **App installs keep the bundle present.** `scripts/mac-app` shares a per-target
  lock with HiveMind, stages each install separately, and atomically exchanges
  bundles. A failed launch or missing Rune ready receipt restores the previous
  bundle; interrupted installs retain both complete bundles for recovery.
