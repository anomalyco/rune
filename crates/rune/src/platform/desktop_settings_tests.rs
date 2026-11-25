use std::sync::{Arc, Mutex};

use super::{DesktopSetting, Watch};

fn recording(namespace: &str, key: &str) -> (Watch, Arc<Mutex<Vec<DesktopSetting>>>) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let sink = got.clone();
    let watch = Watch {
        namespace: namespace.into(),
        key: key.into(),
        on_change: Box::new(move |v| sink.lock().unwrap().push(v)),
        last: None,
    };
    (watch, got)
}

#[test]
fn a_watch_gets_its_own_key_once_per_new_value() {
    let ns = "org.gnome.desktop.interface";
    let (mut watch, got) = recording(ns, "text-scaling-factor");
    assert!(watch.offer(ns, "text-scaling-factor", &DesktopSetting::Double(1.0)));
    assert!(watch.offer(ns, "text-scaling-factor", &DesktopSetting::Double(1.0)));
    assert!(watch.offer(ns, "cursor-blink-time", &DesktopSetting::Int(1200)));
    assert!(watch.offer("org.freedesktop.appearance", "text-scaling-factor", &DesktopSetting::Double(2.0)));
    assert!(watch.offer(ns, "text-scaling-factor", &DesktopSetting::Double(1.5)));
    assert_eq!(*got.lock().unwrap(), [DesktopSetting::Double(1.0), DesktopSetting::Double(1.5)]);
}

#[test]
fn a_panicking_callback_reports_it_instead_of_unwinding() {
    let mut watch = Watch {
        namespace: "org.gnome.desktop.interface".into(),
        key: "cursor-blink".into(),
        on_change: Box::new(|_| panic!("app callback")),
        last: None,
    };
    assert!(!watch.offer("org.gnome.desktop.interface", "cursor-blink", &DesktopSetting::Bool(true)));
}

#[test]
fn accessors_convert_numbers_and_refuse_other_types() {
    assert_eq!(DesktopSetting::Int(1200).as_f64(), Some(1200.0));
    assert_eq!(DesktopSetting::Uint(2).as_i64(), Some(2));
    assert_eq!(DesktopSetting::Uint(u64::MAX).as_i64(), None);
    assert_eq!(DesktopSetting::Double(1.25).as_f64(), Some(1.25));
    assert_eq!(DesktopSetting::Double(1.25).as_i64(), None);
    assert_eq!(DesktopSetting::Bool(true).as_bool(), Some(true));
    assert_eq!(DesktopSetting::Bool(true).as_f64(), None);
    assert_eq!(DesktopSetting::Str("Adwaita".into()).as_str(), Some("Adwaita"));
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
#[test]
fn decodes_portal_values_through_read_s_double_wrap() {
    use zbus::zvariant::Value;
    let decode = super::dbus::decode;
    assert_eq!(decode(&Value::F64(1.5)), Some(DesktopSetting::Double(1.5)));
    assert_eq!(decode(&Value::I32(400)), Some(DesktopSetting::Int(400)));
    assert_eq!(decode(&Value::U32(1)), Some(DesktopSetting::Uint(1)));
    assert_eq!(decode(&Value::Bool(false)), Some(DesktopSetting::Bool(false)));
    assert_eq!(decode(&Value::from("Cantarell 11")), Some(DesktopSetting::Str("Cantarell 11".into())));
    let twice = Value::Value(Box::new(Value::Value(Box::new(Value::Bool(true)))));
    assert_eq!(decode(&twice), Some(DesktopSetting::Bool(true)));
    assert_eq!(decode(&Value::from(vec![1_u32, 2])), None);
}
