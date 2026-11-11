use super::*;

/// The column as the old code kept it. Its own type, so the reference shares nothing with the
/// conversions it checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OldRegion {
    List,
    Stage,
    Side,
}

/// The caches this replaced, transcribed from the old code: the column, `side_active` beside
/// it, and `region_seen`. It calls nothing in production.
#[derive(Clone, Debug)]
struct Old {
    region: OldRegion,
    side_active: bool,
    region_seen: Option<&'static str>,
}

impl Old {
    fn set_region(&mut self, region: OldRegion) -> bool {
        std::mem::replace(&mut self.region, region) != region
    }

    fn see_focus(&mut self, focused: Option<&'static str>) -> bool {
        if self.region_seen == focused { return false; }
        self.region_seen = focused;
        true
    }

    fn set_side_active(&mut self, active: bool) -> bool {
        std::mem::replace(&mut self.side_active, active) != active
    }

    /// `give_region`.
    fn give_region(&mut self, region: OldRegion, focused: Option<&'static str>) {
        self.see_focus(focused);
        self.set_region(region);
    }

    /// `sync_region`, `at` being the column of the focused element when there is one.
    fn sync_region(&mut self, focused: Option<&'static str>, hover: bool, at: Option<OldRegion>) {
        if !self.see_focus(focused) || hover { return; }
        let Some(region) = at else { return };
        self.set_region(region);
    }

    /// `Live::close`.
    fn close(&mut self) {
        self.region = OldRegion::List;
    }
}

/// A focusable element: its column, whether it's the side column's slot, and whether the pointer
/// can light it. The slot has no `focus_highlight`, so a hover never lands on it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Node {
    path: &'static str,
    column: OldRegion,
    slot: bool,
    highlight: bool,
}

const LIST_ROW: Node = Node { path: "list/row", column: OldRegion::List, slot: false, highlight: true };
const SIDE_ROW: Node = Node { path: "side/row", column: OldRegion::Side, slot: false, highlight: true };
const SLOT: Node = Node { path: "side/slot", column: OldRegion::Side, slot: true, highlight: false };
const FIELD: Node = Node { path: "stage/field", column: OldRegion::Stage, slot: false, highlight: false };

/// An event as the runtime meets it.
#[derive(Clone, Copy, Debug)]
enum Feed {
    /// `set_focus`, which reports the slot.
    Focus(Option<Node>),
    /// The pointer lights or leaves the focused row, which reports nothing.
    Hover(bool),
    /// `sync_region`, as a commit or a key looks at focus.
    Look,
    /// A press in a column, or a key that asks for one. It leaves focus where it is.
    Press(OldRegion),
    /// The playground closes.
    Close,
    /// A step's key starts its route, setting aside focus outside the stage.
    RouteStart,
    /// Its route ends, putting focus back if the key left none.
    RouteEnd,
}

const FEEDS: [Feed; 14] = [
    Feed::Focus(None),
    Feed::Focus(Some(LIST_ROW)),
    Feed::Focus(Some(SIDE_ROW)),
    Feed::Focus(Some(SLOT)),
    Feed::Focus(Some(FIELD)),
    Feed::Hover(true),
    Feed::Hover(false),
    Feed::Look,
    Feed::Press(OldRegion::List),
    Feed::Press(OldRegion::Stage),
    Feed::Press(OldRegion::Side),
    Feed::Close,
    Feed::RouteStart,
    Feed::RouteEnd,
];

/// What a route remembers while it runs, for each side.
#[derive(Clone, Copy)]
struct Route {
    old: OldRegion,
    new: Region,
    aside: Option<(Node, bool)>,
}

/// How many of the sequences the walk has to reach it reached.
#[derive(Default)]
struct Reached {
    retained_clicks: u64,
    restored_unseen: u64,
    routes_moving_focus: u64,
    closes_on_the_slot: u64,
}

/// The focus both sides read, the keyboard, and the old caches.
#[derive(Clone)]
struct Sim {
    keys: Keyboard,
    old: Old,
    focus: Option<Node>,
    hovered: bool,
    route: Option<Route>,
    /// A route blurred the focus it had set aside and then put it back, which reports nothing:
    /// the old flag stayed false with focus on the slot, whose keys it still took.
    old_flag_stale: bool,
}

fn new_region(region: OldRegion) -> Region {
    match region {
        OldRegion::List => Region::List,
        OldRegion::Stage => Region::Stage,
        OldRegion::Side => Region::Side,
    }
}

impl Sim {
    fn new() -> Self {
        Self {
            keys: Keyboard::default(),
            old: Old { region: OldRegion::List, side_active: false, region_seen: None },
            focus: None,
            hovered: false,
            route: None,
            old_flag_stale: false,
        }
    }

    fn path(&self) -> Option<&'static str> {
        self.focus.map(|n| n.path)
    }

    /// `on_side_keys`.
    fn on_slot(&self) -> bool {
        !self.hovered && self.focus.is_some_and(|n| n.slot)
    }

    /// Whether the runtime can meet `feed` now. A route runs inside one key, so the pointer and
    /// a close wait for it.
    fn allows(&self, feed: Feed) -> bool {
        let routing = self.route.is_some();
        match feed {
            Feed::Focus(_) | Feed::Look => true,
            Feed::Hover(lit) => !routing && self.hovered != lit && self.focus.is_some_and(|n| n.highlight),
            Feed::Press(_) => true,
            Feed::Close | Feed::RouteStart => !routing,
            Feed::RouteEnd => routing,
        }
    }

    /// Feeds `feed` to both sides; true when the keyboard says its region changed.
    fn feed(&mut self, feed: Feed, reached: &mut Reached) -> bool {
        match feed {
            Feed::Focus(next) => {
                if self.path() != next.map(|n| n.path) { self.hovered = false; }
                self.focus = next;
                self.old_flag_stale = false;
                let slot = self.on_slot();
                self.old.set_side_active(slot);
                self.keys.step(Moved::Slot(slot))
            }
            Feed::Hover(lit) => {
                self.hovered = lit;
                false
            }
            Feed::Look => {
                let at = if self.hovered { None } else { self.focus };
                self.old.sync_region(self.path(), self.hovered, self.focus.map(|n| n.column));
                if self.keys.saw(self.path()) { return false; }
                self.keys.step(Moved::Focus {
                    focus: self.path().map(str::to_owned),
                    region: at.map(|n| new_region(n.column)),
                })
            }
            Feed::Press(region) => {
                if region == OldRegion::Stage && self.focus == Some(SLOT) { reached.retained_clicks += 1; }
                self.old.give_region(region, self.path());
                self.keys.step(Moved::Give { region: new_region(region), seen: self.path().map(str::to_owned) })
            }
            Feed::Close => {
                if self.focus == Some(SLOT) { reached.closes_on_the_slot += 1; }
                self.old.close();
                self.keys.step(Moved::Closed)
            }
            Feed::RouteStart => {
                let aside = self.focus.filter(|n| n.column != OldRegion::Stage).map(|n| (n, self.hovered));
                self.route = Some(Route { old: self.old.region, new: self.keys.region(), aside });
                if aside.is_some() {
                    self.focus = None;
                    self.hovered = false;
                }
                self.old.set_region(OldRegion::Stage);
                self.keys.step(Moved::Hold(Region::Stage))
            }
            Feed::RouteEnd => {
                let route = self.route.take().expect("a route is running");
                let back = route.aside.filter(|_| self.focus.is_none());
                if route.aside.is_some() && back.is_none() { reached.routes_moving_focus += 1; }
                if back.is_some_and(|(n, _)| self.old.region_seen != Some(n.path)) { reached.restored_unseen += 1; }
                self.old.set_region(route.old);
                if let Some((node, _)) = back { self.old.see_focus(Some(node.path)); }
                let mut changed = self.keys.step(Moved::Resume { region: route.new, back: back.map(|(n, _)| n.path.to_owned()) });
                if let Some((node, hovered)) = back {
                    self.focus = Some(node);
                    self.hovered = hovered;
                    self.old_flag_stale = self.old.side_active != self.on_slot();
                    assert!(!self.old_flag_stale || self.on_slot(), "the one difference keeps a mark, never drops one");
                    changed |= self.keys.step(Moved::Slot(self.on_slot()));
                }
                changed
            }
        }
    }

    /// The keyboard against the old caches, by hand so no production conversion checks itself.
    /// They differ in one approved way: the mark on a slot a route blurred and put back.
    fn check(&self, trail: &[Feed]) {
        let region = match self.keys.region() {
            Region::List => OldRegion::List,
            Region::Stage => OldRegion::Stage,
            Region::Side => OldRegion::Side,
        };
        assert_eq!(region, self.old.region, "{trail:?}: the column");
        assert_eq!(self.keys.slot_focused(), self.old.side_active || self.old_flag_stale, "{trail:?}: what looks active");
        assert!(self.keys.saw(self.old.region_seen), "{trail:?}: the focus last taken, {:?}", self.old.region_seen);
    }
}

fn walk(sim: &Sim, depth: usize, trail: &mut Vec<Feed>, reached: &mut Reached) -> u64 {
    let mut checked = 0;
    for feed in FEEDS {
        if !sim.allows(feed) { continue; }
        let mut next = sim.clone();
        let before = (next.keys.region(), next.keys.slot_focused());
        let changed = next.feed(feed, reached);
        trail.push(feed);
        assert_eq!(changed, before != (next.keys.region(), next.keys.slot_focused()), "{trail:?}: says whether it changed");
        next.check(trail);
        checked += 1;
        if depth > 1 { checked += walk(&next, depth - 1, trail, reached); }
        trail.pop();
    }
    checked
}

#[test]
fn any_order_of_keyboard_events_leaves_the_column_slot_and_seen_focus_the_old_caches_did() {
    let mut reached = Reached::default();
    let checked = walk(&Sim::new(), 7, &mut Vec::new(), &mut reached);
    assert!(checked > 1_000_000, "walked {checked}");
    assert!(reached.retained_clicks > 0, "a stage click that keeps the slot's focus");
    assert!(reached.restored_unseen > 0, "a route restoring a focus the keyboard hadn't seen");
    assert!(reached.routes_moving_focus > 0, "a route whose key moved focus");
    assert!(reached.closes_on_the_slot > 0, "a close with focus on the slot");
}

#[test]
fn a_look_at_focus_it_already_took_has_nothing_to_do() {
    let mut keys = Keyboard::default();
    assert!(keys.saw(None));
    keys.step(Moved::Focus { focus: Some("side/row".into()), region: None });
    assert!(keys.saw(Some("side/row")), "a hover is taken as seen too, so it isn't looked at again");
    assert!(!keys.saw(None));
}

#[test]
fn a_clicked_column_keeps_the_keyboard_until_focus_moves() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::Stage, seen: None });
    // A frame's look at focus, which nothing has: the stage keeps the keyboard.
    keys.step(Moved::Focus { focus: None, region: None });
    assert_eq!(keys.region(), Region::Stage);
    keys.step(Moved::Focus { focus: Some("list/story".into()), region: Some(Region::List) });
    assert_eq!(keys.region(), Region::List);
}

#[test]
fn a_stage_click_that_keeps_the_slots_focus_keeps_its_mark() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Slot(true));
    keys.step(Moved::Give { region: Region::Side, seen: Some("side/slot".into()) });
    keys.step(Moved::Give { region: Region::Stage, seen: Some("side/slot".into()) });
    keys.step(Moved::Focus { focus: Some("side/slot".into()), region: Some(Region::Side) });
    assert_eq!(keys.region(), Region::Stage, "focus didn't move, so the click stands");
    assert!(keys.slot_focused(), "and the slot it left focused stays marked");
}

#[test]
fn the_slot_goes_by_focus_whatever_the_column() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::Stage, seen: None });
    assert!(keys.step(Moved::Slot(true)), "a change redraws its view");
    assert_eq!(keys.region(), Region::Stage, "and moves no column");
    assert!(keys.step(Moved::Slot(false)));
    assert!(!keys.slot_focused());
}

#[test]
fn a_route_that_moves_focus_gives_the_column_it_landed_in_the_keyboard() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::List, seen: Some("list/story".into()) });
    // A key a step pressed holds the stage, then gives the list back: focus was set aside, so
    // nothing new is seen.
    keys.step(Moved::Hold(Region::Stage));
    keys.step(Moved::Resume { region: Region::List, back: None });
    assert_eq!(keys.region(), Region::List);
    // The stage's key focused a control in the side column: the next look at focus gives it the
    // keyboard, because the route restored nothing.
    keys.step(Moved::Focus { focus: Some("side/variant".into()), region: Some(Region::Side) });
    assert_eq!(keys.region(), Region::Side, "focus where the route left it takes the keyboard");
}

#[test]
fn a_route_that_puts_focus_back_keeps_the_column_it_had() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::Stage, seen: Some("stage/old".into()) });
    keys.step(Moved::Hold(Region::Stage));
    keys.step(Moved::Resume { region: Region::Stage, back: Some("stage/field".into()) });
    assert!(keys.saw(Some("stage/field")), "the focus it put back is the one it has seen");
    // The focus that came back isn't a move.
    keys.step(Moved::Focus { focus: Some("stage/field".into()), region: Some(Region::List) });
    assert_eq!(keys.region(), Region::Stage);
}

#[test]
fn closing_gives_the_list_the_keyboard_and_keeps_the_focus_it_saw() {
    let mut keys = Keyboard::default();
    keys.step(Moved::Give { region: Region::Side, seen: Some("side/slot".into()) });
    assert!(keys.step(Moved::Closed));
    assert_eq!(keys.region(), Region::List);
    assert!(keys.saw(Some("side/slot")));
}

#[test]
fn a_route_that_blurs_the_focus_it_puts_back_keeps_the_slots_mark() {
    let mut sim = Sim::new();
    let mut reached = Reached::default();
    for feed in [Feed::Focus(Some(SLOT)), Feed::Press(OldRegion::Side), Feed::RouteStart, Feed::Focus(None), Feed::RouteEnd] {
        sim.feed(feed, &mut reached);
    }
    assert_eq!(sim.focus, Some(SLOT));
    assert!(!sim.old.side_active, "the old flag stayed off with the slot focused");
    assert!(sim.keys.slot_focused(), "the keyboard follows focus");
}
