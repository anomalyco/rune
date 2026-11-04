- **rune-kit components tell screen readers what they are.** Every component reports its role,
  name, value, state and actions: checkboxes, switches, radios, segments and checked menu items
  read checked; accordions, collapsibles, menus, selects, comboboxes, popovers, tree rows,
  sidebar sections and the sidebar toggle read expanded or collapsed and expand by assistive
  action; list, select, table, tree and sidebar rows read selected and their place in the set;
  disabled components read disabled; dialogs are modal and described; toasts and badges are
  polite live regions, field errors assertive ones; progress reads its percentage, a pane's
  grip its width (increment and decrement resize it); Markdown reads as headings, lists, task
  checkboxes, code, quotes and tables, and a diagram as a figure described by its source.
  Decorative icons, dots, rules and the words a control's label already says are hidden. New
  builders: `Select::label`, `Combobox::label`, `SegmentedControl::label`, `TextInput::label`,
  `TextArea::label`, `ShortcutRecorder::label`, `ProgressCircle::label`, `Popover::label` and
  `Collapsible::disabled`, and `TextInput::active_descendant`. A tree, table and combobox
  search field name the row under the cursor as their active descendant, so a screen reader
  follows arrow keys while focus stays put. A table's sorted column header is named by its
  title and reports its sort direction (no longer ", sorted ascending" in the label). A select's or combobox's trigger is now named by its label (else its
  placeholder) with the chosen option as its value, a text field's editor (not its box) carries
  its name, a sidebar section header is named by its title alone, and accordion and collapsible
  triggers and a toast's close button are now Tab stops. `Button::expanded` and
  `IconButton::expanded` mark a menu or popover trigger, open or not.
