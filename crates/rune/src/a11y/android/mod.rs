//! Android accessibility (TalkBack) adapter, scroll pacing, and per-role focusability rules.
//!
//! `focusable` and `throttle` compile on every target so host unit tests exercise the grouping
//! policy and 100 ms scroll-pacing state machine without an Android emulator. The JNI bridge to
//! `accesskit_android::Adapter` compiles on `target_os = "android"` with `feature = "accessibility"`.

pub(crate) mod focusable;
#[cfg(test)]
mod focusable_tests;
pub(crate) mod throttle;
#[cfg(test)]
mod throttle_tests;

#[cfg(all(feature = "accessibility", target_os = "android"))]
mod bridge {
    use std::time::Duration;

    use accesskit_android::jni::JNIEnv;
    use accesskit_android::jni::objects::JObject;
    use accesskit_android::jni::sys::{jfloat, jint};
    use accesskit_android::{PlatformAction, QueuedEvents};

    use crate::a11y::{AccessibilityLink, Actions, Activation};
    use crate::machine::{Attempt, Seq};
    use crate::runtime::RuntimeCore;
    use crate::time::Instant;

    use super::focusable::{AndroidTreeIndex, NodeQuery};
    use super::throttle::{self, Effect, Event, State};

    const FOCUS_INPUT: jint = 1;

    /// Outcome of a frame or wake accessibility sync turn.
    pub(crate) struct SyncOutcome {
        /// Events that must be raised on the UI thread after releasing host borrows.
        pub(crate) events: Option<QueuedEvents>,
        /// Delayed wake to schedule when scroll updates are held inside the 100 ms cooldown.
        pub(crate) wake_after: Option<(Attempt, Duration)>,
    }

    /// Per-view Android accessibility adapter bridging `AccessibilityLink`,
    /// `accesskit_android::Adapter`, per-role focusability indexing, and scroll pacing.
    pub(crate) struct Adapter {
        link: AccessibilityLink,
        activation: Activation,
        actions: Actions,
        inner: accesskit_android::Adapter,
        index: AndroidTreeIndex,
        throttle: State,
        seq: Seq,
    }

    impl Adapter {
        pub(crate) fn new(wake: crate::Waker) -> Self {
            let link = AccessibilityLink::with_wake(wake);
            let activation = Activation(link.shared.clone());
            let actions = Actions(link.shared.clone());
            Self {
                link,
                activation,
                actions,
                inner: accesskit_android::Adapter::default(),
                index: AndroidTreeIndex::default(),
                throttle: State::default(),
                seq: Seq::default(),
            }
        }

        /// Drains activation requests and actions queued by TalkBack into `rt`.
        pub(crate) fn receive(&mut self, rt: &mut RuntimeCore, name: &str) -> bool {
            self.link.receive(rt, name)
        }

        /// Runs scroll-paced tree synchronization at the end of a UI frame.
        pub(crate) fn sync_on_frame(&mut self, rt: &mut RuntimeCore, now: Instant) -> SyncOutcome {
            let scrolling = rt.a11y_scroll_active();
            self.apply_throttle(rt, Event::Frame { now, scrolling })
        }

        /// Runs a scheduled scroll-pacing wake for `attempt`.
        pub(crate) fn sync_on_wake(
            &mut self,
            rt: &mut RuntimeCore,
            attempt: Attempt,
            now: Instant,
        ) -> SyncOutcome {
            let scrolling = rt.a11y_scroll_active();
            self.apply_throttle(rt, Event::Wake { attempt, now, scrolling })
        }

        /// Drops any pending scroll-hold timer (on pause, surface detach, or shutdown).
        pub(crate) fn reset_throttle(&mut self) {
            let _ = throttle::step(&mut self.seq, &mut self.throttle, Event::Reset);
        }

        fn apply_throttle(&mut self, rt: &mut RuntimeCore, event: Event) -> SyncOutcome {
            let mut outcome = SyncOutcome {
                events: None,
                wake_after: None,
            };
            for effect in throttle::step(&mut self.seq, &mut self.throttle, event) {
                match effect {
                    Effect::SyncNow => {
                        if let Some(update) = self.link.update(rt) {
                            self.index.apply_update(&update);
                            outcome.events = self.inner.update_if_active(|| update);
                        }
                    }
                    Effect::WakeAfter { attempt, delay } => {
                        outcome.wake_after = Some((attempt, delay));
                    }
                }
            }
            outcome
        }

        pub(crate) fn create_accessibility_node_info<'local>(
            &mut self,
            env: &mut JNIEnv<'local>,
            host: &JObject,
            virtual_view_id: jint,
        ) -> JObject<'local> {
            self.inner.create_accessibility_node_info(
                &mut self.activation,
                env,
                host,
                virtual_view_id,
            )
        }

        pub(crate) fn find_focus<'local>(
            &mut self,
            env: &mut JNIEnv<'local>,
            host: &JObject,
            focus_type: jint,
        ) -> JObject<'local> {
            // The host root is passive on Android; do not report it as holding input focus
            // when no child control in Rune is focused.
            if focus_type == FOCUS_INPUT && self.index.focus_is_root() {
                return JObject::null();
            }
            self.inner.find_focus(&mut self.activation, env, host, focus_type)
        }

        pub(crate) fn perform_action(
            &mut self,
            virtual_view_id: jint,
            action: &PlatformAction,
        ) -> Option<QueuedEvents> {
            self.inner.perform_action(&mut self.actions, virtual_view_id, action)
        }

        pub(crate) fn on_hover_event(
            &mut self,
            action: jint,
            x: jfloat,
            y: jfloat,
        ) -> Option<QueuedEvents> {
            self.inner.on_hover_event(&mut self.activation, action, x, y)
        }

        pub(crate) fn node_flags(&mut self, query: &NodeQuery<'_>) -> jint {
            self.index.flags_for_query(query)
        }
    }
}

#[cfg(all(feature = "accessibility", target_os = "android"))]
pub(crate) use bridge::{Adapter, SyncOutcome};
