//! Files dropped on an element from another app: `El::on_drop_files`, acceptance hooks,
//! hover/move/reject callbacks, passive ancestor observers, drop operations and native drop
//! descriptions, `Tx::drop_point` and `Tx::drag_paths`. `runtime/input/drop.rs` routes the drag.
use std::rc::Rc;

use crate::drag_out::{DragOperation, DragOperations};
use crate::el::{El, PointerEvent, Tx};
use crate::states::{StatesRef, downcast};

/// The icon shown beside a native drop description label (Windows `DROPIMAGETYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DropDescriptionKind {
    /// "Cannot drop" icon (`DROPIMAGE_NONE`).
    None,
    /// Copy badge (`DROPIMAGE_COPY`).
    Copy,
    /// Move badge (`DROPIMAGE_MOVE`).
    Move,
    /// Link / shortcut badge (`DROPIMAGE_LINK`).
    Link,
    /// Plain text label without an action badge (`DROPIMAGE_LABEL`).
    Label,
    /// Warning icon (`DROPIMAGE_WARNING`).
    Warning,
}

/// The label the shell draws beneath the drag image while hovering a drop target on Windows
/// (`CFSTR_DROPDESCRIPTION` / `DROPDESCRIPTION`).
///
/// `message` may include `%1`, which Windows replaces with `insert` in emphasis (for example
/// `"Copy to %1"` with insert `"Documents"`). Platforms without native drop descriptions ignore it.
///
/// ```
/// # use rune::{DropDescription, DropDescriptionKind};
/// let desc = DropDescription::copy_to("Documents");
/// assert_eq!(desc.kind(), DropDescriptionKind::Copy);
/// assert_eq!(desc.message(), "Copy to %1");
/// assert_eq!(desc.insert_text(), "Documents");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropDescription {
    kind: DropDescriptionKind,
    message: String,
    insert: String,
}

impl DropDescription {
    /// A drop description with `kind` and `message` (no `%1` insert).
    pub fn new(kind: DropDescriptionKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), insert: String::new() }
    }

    /// Sets the `%1` insert text highlighted by the shell inside `message`.
    pub fn insert(mut self, insert: impl Into<String>) -> Self {
        self.insert = insert.into();
        self
    }

    /// `"Copy to %1"` with `target` as the highlighted insert and a copy badge.
    pub fn copy_to(target: impl Into<String>) -> Self {
        Self::new(DropDescriptionKind::Copy, "Copy to %1").insert(target)
    }

    /// `"Move to %1"` with `target` as the highlighted insert and a move badge.
    pub fn move_to(target: impl Into<String>) -> Self {
        Self::new(DropDescriptionKind::Move, "Move to %1").insert(target)
    }

    /// `"Create link in %1"` with `target` as the highlighted insert and a link badge.
    pub fn link_in(target: impl Into<String>) -> Self {
        Self::new(DropDescriptionKind::Link, "Create link in %1").insert(target)
    }

    /// A "cannot drop" description (`DropDescriptionKind::None`).
    pub fn none(message: impl Into<String>) -> Self {
        Self::new(DropDescriptionKind::None, message)
    }

    /// A plain text label without an action badge (`DropDescriptionKind::Label`).
    pub fn label(message: impl Into<String>) -> Self {
        Self::new(DropDescriptionKind::Label, message)
    }

    /// A warning description (`DropDescriptionKind::Warning`).
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(DropDescriptionKind::Warning, message)
    }

    pub fn kind(&self) -> DropDescriptionKind { self.kind }
    pub fn message(&self) -> &str { &self.message }
    pub fn insert_text(&self) -> &str { &self.insert }
}

/// How a drop target answers a hovering file drag: the chosen [`DragOperation`] (or `None` to
/// refuse the drop) and an optional native [`DropDescription`] drawn under Windows' drag image.
///
/// ```
/// # use rune::{DragOperation, DropDescription, DropFeedback};
/// let fb = DropFeedback::from(DragOperation::Move)
///     .description(DropDescription::move_to("Archive"));
/// assert_eq!(fb.operation(), Some(DragOperation::Move));
/// assert_eq!(fb.drop_description().map(DropDescription::insert_text), Some("Archive"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropFeedback {
    operation: Option<DragOperation>,
    description: Option<DropDescription>,
}

impl DropFeedback {
    /// Feedback with an explicit `operation` (or `None` to refuse the drop) and no description.
    pub fn new(operation: Option<DragOperation>) -> Self {
        Self { operation, description: None }
    }

    /// Refuse the drop (`DROPEFFECT_NONE`).
    pub fn none() -> Self { Self::new(None) }
    /// Accept the drop as a copy (`DROPEFFECT_COPY`).
    pub fn copy() -> Self { Self::new(Some(DragOperation::Copy)) }
    /// Accept the drop as a move (`DROPEFFECT_MOVE`).
    pub fn move_files() -> Self { Self::new(Some(DragOperation::Move)) }
    /// Accept the drop as a link (`DROPEFFECT_LINK`).
    pub fn link() -> Self { Self::new(Some(DragOperation::Link)) }

    /// Sets or clears the chosen operation.
    pub fn with_operation(mut self, operation: Option<DragOperation>) -> Self {
        self.operation = operation;
        self
    }

    /// Attaches a native [`DropDescription`] drawn under the drag image on Windows.
    pub fn description(mut self, description: DropDescription) -> Self {
        self.description = Some(description);
        self
    }

    /// Attaches or clears the native [`DropDescription`].
    pub fn with_description(mut self, description: Option<DropDescription>) -> Self {
        self.description = description;
        self
    }

    pub fn operation(&self) -> Option<DragOperation> { self.operation }
    pub fn drop_description(&self) -> Option<&DropDescription> { self.description.as_ref() }
    pub(crate) fn constrain(mut self, allowed: DragOperations) -> Self {
        self.operation = self.operation.filter(|&op| allowed.contains(op));
        self
    }
}

impl From<DragOperation> for DropFeedback {
    fn from(op: DragOperation) -> Self { Self::new(Some(op)) }
}

impl From<Option<DragOperation>> for DropFeedback {
    fn from(op: Option<DragOperation>) -> Self { Self::new(op) }
}

/// Active file-drag details passed to [`El::drop_feedback`].
#[derive(Clone, Copy, Debug)]
pub struct DropInfo<'a> {
    pub(crate) paths: &'a [std::path::PathBuf],
    pub(crate) point: Option<PointerEvent>,
    pub(crate) mods: crate::Mods,
    pub(crate) allowed: DragOperations,
}

impl<'a> DropInfo<'a> {
    /// All paths carried by the active file drag.
    pub fn paths(&self) -> &'a [std::path::PathBuf] { self.paths }
    /// Where the drag is on this element, in its own points and window points.
    pub fn point(&self) -> Option<PointerEvent> { self.point }
    /// Modifier keys currently held by the user during the drag (e.g. Shift to move, Ctrl to copy).
    pub fn mods(&self) -> crate::Mods { self.mods }
    /// Operations the drag source allows (`Copy`, `Move`, `Link`).
    pub fn allowed(&self) -> DragOperations { self.allowed }
    /// Standard modifier-based choice among [`Self::allowed`] (`Shift` -> Move, `Ctrl` -> Copy,
    /// `Ctrl+Shift`/`Alt` -> Link, otherwise Copy/Move/Link).
    pub fn default_operation(&self) -> Option<DragOperation> { self.allowed.resolve(self.mods) }
}

pub(crate) type DropFilesFn = Rc<dyn Fn(crate::states::States<'_>, &[std::path::PathBuf], &mut Tx)>;

/// [`El::on_drop_files`], [`El::accepts_files`], [`El::accepts_drag`], [`El::drop_feedback`],
/// [`El::on_drop_hover`], [`El::on_drop_move`], [`El::on_drop_rejected`],
/// [`El::on_drop_rejected_hover`], [`El::on_drop_hover_within`] and [`El::on_drop_move_within`].
#[derive(Clone, Default)]
pub(crate) struct DropSpec {
    pub on_drop: Option<DropFilesFn>,
    pub accepts: Option<AcceptsFn>,
    pub accepts_drag: Option<AcceptsDragFn>,
    pub feedback: Option<DropFeedbackFn>,
    pub on_hover: Option<DropHoverFn>,
    pub on_reject: Option<DropFilesFn>,
    pub on_reject_hover: Option<DropHoverFn>,
    pub on_move: Option<DropMoveFn>,
    pub on_hover_within: Option<DropHoverFn>,
    pub on_move_within: Option<DropMoveFn>,
}
pub(crate) type AcceptsFn = Rc<dyn Fn(&std::path::Path) -> bool>;
pub(crate) type AcceptsDragFn = Rc<dyn Fn(StatesRef<'_>, &[std::path::PathBuf]) -> bool>;
pub(crate) type DropFeedbackFn = Rc<dyn Fn(StatesRef<'_>, &DropInfo<'_>) -> DropFeedback>;
pub(crate) type DropHoverFn = Rc<dyn Fn(crate::states::States<'_>, bool, &mut Tx)>;
pub(crate) type DropMoveFn = Rc<dyn Fn(crate::states::States<'_>, PointerEvent, &mut Tx)>;

impl DropSpec {
    /// The files of `paths` this element takes, when it's a drop target, and the rest. When
    /// `states` is `Some`, evaluates [`El::accepts_drag`] over the full drag slice first; if it
    /// returns `false`, all `paths` are rejected.
    pub(crate) fn split(
        &self,
        states: Option<StatesRef<'_>>,
        paths: &[std::path::PathBuf],
    ) -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) {
        if paths.is_empty() { return (Vec::new(), Vec::new()); }
        if self.on_drop.is_none() { return (Vec::new(), paths.to_vec()); }
        if let Some(accepts_drag) = &self.accepts_drag
            && let Some(states) = states
            && !accepts_drag(states, paths)
        {
            return (Vec::new(), paths.to_vec());
        }
        paths.iter().cloned().partition(|p| self.accepts.as_ref().is_none_or(|f| f(p)))
    }
    /// It stops a drag it takes none of, rather than passing it to the target around it.
    pub(crate) fn claims_rejects(&self) -> bool { self.on_reject.is_some() || self.on_reject_hover.is_some() }
    /// It passively observes file drags crossing its subtree.
    pub(crate) fn observes_within(&self) -> bool { self.on_hover_within.is_some() || self.on_move_within.is_some() }
    /// Its hover callback for a drag it takes (`accepted`) or rejects.
    pub(crate) fn hover_fn(&self, accepted: bool) -> Option<DropHoverFn> {
        if accepted { self.on_hover.clone() } else { self.on_reject_hover.clone() }
    }
}

impl El {
    /// Take files dropped on this element from another app (Finder, Explorer, a file manager).
    /// The innermost target under the drop that accepts any of the files gets the ones it
    /// accepts, all in one call; files it doesn't accept are left out. A drop no target takes
    /// is ignored: put `on_drop_files` on the root as well to catch drops anywhere. While a modal
    /// is open, its root stands in for the window's, and a drop outside it reaches nothing. A drop
    /// target takes no pointer events, so one laid over other elements leaves their clicks,
    /// context menus and help.
    ///
    /// ```
    /// # use rune::el::el;
    /// # use std::path::PathBuf;
    /// # struct App { attachments: Vec<PathBuf>, over: bool }
    /// # let s = App { attachments: Vec::new(), over: false };
    /// el().fill(if s.over { rune::Color::WHITE.alpha(0.1) } else { rune::Color::TRANSPARENT })
    ///     .on_drop_files(|s: &mut App, files, _| s.attachments.extend_from_slice(files))
    ///     .accepts_extensions(&["png", "jpg", "m4a"])
    ///     .on_drop_hover(|s: &mut App, over, _| s.over = over);
    /// ```
    ///
    /// [`Tx::drop_point`] says where on the element the files landed, to insert them there.
    ///
    /// macOS, Windows, Wayland and X11; the web build has no paths to give. Headless scripts
    /// drop files with `drop_files` events.
    pub fn on_drop_files<S: 'static>(mut self, f: impl Fn(&mut S, &[std::path::PathBuf], &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_drop = Some(Rc::new(move |s, files, tx| f(downcast::<S>(s), files, tx)));
        self
    }
    /// Only the files `accepts` returns true for reach this element's [`El::on_drop_files`]. A
    /// drag carrying none of them passes this element over: the next accepting ancestor takes
    /// the drop and shows the drop hover.
    pub fn accepts_files(mut self, accepts: impl Fn(&std::path::Path) -> bool + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().accepts = Some(Rc::new(accepts));
        self
    }
    /// [`El::accepts_files`] for files with one of these extensions, compared without case.
    pub fn accepts_extensions(self, extensions: &[&str]) -> Self {
        let extensions: Vec<String> = extensions.iter().map(|e| e.trim_start_matches('.').to_ascii_lowercase()).collect();
        self.accepts_files(move |path| path.extension().and_then(|e| e.to_str())
            .is_some_and(|e| extensions.iter().any(|x| x.eq_ignore_ascii_case(e))))
    }
    /// Decide whether this element takes a file drag as a whole, with read access to app state
    /// `&S` and every dragged path together. When `accepts` returns `false`, the whole drag is
    /// refused (passed to the next accepting ancestor, or stopped by [`El::on_drop_rejected`] /
    /// [`El::on_drop_rejected_hover`]). When paired with [`El::accepts_files`] or
    /// [`El::accepts_extensions`], `accepts_drag` runs first over all paths and the per-file
    /// filter partitions what remains.
    ///
    /// ```
    /// # use rune::el::el;
    /// # use std::path::PathBuf;
    /// # struct App { read_only: bool, dir: PathBuf }
    /// el().on_drop_files(|_: &mut App, _, _| {})
    ///     .accepts_drag(|s: &App, paths| !s.read_only && !paths.iter().any(|p| p == &s.dir));
    /// ```
    pub fn accepts_drag<S: 'static>(
        mut self,
        accepts: impl Fn(&S, &[std::path::PathBuf]) -> bool + 'static,
    ) -> Self {
        self.handlers.drop.get_or_insert_default().accepts_drag = Some(Rc::new(move |s, paths| {
            accepts(s.cast::<S>().expect("handler registered for a different state type than the app's"), paths)
        }));
        self
    }
    /// Called with `true` when a file drag over the window makes this element the drop target
    /// (the innermost [`El::on_drop_files`] under the pointer that accepts the files), and with
    /// `false` when the drag moves off it, leaves the window, or drops, or when the element goes
    /// away under the drag. Draw a drop highlight from it. With `true`, [`Tx::drop_point`] says
    /// where the drag came in.
    pub fn on_drop_hover<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_hover = Some(Rc::new(move |s, over, tx| f(downcast::<S>(s), over, tx)));
        self
    }
    /// Follow a file drag this element takes ([`El::on_drop_files`]) while it's over it: `f`
    /// hears the drag's point on the element, in the element's own points as
    /// [`El::on_pointer_move_local`] gives them, when it becomes the target and then as the drag
    /// moves, at most once a frame. Draw a drop caret from it, cleared by
    /// [`El::on_drop_hover`]'s `false`, and insert where [`Tx::drop_point`] says on the drop.
    /// Only an element with this asks the window for frames as the drag moves over it, and a
    /// drag the platform gives no position for is never heard here. Scripts move a drag with
    /// further `drag_files` events.
    pub fn on_drop_move<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_move = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }
    /// Hear the files dropped on this element that it doesn't accept ([`El::accepts_files`]),
    /// to say why ("that isn't an export"). With this, or [`El::on_drop_rejected_hover`], the
    /// element stops a drag carrying none of the files it takes instead of passing it to the
    /// target around it, and a drop of only those files comes here. On a drop of some files it
    /// takes and some it doesn't, [`El::on_drop_files`] gets the first and this the rest.
    ///
    /// ```
    /// # use rune::el::el;
    /// # struct App { error: Option<String>, wrong: bool }
    /// el().on_drop_files(|s: &mut App, _, _| s.error = None)
    ///     .accepts_extensions(&["json"])
    ///     .on_drop_rejected(|s: &mut App, files, _| s.error = Some(format!("{} isn't an export", files[0].display())))
    ///     .on_drop_rejected_hover(|s: &mut App, over, _| s.wrong = over);
    /// ```
    pub fn on_drop_rejected<S: 'static>(mut self, f: impl Fn(&mut S, &[std::path::PathBuf], &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_reject = Some(Rc::new(move |s, files, tx| f(downcast::<S>(s), files, tx)));
        self
    }
    /// [`El::on_drop_hover`] for a drag this element stops but takes none of (see
    /// [`El::on_drop_rejected`]): `true` when the drag reaches it, `false` when it moves off,
    /// leaves the window or drops, or when the element goes away under it. Draw a "can't drop
    /// here" state from it.
    pub fn on_drop_rejected_hover<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_reject_hover = Some(Rc::new(move |s, over, tx| f(downcast::<S>(s), over, tx)));
        self
    }
    /// Passively observe when a file drag enters (`true`) or leaves (`false`) this element or
    /// any descendant along the hit path, alongside the innermost drop target without stealing
    /// the drop. Fires innermost first across ancestors (matching DOM `dragenter`/`dragleave`
    /// bubbling), and hears `false` when the drag moves outside the subtree, leaves the window,
    /// drops, or when the element unmounts. With `true`, [`Tx::drop_point`] and
    /// [`Tx::drag_paths`] give the drag's point on this element and the dragged files.
    ///
    /// ```
    /// # use rune::el::el;
    /// # struct App { strip_over: bool }
    /// el().on_drop_hover_within(|s: &mut App, over, _| s.strip_over = over);
    /// ```
    pub fn on_drop_hover_within<S: 'static>(mut self, f: impl Fn(&mut S, bool, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_hover_within = Some(Rc::new(move |s, over, tx| f(downcast::<S>(s), over, tx)));
        self
    }
    /// Passively follow a file drag's position while it moves anywhere over this element or its
    /// descendants, alongside the innermost drop target without stealing the drop. `f` hears the
    /// drag's point in this element's own points (and [`Tx::drop_point`] / [`Tx::drag_paths`]),
    /// at most once a frame, innermost ancestor first. Only an element with this (or
    /// [`El::on_drop_move`]) asks for frames as the drag moves across it; a drag without a
    /// platform position is never heard here.
    ///
    /// ```
    /// # use rune::el::el;
    /// # struct App { drag_x: Option<f32> }
    /// el().on_drop_move_within(|s: &mut App, p, _| s.drag_x = Some(p.x));
    /// ```
    pub fn on_drop_move_within<S: 'static>(mut self, f: impl Fn(&mut S, PointerEvent, &mut Tx) + 'static) -> Self {
        self.handlers.drop.get_or_insert_default().on_move_within = Some(Rc::new(move |s, p, tx| f(downcast::<S>(s), p, tx)));
        self
    }
    /// Choose the drop operation (`Copy`, `Move`, `Link`, or `None` to refuse) and optional
    /// native [`DropDescription`] while a file drag hovers this element, with read access to app
    /// state `&S` and [`DropInfo`] (dragged paths, local pointer position, modifier keys, and
    /// source-allowed operations).
    ///
    /// Re-runs whenever the drag moves over this element, modifier keys change, or the view
    /// commits. The chosen operation is constrained to [`DropInfo::allowed`] and reported in
    /// [`Tx::drop_operation`] during hover, move, and [`El::on_drop_files`]. Without this (and
    /// without calling [`Tx::set_drop_operation`] from hover/move handlers), accepting targets
    /// default to [`DragOperation::Copy`] when the source allows it.
    ///
    /// ```
    /// # use rune::{DragOperation, DropDescription, DropFeedback, el};
    /// # struct App { folder: String }
    /// el().on_drop_files(|_: &mut App, _, _| {})
    ///     .drop_feedback(|s: &App, info| {
    ///         let Some(op) = info.default_operation() else { return DropFeedback::none() };
    ///         let desc = match op {
    ///             DragOperation::Move => DropDescription::move_to(&s.folder),
    ///             DragOperation::Link => DropDescription::link_in(&s.folder),
    ///             _ => DropDescription::copy_to(&s.folder),
    ///         };
    ///         DropFeedback::from(op).description(desc)
    ///     });
    /// ```
    pub fn drop_feedback<S: 'static>(
        mut self,
        feedback: impl Fn(&S, &DropInfo<'_>) -> DropFeedback + 'static,
    ) -> Self {
        self.handlers.drop.get_or_insert_default().feedback = Some(Rc::new(move |s, info| {
            feedback(s.cast::<S>().expect("handler registered for a different state type than the app's"), info)
        }));
        self
    }
}

impl Tx {
    /// In [`El::on_drop_files`], [`El::on_drop_rejected`], [`El::on_drop_move`],
    /// [`El::on_drop_move_within`], and [`El::on_drop_hover`] / [`El::on_drop_rejected_hover`] /
    /// [`El::on_drop_hover_within`]'s `true`: where the file drag is on the element, in its own
    /// points (`x`, `y`, following its scroll and transforms) and in the window's. `None` when
    /// the platform gave no position and in every other handler.
    pub fn drop_point(&self) -> Option<PointerEvent> { self.drop_point }

    /// In [`El::on_drop_files`], [`El::on_drop_rejected`], [`El::on_drop_move`],
    /// [`El::on_drop_move_within`], and [`El::on_drop_hover`] / [`El::on_drop_rejected_hover`] /
    /// [`El::on_drop_hover_within`]'s `true`: all paths carried by the active file drag. `None`
    /// when a hover ends (`false`) and in every other handler.
    ///
    /// ```
    /// # use rune::el::el;
    /// # struct App { count: usize }
    /// el().on_drop_hover(|s: &mut App, over, tx| {
    ///     s.count = if over { tx.drag_paths().map_or(0, |p| p.len()) } else { 0 };
    /// });
    /// ```
    pub fn drag_paths(&self) -> Option<&[std::path::PathBuf]> { self.drag_paths.as_deref() }

    /// In file-drop callbacks (`on_drop_files`, `on_drop_rejected`, `on_drop_move`,
    /// `on_drop_move_within`, and `true` calls of `on_drop_hover` / `on_drop_rejected_hover` /
    /// `on_drop_hover_within`): the modifier keys held during the OS drag (such as `Shift` to
    /// move or `Ctrl` to copy on Windows). Defaults to empty elsewhere.
    pub fn drag_mods(&self) -> crate::Mods { self.drag_mods }

    /// In file-drop callbacks: the set of [`DragOperation`]s allowed by the drag source
    /// (`Copy`, `Move`, `Link`). Defaults to empty outside file-drop callbacks.
    pub fn drag_operations(&self) -> DragOperations { self.drag_operations }

    /// In file-drop callbacks: the chosen [`DragOperation`] for the active drag or completed drop
    /// (for example `Some(DragOperation::Move)` when Shift-dropping onto a target that accepts
    /// moves). `None` when no operation is selected or outside file-drop callbacks.
    pub fn drop_operation(&self) -> Option<DragOperation> { self.drop_operation }

    /// In [`El::on_drop_hover`] (`true`), [`El::on_drop_move`], or
    /// [`El::on_drop_rejected_hover`] (`true`): set the [`DragOperation`] this target offers
    /// to the OS (`Copy`, `Move`, `Link`, or `None` to refuse the drop). Constrained to
    /// [`Tx::drag_operations`] and cleared when the drag leaves this target.
    pub fn set_drop_operation(&mut self, operation: Option<DragOperation>) {
        self.drop_op_set = Some(operation);
    }

    /// In [`El::on_drop_hover`] (`true`), [`El::on_drop_move`], or
    /// [`El::on_drop_rejected_hover`] (`true`): set or clear the native [`DropDescription`]
    /// shown beneath the drag image on Windows (for example `"Copy to Documents"`). Cleared
    /// automatically when the drag leaves this target.
    pub fn set_drop_description(&mut self, description: Option<DropDescription>) {
        self.drop_desc_set = Some(description);
    }
}
