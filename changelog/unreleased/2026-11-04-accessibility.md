- **Every window and overlay is accessible.** Rune exposes its elements to UI Automation,
  NSAccessibility and AT-SPI through AccessKit (default feature `accessibility`): roles, names
  from labels or text, keys as automation ids, values, focus, checked, expanded, selected and
  disabled states, text fields with caret, selection and character geometry, inline links,
  scroll viewports, modal dialogs, live regions and virtual lists' set sizes. Assistive actions
  run the same handlers as pointer and keyboard input. Nothing is built until a client asks.
  New `El` builders: `description`, `a11y_value`, `range_value`, `range_step`, `checked`,
  `checked_mixed`, `disabled`, `read_only`, `invalid`, `sorted`, `a11y_hidden`, `live_region`
  (`LiveRegion`), `position_in_set`, `shortcut_hint` and `active_descendant`. On Windows an
  update that adds, removes or moves nodes also raises `EVENT_OBJECT_REORDER` or
  `EVENT_OBJECT_LOCATIONCHANGE`. `--tree` writes the tree under `"accessibility"`;
  `Runtime::accessibility_json` and `Runtime::accessibility_tree` return it. README,
  "Accessibility".
