//! StatusNotifierItem (`org.kde.StatusNotifierItem`) with a `com.canonical.dbusmenu` menu, on
//! its own session-bus connection and thread. The watcher is asked to show the item, again
//! whenever a new watcher takes the name (a bar that starts later or restarts).
use super::{Handler, TrayError, TrayEvent, TrayItem, TraySpec};
use crate::platform::portal;
use async_channel::{Sender, unbounded};
use futures_lite::{FutureExt, StreamExt};
use std::{collections::HashMap, sync::{Arc, Mutex, mpsc}, thread::JoinHandle, time::Duration};
use zbus::{
    Connection, Proxy,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Str, Value},
};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const ROOT: i32 = 0;

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
/// One dbusmenu node, `(ia{sv}av)`; children are variants holding the same shape.
type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

struct Model { id: String, title: String, tooltip: String, icon_name: String, pixmaps: Pixmaps, menu: Vec<TrayItem>, revision: u32, token: Option<String> }
type Shared = Arc<Mutex<Model>>;

enum Command { Menu(Vec<TrayItem>), Tooltip(String) }
pub(super) struct Native { commands: Option<Sender<Command>>, worker: Option<JoinHandle<()>> }

fn backend(e: impl std::fmt::Display) -> TrayError { TrayError::Backend(e.to_string()) }

/// ARGB32 in network byte order, as the spec's `IconPixmap` wants.
fn pixmaps(spec: &TraySpec) -> Pixmaps {
    let argb = spec.icon.rgba.chunks_exact(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    vec![(spec.icon.width as i32, spec.icon.height as i32, argb)]
}

/// Menu ids are the item's position plus one; 0 is the root.
fn properties(menu: &[TrayItem], id: i32) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    if id == ROOT { map.insert("children-display".into(), OwnedValue::from(Str::from_static("submenu"))); return map; }
    match menu.get(id as usize - 1) {
        Some(TrayItem::Action { label, enabled, .. }) => {
            map.insert("label".into(), OwnedValue::from(Str::from(super::dbusmenu_label(label))));
            if !enabled { map.insert("enabled".into(), OwnedValue::from(false)); }
        }
        Some(TrayItem::Separator) => { map.insert("type".into(), OwnedValue::from(Str::from_static("separator"))); }
        None => {}
    }
    map
}
fn layout(menu: &[TrayItem], id: i32) -> Layout {
    let children = if id == ROOT {
        (1..=menu.len() as i32).filter_map(|child| OwnedValue::try_from(Value::from(layout(menu, child))).ok()).collect()
    } else { Vec::new() };
    (id, properties(menu, id), children)
}

struct Item { model: Shared, on_event: Handler }

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    fn activate(&self, _x: i32, _y: i32) {
        let token = self.model.lock().unwrap().token.take();
        (self.on_event)(TrayEvent::Activate { token });
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {}
    /// The host draws the menu from `Menu`.
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
    fn provide_xdg_activation_token(&self, token: String) { self.model.lock().unwrap().token = Some(token); }
    #[zbus(property)]
    fn category(&self) -> &str { "ApplicationStatus" }
    #[zbus(property)]
    fn id(&self) -> String { self.model.lock().unwrap().id.clone() }
    #[zbus(property)]
    fn title(&self) -> String { self.model.lock().unwrap().title.clone() }
    #[zbus(property)]
    fn status(&self) -> &str { "Active" }
    #[zbus(property)]
    fn window_id(&self) -> i32 { 0 }
    /// A host that resolves the name draws that icon, and may recolour a symbolic one to fit its
    /// panel; one that can't, or gets no name, draws `IconPixmap`.
    #[zbus(property)]
    fn icon_name(&self) -> String { self.model.lock().unwrap().icon_name.clone() }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps { self.model.lock().unwrap().pixmaps.clone() }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (String::new(), Vec::new(), self.model.lock().unwrap().tooltip.clone(), String::new())
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool { false }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath { OwnedObjectPath::try_from(MENU_PATH).expect("MENU_PATH is an object path") }
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct Menu { model: Shared, on_event: Handler }
impl Menu {
    fn clicked(&self, id: i32, event: &str) {
        if event != "clicked" || id <= ROOT { return; }
        let picked = match self.model.lock().unwrap().menu.get(id as usize - 1) {
            Some(TrayItem::Action { id, enabled: true, .. }) => id.clone(),
            _ => return,
        };
        (self.on_event)(TrayEvent::Menu(picked));
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(&self, parent_id: i32, _recursion_depth: i32, _property_names: Vec<String>) -> (u32, Layout) {
        let model = self.model.lock().unwrap();
        (model.revision, layout(&model.menu, parent_id))
    }
    fn get_group_properties(&self, ids: Vec<i32>, _property_names: Vec<String>) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let model = self.model.lock().unwrap();
        // No ids asks for every item.
        let ids = if ids.is_empty() { (1..=model.menu.len() as i32).collect() } else { ids };
        ids.into_iter().map(|id| (id, properties(&model.menu, id))).collect()
    }
    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        properties(&self.model.lock().unwrap().menu, id).remove(&name)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("no such menu property".into()))
    }
    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) { self.clicked(id, &event_id); }
    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event, _, _) in events { self.clicked(id, &event); }
        Vec::new()
    }
    fn about_to_show(&self, _id: i32) -> bool { false }
    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) { (Vec::new(), Vec::new()) }
    #[zbus(property)]
    fn version(&self) -> u32 { 3 }
    #[zbus(property)]
    fn text_direction(&self) -> &str { "ltr" }
    #[zbus(property)]
    fn status(&self) -> &str { "normal" }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> { Vec::new() }
    #[zbus(signal)]
    async fn layout_updated(emitter: &SignalEmitter<'_>, revision: u32, parent: i32) -> zbus::Result<()>;
}

/// Asks the watcher to show the item, named by this connection's unique name so the host
/// finds it at `/StatusNotifierItem`. No watcher means no tray host yet.
async fn register(conn: &Connection) {
    let Some(name) = conn.unique_name().map(|n| n.to_string()) else { return };
    let call = async {
        let watcher = Proxy::new(conn, WATCHER, WATCHER_PATH, WATCHER).await?;
        watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),)).await
    };
    let _ = portal::timeout(RPC_TIMEOUT, call).await;
}

impl Native {
    pub(super) fn new(spec: TraySpec, on_event: Handler) -> Result<Self, TrayError> {
        let model = Arc::new(Mutex::new(Model { pixmaps: pixmaps(&spec), id: spec.id, title: spec.title, tooltip: spec.tooltip,
            icon_name: spec.icon_name, menu: spec.menu, revision: 1, token: None }));
        let (commands, receiver) = unbounded();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-tray".into()).spawn(move || {
            async_io::block_on(async move {
                let setup = async {
                    let conn = zbus::connection::Builder::session()?
                        .serve_at(ITEM_PATH, Item { model: model.clone(), on_event: on_event.clone() })?
                        .serve_at(MENU_PATH, Menu { model: model.clone(), on_event })?
                        .build().await?;
                    let bus = Proxy::new(&conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
                    let owners = bus.receive_signal_with_args("NameOwnerChanged", &[(0, WATCHER)]).await?;
                    Ok::<_, zbus::Error>((conn, owners))
                };
                let (conn, mut owners) = match portal::timeout(RPC_TIMEOUT, setup).await {
                    Some(Ok(v)) => { ready.send(Ok(())).ok(); v }
                    Some(Err(e)) => { ready.send(Err(backend(e))).ok(); return; }
                    None => { ready.send(Err(backend("session bus timed out"))).ok(); return; }
                };
                register(&conn).await;
                enum Wake {
                    Command(Option<Command>),
                    Owner(Option<zbus::Message>),
                }
                loop {
                    let next = async { Wake::Command(receiver.recv().await.ok()) }
                        .or(async { Wake::Owner(owners.next().await) })
                        .await;
                    match next {
                        Wake::Command(Some(Command::Menu(menu))) => {
                            let revision = { let mut m = model.lock().unwrap(); m.menu = menu; m.revision += 1; m.revision };
                            if let Ok(emitter) = SignalEmitter::new(&conn, MENU_PATH) { Menu::layout_updated(&emitter, revision, ROOT).await.ok(); }
                        }
                        Wake::Command(Some(Command::Tooltip(tooltip))) => {
                            model.lock().unwrap().tooltip = tooltip;
                            if let Ok(emitter) = SignalEmitter::new(&conn, ITEM_PATH) { Item::new_tool_tip(&emitter).await.ok(); }
                        }
                        Wake::Command(None) | Wake::Owner(None) => break,
                        Wake::Owner(Some(changed)) => {
                            let owner = changed.body().deserialize::<(String, String, String)>().map(|(_, _, new)| new);
                            if owner.is_ok_and(|new| !new.is_empty()) { register(&conn).await; }
                        }
                    }
                }
                // Closing the connection removes the icon at once.
                conn.close().await.ok();
            });
        }).map_err(backend)?;
        match result.recv() {
            Ok(Ok(())) => Ok(Self { commands: Some(commands), worker: Some(worker) }),
            Ok(Err(e)) => { worker.join().ok(); Err(e) }
            Err(_) => { worker.join().ok(); Err(backend("tray thread exited")) }
        }
    }
    pub(super) fn set_menu(&self, menu: Vec<TrayItem>) {
        if let Some(c) = &self.commands { c.try_send(Command::Menu(menu)).ok(); }
    }
    pub(super) fn set_tooltip(&self, tooltip: String) {
        if let Some(c) = &self.commands { c.try_send(Command::Tooltip(tooltip)).ok(); }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() { worker.join().ok(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Type;

    fn menu() -> Vec<TrayItem> { vec![TrayItem::action("show", "Show"), TrayItem::Separator, TrayItem::action("quit", "Quit").enabled(false)] }

    #[test]
    fn the_layout_has_the_dbusmenu_shape() {
        assert_eq!(<(u32, Layout)>::SIGNATURE.to_string(), "(u(ia{sv}av))");
        let (id, props, children) = layout(&menu(), ROOT);
        assert_eq!(id, ROOT);
        assert!(props.contains_key("children-display"));
        assert_eq!(children.len(), 3);
    }

    #[test]
    fn items_carry_label_type_and_enabled() {
        let menu = menu();
        assert!(properties(&menu, 1).contains_key("label"));
        assert!(properties(&menu, 2).contains_key("type"));
        assert!(properties(&menu, 3).contains_key("enabled"));
        assert!(properties(&menu, 9).is_empty());
        let underscored = vec![TrayItem::action("open", "open_file")];
        let props = properties(&underscored, 1);
        assert_eq!(<&str>::try_from(props.get("label").unwrap()).ok(), Some("open__file"));
    }

    #[test]
    fn the_icon_is_argb_in_network_order() {
        let spec = TraySpec::new("t", "T", super::super::TrayIcon::rgba(1, 1, vec![1, 2, 3, 4]));
        assert_eq!(pixmaps(&spec), vec![(1, 1, vec![4, 1, 2, 3])]);
    }

    fn item(icon_name: &str) -> Item {
        let spec = TraySpec::new("t", "T", super::super::TrayIcon::rgba(1, 1, vec![1, 2, 3, 4])).icon_name(icon_name);
        let pixmaps = pixmaps(&spec);
        let model = Model { id: spec.id, title: spec.title, tooltip: spec.tooltip, icon_name: spec.icon_name,
            pixmaps, menu: spec.menu, revision: 1, token: None };
        Item { model: Arc::new(Mutex::new(model)), on_event: Arc::new(|_| {}) }
    }

    #[test]
    fn the_icon_name_is_served_and_empty_until_set() {
        assert_eq!(item("").icon_name(), "");
        assert_eq!(item("my-app-symbolic").icon_name(), "my-app-symbolic");
        assert_eq!(item("my-app-symbolic").icon_pixmap(), vec![(1, 1, vec![4, 1, 2, 3])]);
    }
}
