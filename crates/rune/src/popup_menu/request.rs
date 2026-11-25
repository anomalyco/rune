//! In-window fallback pop-up menu request ([`PopupMenuRequest`]) and resolution
//! ([`Tx::resolve_popup_menu`]) for platforms without native pop-up menus (such as Linux).

use std::cell::RefCell;
use std::rc::Rc;

use super::{PopupAnchor, PopupMenu, Show};
use crate::{PointerPress, Tx};

/// A pop-up or context-menu request handed to [`App::on_popup_menu`](crate::App::on_popup_menu)
/// on platforms where [`popup_menu_supported`](crate::popup_menu_supported) is `false` and not on
/// the web (such as Linux).
///
/// Covers every menu that flows through Rune's pop-up queue: app [`Tx::popup_menu`] calls,
/// selectable-text Copy menus, default link/image context menus, and text-field menus when
/// [`App::on_field_menu`](crate::App::on_field_menu) is not registered.
///
/// Inspect [`Self::menu`] with [`PopupMenu::entries`] (or pass the request to
/// `rune_kit::Menu::from_popup_menu`) to draw an in-window menu at [`Self::point`] /
/// [`Self::anchor_rect`], then answer with [`Tx::resolve_popup_menu`](crate::Tx::resolve_popup_menu).
///
/// ```
/// use rune::{App, AppSpec, PopupEntry, PopupMenuRequest, Tx, el};
///
/// struct S { open_menu: Option<PopupMenuRequest> }
/// let _app = App::new(AppSpec::new("demo", (480.0, 320.0), || S { open_menu: None }, |_, _| el()))
///     .on_popup_menu(|s: &mut S, req: PopupMenuRequest, _tx: &mut Tx| {
///         let _entries: &[PopupEntry] = req.menu().entries();
///         s.open_menu = Some(req);
///     });
/// ```
#[derive(Clone)]
pub struct PopupMenuRequest {
    menu: PopupMenu,
    anchor: PopupAnchor,
    point: PointerPress,
    anchor_rect: Option<[f32; 4]>,
    min_width: f32,
    positioned: Option<String>,
    /// Presentation-only open submenu path shared across clones of this open request so
    /// `rune_kit::Menu::from_popup_menu` can open and close nested submenus without requiring
    /// extra fields in app state.
    open_submenu: Rc<RefCell<Vec<usize>>>,
}

impl PartialEq for PopupMenuRequest {
    fn eq(&self, other: &Self) -> bool {
        self.menu == other.menu
            && self.anchor == other.anchor
            && self.point == other.point
            && self.anchor_rect == other.anchor_rect
            && self.min_width == other.min_width
            && self.positioned == other.positioned
    }
}

impl std::fmt::Debug for PopupMenuRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopupMenuRequest")
            .field("menu", &self.menu)
            .field("anchor", &self.anchor)
            .field("point", &self.point)
            .field("anchor_rect", &self.anchor_rect)
            .field("min_width", &self.min_width)
            .field("positioned", &self.positioned)
            .finish()
    }
}

impl PopupMenuRequest {
    /// Builds a [`PopupMenuRequest`] in view points from a validated [`Show`] (which stores
    /// window points = view points × `zoom`).
    pub(crate) fn from_show(show: &Show, anchor: PopupAnchor, zoom: f32) -> Self {
        let inv = if zoom > 0.0 && zoom.is_finite() { 1.0 / zoom } else { 1.0 };
        let point = PointerPress { x: show.at.0 * inv, y: show.at.1 * inv };
        let anchor_rect = show.element.map(|[x, y, w, h]| [x * inv, y * inv, w * inv, h * inv]);
        let min_width = show.min_width * inv;
        Self {
            menu: show.menu.clone(),
            anchor,
            point,
            anchor_rect,
            min_width,
            positioned: show.positioned.clone(),
            open_submenu: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// The menu to display (items, separators, disabled/checked states, shortcuts, and submenus).
    /// Read its entries with [`PopupMenu::entries`].
    ///
    /// ```
    /// # use rune::{PopupMenuRequest, Tx};
    /// fn inspect(req: &PopupMenuRequest, tx: &mut Tx) {
    ///     if req.menu().entries().is_empty() { tx.resolve_popup_menu(None); }
    /// }
    /// ```
    pub fn menu(&self) -> &PopupMenu { &self.menu }

    /// Consumes the request and returns its [`PopupMenu`].
    ///
    /// ```
    /// # use rune::{PopupMenu, PopupMenuRequest};
    /// fn take_menu(req: PopupMenuRequest) -> PopupMenu { req.into_menu() }
    /// ```
    pub fn into_menu(self) -> PopupMenu { self.menu }

    /// The anchor originally passed to [`Tx::popup_menu`] (`PopupAnchor::Key` or
    /// `PopupAnchor::Point`).
    ///
    /// ```
    /// # use rune::{PopupAnchor, PopupMenuRequest};
    /// fn is_key_anchored(req: &PopupMenuRequest) -> bool {
    ///     matches!(req.anchor(), PopupAnchor::Key(_))
    /// }
    /// ```
    pub fn anchor(&self) -> &PopupAnchor { &self.anchor }

    /// Where the menu should open, in view points: the press point for [`PopupAnchor::Point`],
    /// the anchor element's top-left when [`Self::positioned`] is set, or its bottom-left
    /// otherwise.
    ///
    /// ```
    /// # use rune::{PointerPress, PopupMenuRequest};
    /// fn coords(req: &PopupMenuRequest) -> PointerPress { req.point() }
    /// ```
    pub fn point(&self) -> PointerPress { self.point }

    /// The anchor element's drawn rectangle `[x, y, width, height]` in view points when anchored
    /// by [`PopupAnchor::Key`], or `None` for [`PopupAnchor::Point`].
    ///
    /// ```
    /// # use rune::PopupMenuRequest;
    /// fn trigger_rect(req: &PopupMenuRequest) -> Option<[f32; 4]> { req.anchor_rect() }
    /// ```
    pub fn anchor_rect(&self) -> Option<[f32; 4]> { self.anchor_rect }

    /// Minimum width for the menu in view points: the anchor element's width when anchored by
    /// [`PopupAnchor::Key`], or `0.0` for [`PopupAnchor::Point`].
    ///
    /// ```
    /// # use rune::PopupMenuRequest;
    /// fn width_floor(req: &PopupMenuRequest) -> f32 { req.min_width() }
    /// ```
    pub fn min_width(&self) -> f32 { self.min_width }

    /// The top-level selected item id set with [`PopupMenu::selected`] or [`PopupMenu::picker`],
    /// if it names a top-level item.
    ///
    /// ```
    /// # use rune::PopupMenuRequest;
    /// fn selected_item(req: &PopupMenuRequest) -> Option<&str> { req.positioned() }
    /// ```
    pub fn positioned(&self) -> Option<&str> { self.positioned.as_deref() }

    /// Shared open-submenu index path for `rune-kit`'s `Menu::from_popup_menu` helper.
    #[doc(hidden)]
    pub fn submenu_open_path(&self) -> Rc<RefCell<Vec<usize>>> {
        Rc::clone(&self.open_submenu)
    }
}

impl Tx {
    /// Answers the pending pop-up menu delivered to
    /// [`App::on_popup_menu`](crate::App::on_popup_menu).
    ///
    /// Pass `Some(id)` with the picked [`PopupItem::id`](crate::PopupItem::id) to run the menu's
    /// callback (or built-in selection/link/image/field action), or `None` to dismiss it. Unknown
    /// or disabled item ids are ignored and leave the menu open; calls when no fallback menu is
    /// open do nothing.
    ///
    /// ```
    /// use rune::{Tx, el};
    ///
    /// let _dark = el().on_press_tx(|_: &mut (), tx: &mut Tx| {
    ///     tx.resolve_popup_menu(Some("dark"));
    /// });
    /// ```
    pub fn resolve_popup_menu(&mut self, id: Option<&str>) {
        self.popup_menu_resolution = Some(id.map(str::to_owned));
    }
}
