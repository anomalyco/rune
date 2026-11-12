//! What a virtual list keeps for text selection between commits: the rows a selection pinned,
//! their selectable texts, and the source every unbuilt row is built from. They change together
//! in [`Pins::sync`], so a commit and a selection change can't leave them disagreeing.

use std::collections::HashMap;

use crate::el::{El, SelectionCopy, TextSpec};

/// Where an offscreen row comes from: the view's row builder, or the row the view built itself
/// and the list kept by index. One or the other, never both.
pub(crate) enum Source {
    /// `El::virtual_rows`: a row is built when selection needs it.
    Rebuild(std::rc::Rc<crate::el::virtual_rows::RowSource>),
    /// Rows the view built; the ones outside the window are kept in
    /// [`PinnedSelection::stored`].
    Stored,
}

impl Default for Source {
    fn default() -> Self {
        Source::Stored
    }
}

impl Source {
    /// The row builder, when rows are built by index.
    pub(crate) fn rebuild(&self) -> Option<&std::rc::Rc<crate::el::virtual_rows::RowSource>> {
        match self {
            Source::Rebuild(rows) => Some(rows),
            Source::Stored => None,
        }
    }
}

/// One selectable text inside an offscreen virtual row.
#[derive(Clone, Debug)]
pub(crate) struct OffscreenText {
    pub list_path: String,
    pub row_key: String,
    pub row_index: usize,
    pub order_in_row: usize,
    pub rel_path: String,
    pub path: String,
    pub key: Option<String>,
    pub group_path: Option<String>,
    pub spec: TextSpec,
    /// True when this text and the previous selectable text in the same row share a horizontal
    /// (`FlexDirection::Row | FlexDirection::RowReverse`) lowest common ancestor (table cells).
    pub tab_before: bool,
}

/// What the last commit collected offscreen rows under: the selection group they belong to and
/// whether the list is inert. They are written together, so they always describe one commit.
#[derive(Clone, Debug, Default)]
pub(crate) struct Scope {
    pub group_path: Option<String>,
    pub inert: bool,
}

/// Offscreen selection state retained on a [`crate::el::virtual_list::VirtualList`].
#[derive(Default)]
pub(crate) struct PinnedSelection {
    /// The pinned rows, their texts and the source every unbuilt row comes from.
    pub pins: Pins,
    /// The rows the view built, by index, while the pins' source is [`Source::Stored`]. Cleared
    /// in place at every commit, so its capacity carries over as dev's did, and kept beside the
    /// pins so a source change doesn't drop it.
    pub stored: HashMap<usize, El>,
    pub scope: Scope,
}

impl std::fmt::Debug for PinnedSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedSelection")
            .field("rows", &self.pins.rows().len())
            .field("texts", &self.pins.texts().len())
            .field("group_path", &self.scope.group_path)
            .field("inert", &self.scope.inert)
            .finish()
    }
}

/// What a list keeps for text selection. One state, so the pinned rows, their texts and the row
/// source are cleared and filled together.
#[derive(Default)]
pub(crate) enum Pins {
    /// The list has not committed yet: nothing pinned and no source to build a row from.
    #[default]
    None,
    /// The selection's rows in this list, the selectable texts built from them, and the source
    /// every unbuilt row comes from. `rows` is empty when the list has a source but nothing in it
    /// is selected.
    Pinned {
        /// The selection's anchor and focus rows, anchor first, deduplicated.
        rows: Vec<String>,
        /// Selectable texts of `rows` that were offscreen when the commit built them.
        texts: Vec<OffscreenText>,
        /// `selection_copy` providers found beside `texts`.
        providers: Vec<(String, SelectionCopy)>,
        source: Source,
    },
}

/// What changes what a list keeps.
pub(crate) enum PinsEvent<'a> {
    /// No text is selected: drop the rows and their texts. The source stays.
    Clear,
    /// The selection's anchor and focus rows in this list (each `None` when that end is not a row
    /// of it). They become the pins; texts of other rows go.
    Select {
        anchor: Option<&'a str>,
        focus: Option<&'a str>,
    },
    /// A selection request pinned `key` and built the texts for it.
    Pin {
        key: String,
        texts: Vec<OffscreenText>,
        providers: Vec<(String, SelectionCopy)>,
    },
}

impl Pins {
    pub(crate) fn rows(&self) -> &[String] {
        match self {
            Pins::None => &[],
            Pins::Pinned { rows, .. } => rows,
        }
    }

    pub(crate) fn texts(&self) -> &[OffscreenText] {
        match self {
            Pins::None => &[],
            Pins::Pinned { texts, .. } => texts,
        }
    }

    pub(crate) fn source(&self) -> Option<&Source> {
        match self {
            Pins::None => None,
            Pins::Pinned { source, .. } => Some(source),
        }
    }

    /// Whether row `key`'s texts are already kept.
    pub(crate) fn has_text_row(&self, key: &str) -> bool {
        self.texts().iter().any(|t| t.row_key == key)
    }

    /// Clears the kept texts and providers in place at the start of a commit. The vectors keep
    /// their allocations, and a builder that panics partway leaves what it already collected.
    pub(crate) fn clear_texts(&mut self) {
        if let Pins::Pinned { texts, providers, .. } = self {
            texts.clear();
            providers.clear();
        }
    }

    /// The vectors a commit collects a built row's texts into, borrowed from the state so a
    /// panic, an early return or a fallback keeps what came before.
    pub(crate) fn text_buffers(
        &mut self,
    ) -> Option<(&mut Vec<OffscreenText>, &mut Vec<(String, SelectionCopy)>)> {
        match self {
            Pins::None => None,
            Pins::Pinned { texts, providers, .. } => Some((texts, providers)),
        }
    }

    /// Installs the commit's row source, keeping the kept rows, texts and providers.
    pub(crate) fn set_source(&mut self, source: Source) {
        match self {
            Pins::None => {
                *self = Pins::Pinned {
                    rows: Vec::new(),
                    texts: Vec::new(),
                    providers: Vec::new(),
                    source,
                };
            }
            Pins::Pinned { source: current, .. } => *current = source,
        }
    }

    /// Applies `event`, keeping the rows, their texts and the source one state that a commit and
    /// a selection change can't leave disagreeing.
    pub(crate) fn sync(&mut self, event: PinsEvent<'_>) {
        match event {
            PinsEvent::Clear => {
                if let Pins::Pinned { rows, texts, providers, .. } = self {
                    rows.clear();
                    texts.clear();
                    providers.clear();
                }
            }
            PinsEvent::Select { anchor, focus } => {
                let (rows, texts, _, _) = self.fields();
                rows.clear();
                if let Some(key) = anchor {
                    rows.push(key.to_owned());
                }
                if let Some(key) = focus
                    && !rows.iter().any(|r| r == key)
                {
                    rows.push(key.to_owned());
                }
                // Dev kept the copy providers across a selection change; only a commit and a
                // cleared selection drop them.
                texts.retain(|t| rows.iter().any(|r| r == &t.row_key));
            }
            PinsEvent::Pin { key, texts, providers } => {
                let (rows, current, current_providers, _) = self.fields();
                if !rows.iter().any(|r| r == &key) {
                    rows.push(key);
                }
                current.extend(texts);
                current_providers.extend(providers);
            }
        }
    }

    /// The mutable parts, turning `None` into a pinned state with a default source when a
    /// selection needs one.
    fn fields(
        &mut self,
    ) -> (
        &mut Vec<String>,
        &mut Vec<OffscreenText>,
        &mut Vec<(String, SelectionCopy)>,
        &mut Source,
    ) {
        if let Pins::None = self {
            *self = Pins::Pinned {
                rows: Vec::new(),
                texts: Vec::new(),
                providers: Vec::new(),
                source: Source::default(),
            };
        }
        let Pins::Pinned { rows, texts, providers, source } = self else { unreachable!() };
        (rows, texts, providers, source)
    }
}

#[cfg(test)]
#[path = "pins_tests.rs"]
mod tests;
