//! Pure state machine, RFC 2045 MIME priority matching, and text decoding for Wayland
//! `zwp_primary_selection_unstable_v1`.
//!
//! Kept cross-platform so every compositor event ordering (Mutter `selection(None)` ->
//! `selection(Some(new))`, Hyprland pointer-enter offer delivery, self-paste fast path,
//! out-of-order stale `Cancelled`, missing serial, MIME priority, Latin-1 `STRING` decode,
//! `MAX_PENDING_OFFERS` eviction, writer cap, latest-wins read cancellation, and
//! `needs_wake_on_read` idle invariants) is unit-tested on every host.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use crate::machine::{Attempt, Seq};
use crate::platform::wayland::pipe::ReadOutcome;

/// MIME types advertised on our `zwp_primary_selection_source_v1`.
#[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
pub(crate) const OFFERED_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// Hard upper bound on a single primary-selection read or write transfer (4 MiB, matching
/// `clipboard/html.rs::MAX_RICH_BYTES`).
pub(crate) const MAX_PRIMARY_BYTES: usize = 4 * 1024 * 1024;

/// Pipe-read timeout before giving up on a silent primary-selection owner (2 s, matching
/// `drop/machine.rs::DROP_READ_TIMEOUT`).
#[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(dead_code))]
pub(crate) const PRIMARY_READ_TIMEOUT: Duration = Duration::from_millis(2_000);

/// Cap on unclaimed `zwp_primary_selection_offer_v1` proxies retained between `data_offer` and
/// `selection` per seat (matching `drop/machine.rs::MAX_PENDING_OFFERS`).
pub(crate) const MAX_PENDING_OFFERS: usize = 16;

/// Cap on concurrent off-thread `zwp_primary_selection_source_v1.send` pipe writers.
pub(crate) const MAX_CONCURRENT_WRITERS: usize = 8;

/// Cap on retired (replaced) sources kept awaiting compositor `cancelled` before evicting oldest.
const MAX_RETIRED_SOURCES: usize = 8;

/// Cap on non-text MIME entries remembered per offer to bound memory from hostile peers.
const MAX_OFFER_MIMES: usize = 64;

/// Wayland `wl_seat` registry global name.
pub(crate) type SeatId = u32;

/// Wayland wire protocol ID of a `zwp_primary_selection_offer_v1`.
pub(crate) type OfferId = u32;

/// Client-side monotonic ID identifying each `zwp_primary_selection_source_v1` we create.
pub(crate) type SourceId = u64;

/// Client-side monotonic ID stashing an incoming `send` `OwnedFd` across pure `step`.
pub(crate) type WriteFdId = u64;

/// Availability of `zwp_primary_selection_device_manager_v1` on the current Wayland display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Availability {
    /// Manager is bound and active.
    Available,
    /// Compositor does not advertise `zwp_primary_selection_device_manager_v1`; fall back to arboard.
    Unavailable,
    /// Display connection failed or controller is shutting down.
    Dead,
}

/// Character encoding used to decode bytes read from a `zwp_primary_selection_offer_v1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextEncoding {
    Utf8,
    Latin1,
}

/// Returns `Some((priority, encoding))` for a recognized primary-selection text MIME type,
/// matching Chromium's RFC 2045 case-insensitive ASCII preference order:
/// `text/plain;charset=utf-8` (0) > `UTF8_STRING` (1) > `text/plain` (2) > `TEXT` (3) > `STRING` (4).
pub(crate) fn mime_priority(mime: &str) -> Option<(u8, TextEncoding)> {
    let trimmed = mime.trim_ascii();
    if trimmed.eq_ignore_ascii_case("UTF8_STRING") {
        return Some((1, TextEncoding::Utf8));
    }
    if trimmed.eq_ignore_ascii_case("text/plain") {
        return Some((2, TextEncoding::Utf8));
    }
    if trimmed.eq_ignore_ascii_case("TEXT") {
        return Some((3, TextEncoding::Utf8));
    }
    if trimmed.eq_ignore_ascii_case("STRING") {
        return Some((4, TextEncoding::Latin1));
    }
    // Check RFC 2045 parameter form: `text/plain;charset=utf-8` (allowing ASCII whitespace around
    // `;` / `=` and optional quotes around the charset value).
    if let Some((media, params)) = trimmed.split_once(';')
        && media.trim_ascii().eq_ignore_ascii_case("text/plain")
    {
        for param in params.split(';') {
            if let Some((k, v)) = param.split_once('=')
                && k.trim_ascii().eq_ignore_ascii_case("charset")
            {
                let v = v.trim_ascii();
                let unquoted = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
                if unquoted.eq_ignore_ascii_case("utf-8") {
                    return Some((0, TextEncoding::Utf8));
                }
            }
        }
    }
    None
}

/// Picks the highest-priority text MIME from `mimes`, returning the **exact advertised MIME
/// string** (to pass unchanged to `offer.receive`) and its text encoding.
pub(crate) fn select_best_mime(mimes: &[String]) -> Option<(&str, TextEncoding)> {
    let mut best: Option<(u8, &str, TextEncoding)> = None;
    for mime in mimes {
        if let Some((prio, enc)) = mime_priority(mime)
            && best.is_none_or(|(b, _, _)| prio < b)
        {
            best = Some((prio, mime.as_str(), enc));
            if prio == 0 {
                break;
            }
        }
    }
    best.map(|(_, m, enc)| (m, enc))
}

/// Returns `true` if `mime` matches one of our [`OFFERED_MIMES`] (case-insensitive ASCII).
pub(crate) fn is_offered_mime(mime: &str) -> bool {
    matches!(mime_priority(mime), Some((0 | 1 | 2, TextEncoding::Utf8)))
}

/// Decodes raw bytes read from a primary-selection offer pipe according to `encoding`,
/// returning `None` if the resulting string is empty.
pub(crate) fn decode_text(bytes: Vec<u8>, encoding: TextEncoding) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let text = match encoding {
        TextEncoding::Utf8 => String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
        TextEncoding::Latin1 => bytes.into_iter().map(char::from).collect(),
    };
    (!text.is_empty()).then_some(text)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OfferEntry {
    id: OfferId,
    mimes: Vec<String>,
}

impl OfferEntry {
    fn push_mime(&mut self, mime: String) {
        if self.mimes.iter().any(|m| m == &mime) {
            return;
        }
        // Always retain recognized text MIMEs even if a peer sent many non-text types first.
        if self.mimes.len() < MAX_OFFER_MIMES || mime_priority(&mime).is_some() {
            self.mimes.push(mime);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SeatState {
    pending_offers: VecDeque<OfferEntry>,
    selection: Option<OfferEntry>,
    selection_order: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnedSource {
    id: SourceId,
    seat: SeatId,
    text: Arc<str>,
    bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RetiredSource {
    id: SourceId,
    seat: SeatId,
    bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveRead {
    attempt: Attempt,
    target: String,
    encoding: TextEncoding,
}

/// Pure per-display `zwp_primary_selection_v1` state machine.
#[derive(Clone, Debug)]
pub(crate) struct State {
    availability: Availability,
    seats: HashMap<SeatId, SeatState>,
    next_selection_order: u64,
    next_source_id: SourceId,
    active_source: Option<OwnedSource>,
    retired_sources: VecDeque<RetiredSource>,
    read_seq: Seq,
    active_read: Option<ActiveRead>,
    active_writers: usize,
}

impl State {
    pub(crate) fn new(availability: Availability) -> Self {
        Self {
            availability,
            seats: HashMap::new(),
            next_selection_order: 0,
            next_source_id: 1,
            active_source: None,
            retired_sources: VecDeque::new(),
            read_seq: Seq::default(),
            active_read: None,
            active_writers: 0,
        }
    }

    pub(crate) fn availability(&self) -> Availability {
        self.availability
    }

    /// True strictly while our own `zwp_primary_selection_source_v1` is owned (or a retired
    /// source is still awaiting compositor `cancelled`). Plain windows with no owned source
    /// always return `false` so `set_wake_on_wayland_read` stays off.
    pub(crate) fn needs_wake_on_read(&self) -> bool {
        self.availability == Availability::Available
            && (self.active_source.is_some() || !self.retired_sources.is_empty())
    }

    #[cfg(test)]
    pub(crate) fn active_source_id(&self) -> Option<SourceId> {
        self.active_source.as_ref().map(|s| s.id)
    }

    #[cfg(test)]
    pub(crate) fn active_writers(&self) -> usize {
        self.active_writers
    }

    fn alloc_source_id(&mut self) -> SourceId {
        let id = self.next_source_id;
        self.next_source_id = self.next_source_id.wrapping_add(1).max(1);
        id
    }

    fn pick_selection_offer(&self, preferred_seat: Option<SeatId>) -> Option<(OfferId, String, TextEncoding)> {
        if let Some(seat) = preferred_seat
            && let Some(info) = self.seats.get(&seat)
            && let Some(offer) = &info.selection
            && let Some((mime, enc)) = select_best_mime(&offer.mimes)
        {
            return Some((offer.id, mime.to_owned(), enc));
        }
        let mut best: Option<(u64, OfferId, &str, TextEncoding)> = None;
        for info in self.seats.values() {
            if let Some(offer) = &info.selection
                && let Some((mime, enc)) = select_best_mime(&offer.mimes)
                && best.is_none_or(|(ord, _, _, _)| info.selection_order > ord)
            {
                best = Some((info.selection_order, offer.id, mime, enc));
            }
        }
        best.map(|(_, id, mime, enc)| (id, mime.to_owned(), enc))
    }

    fn source_bytes(&self, source_id: SourceId) -> Option<Arc<[u8]>> {
        if let Some(active) = &self.active_source
            && active.id == source_id
        {
            return Some(Arc::clone(&active.bytes));
        }
        self.retired_sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| Arc::clone(&s.bytes))
    }
}

/// Inputs driving [`step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    SeatAdded { seat: SeatId },
    SeatRemoved { seat: SeatId },
    DataOffer { seat: SeatId, offer_id: OfferId },
    OfferMime { offer_id: OfferId, mime_type: String },
    Selection { seat: SeatId, offer_id: Option<OfferId> },
    RequestWrite { text: String, serial: Option<(SeatId, u32)> },
    SourceSend { source_id: SourceId, mime_type: String, fd_id: WriteFdId },
    WriterFinished,
    SourceCancelled { source_id: SourceId },
    RequestRead { target: String, preferred_seat: Option<SeatId> },
    ReadFinished { attempt: Attempt, outcome: ReadOutcome },
    DisplayLost,
    Shutdown,
}

/// Side effects emitted by [`step`] for the Wayland wire layer to carry out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    GetDevice { seat: SeatId },
    DestroyDevice { seat: SeatId },
    DestroyOffer { offer_id: OfferId },
    SetSelection { seat: SeatId, source_id: SourceId, serial: u32 },
    DestroySource { source_id: SourceId },
    SelectionLost,
    SpawnWriter { fd_id: WriteFdId, bytes: Arc<[u8]> },
    CloseFd { fd_id: WriteFdId },
    StartRead { attempt: Attempt, offer_id: OfferId, mime: String },
    CancelRead { attempt: Attempt },
    DeliverText { attempt: Option<Attempt>, target: String, text: Arc<str> },
}

/// Advances `state` by `event` and returns the ordered side effects for the wire layer.
pub(crate) fn step(state: &mut State, event: Event) -> Vec<Effect> {
    let mut effects = Vec::new();
    match event {
        Event::SeatAdded { seat } => {
            if state.availability != Availability::Available || state.seats.contains_key(&seat) {
                return effects;
            }
            state.seats.insert(seat, SeatState::default());
            effects.push(Effect::GetDevice { seat });
        }
        Event::SeatRemoved { seat } => {
            if let Some(mut info) = state.seats.remove(&seat) {
                while let Some(offer) = info.pending_offers.pop_front() {
                    effects.push(Effect::DestroyOffer { offer_id: offer.id });
                }
                if let Some(offer) = info.selection.take() {
                    effects.push(Effect::DestroyOffer { offer_id: offer.id });
                }
                effects.push(Effect::DestroyDevice { seat });
            }
            if state.active_source.as_ref().is_some_and(|s| s.seat == seat)
                && let Some(owned) = state.active_source.take()
            {
                effects.push(Effect::DestroySource { source_id: owned.id });
                effects.push(Effect::SelectionLost);
            }
            let mut i = 0;
            while i < state.retired_sources.len() {
                if state.retired_sources[i].seat == seat {
                    if let Some(retired) = state.retired_sources.remove(i) {
                        effects.push(Effect::DestroySource { source_id: retired.id });
                    }
                } else {
                    i += 1;
                }
            }
        }
        Event::DataOffer { seat, offer_id } => {
            if state.availability != Availability::Available {
                effects.push(Effect::DestroyOffer { offer_id });
                return effects;
            }
            let Some(info) = state.seats.get_mut(&seat) else {
                effects.push(Effect::DestroyOffer { offer_id });
                return effects;
            };
            if info.pending_offers.len() >= MAX_PENDING_OFFERS
                && let Some(evicted) = info.pending_offers.pop_front()
            {
                effects.push(Effect::DestroyOffer { offer_id: evicted.id });
            }
            info.pending_offers.push_back(OfferEntry { id: offer_id, mimes: Vec::new() });
        }
        Event::OfferMime { offer_id, mime_type } => {
            if state.availability != Availability::Available {
                return effects;
            }
            for info in state.seats.values_mut() {
                if let Some(entry) = info.pending_offers.iter_mut().find(|o| o.id == offer_id) {
                    entry.push_mime(mime_type);
                    return effects;
                }
                if let Some(entry) = info.selection.as_mut().filter(|o| o.id == offer_id) {
                    entry.push_mime(mime_type);
                    return effects;
                }
            }
        }
        Event::Selection { seat, offer_id } => {
            if state.availability != Availability::Available {
                if let Some(id) = offer_id {
                    effects.push(Effect::DestroyOffer { offer_id: id });
                }
                return effects;
            }
            let Some(info) = state.seats.get_mut(&seat) else {
                if let Some(id) = offer_id {
                    effects.push(Effect::DestroyOffer { offer_id: id });
                }
                return effects;
            };
            if let Some(prev) = info.selection.take()
                && Some(prev.id) != offer_id
            {
                effects.push(Effect::DestroyOffer { offer_id: prev.id });
            }
            if let Some(id) = offer_id {
                let entry = if let Some(pos) = info.pending_offers.iter().position(|o| o.id == id) {
                    info.pending_offers.remove(pos).unwrap()
                } else {
                    OfferEntry { id, mimes: Vec::new() }
                };
                state.next_selection_order = state.next_selection_order.wrapping_add(1);
                info.selection = Some(entry);
                info.selection_order = state.next_selection_order;
            }
        }
        Event::RequestWrite { text, serial } => {
            if state.availability != Availability::Available || text.is_empty() {
                return effects;
            }
            let Some((seat, serial_val)) = serial else {
                return effects;
            };
            if !state.seats.contains_key(&seat) {
                return effects;
            }
            if text.len() > MAX_PRIMARY_BYTES {
                return effects;
            }
            let source_id = state.alloc_source_id();
            if let Some(prev) = state.active_source.take() {
                if state.retired_sources.len() >= MAX_RETIRED_SOURCES
                    && let Some(evicted) = state.retired_sources.pop_front()
                {
                    effects.push(Effect::DestroySource { source_id: evicted.id });
                }
                state.retired_sources.push_back(RetiredSource {
                    id: prev.id,
                    seat: prev.seat,
                    bytes: prev.bytes,
                });
            }
            let bytes: Arc<[u8]> = Arc::from(text.as_bytes());
            let arc_text: Arc<str> = Arc::from(text);
            state.active_source = Some(OwnedSource {
                id: source_id,
                seat,
                text: arc_text,
                bytes,
            });
            effects.push(Effect::SetSelection { seat, source_id, serial: serial_val });
        }
        Event::SourceSend { source_id, mime_type, fd_id } => {
            if state.availability != Availability::Available
                || !is_offered_mime(&mime_type)
                || state.active_writers >= MAX_CONCURRENT_WRITERS
            {
                effects.push(Effect::CloseFd { fd_id });
                return effects;
            }
            let Some(bytes) = state.source_bytes(source_id) else {
                effects.push(Effect::CloseFd { fd_id });
                return effects;
            };
            state.active_writers += 1;
            effects.push(Effect::SpawnWriter { fd_id, bytes });
        }
        Event::WriterFinished => {
            state.active_writers = state.active_writers.saturating_sub(1);
        }
        Event::SourceCancelled { source_id } => {
            if state.active_source.as_ref().is_some_and(|s| s.id == source_id) {
                state.active_source = None;
                effects.push(Effect::DestroySource { source_id });
                effects.push(Effect::SelectionLost);
                return effects;
            }
            if let Some(pos) = state.retired_sources.iter().position(|s| s.id == source_id) {
                state.retired_sources.remove(pos);
                effects.push(Effect::DestroySource { source_id });
            }
        }
        Event::RequestRead { target, preferred_seat } => {
            if state.availability != Availability::Available {
                return effects;
            }
            if let Some(prev) = state.active_read.take() {
                effects.push(Effect::CancelRead { attempt: prev.attempt });
            }
            // Zero-syscall self-paste fast path while our own source is actively owned.
            if let Some(owned) = &state.active_source {
                effects.push(Effect::DeliverText {
                    attempt: None,
                    target,
                    text: Arc::clone(&owned.text),
                });
                return effects;
            }
            let Some((offer_id, mime, encoding)) = state.pick_selection_offer(preferred_seat) else {
                return effects;
            };
            let attempt = state.read_seq.next();
            state.active_read = Some(ActiveRead { attempt, target, encoding });
            effects.push(Effect::StartRead { attempt, offer_id, mime });
        }
        Event::ReadFinished { attempt, outcome } => {
            let matches = state.active_read.as_ref().is_some_and(|r| r.attempt == attempt);
            if !matches {
                return effects;
            }
            let active = state.active_read.take().unwrap();
            if let ReadOutcome::Bytes(bytes) = outcome
                && let Some(text) = decode_text(bytes, active.encoding)
            {
                effects.push(Effect::DeliverText {
                    attempt: Some(attempt),
                    target: active.target,
                    text: Arc::from(text),
                });
            }
        }
        Event::DisplayLost | Event::Shutdown => {
            state.availability = Availability::Dead;
            if let Some(active) = state.active_read.take() {
                effects.push(Effect::CancelRead { attempt: active.attempt });
            }
            let mut seat_ids: Vec<SeatId> = state.seats.keys().copied().collect();
            seat_ids.sort_unstable();
            for seat in seat_ids {
                if let Some(mut info) = state.seats.remove(&seat) {
                    while let Some(offer) = info.pending_offers.pop_front() {
                        effects.push(Effect::DestroyOffer { offer_id: offer.id });
                    }
                    if let Some(offer) = info.selection.take() {
                        effects.push(Effect::DestroyOffer { offer_id: offer.id });
                    }
                    effects.push(Effect::DestroyDevice { seat });
                }
            }
            if let Some(owned) = state.active_source.take() {
                effects.push(Effect::DestroySource { source_id: owned.id });
            }
            while let Some(retired) = state.retired_sources.pop_front() {
                effects.push(Effect::DestroySource { source_id: retired.id });
            }
        }
    }
    effects
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
