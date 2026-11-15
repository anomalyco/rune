//! Searchable label runs from a laid-out Mermaid diagram, not the syntax that produced it.

use std::rc::Rc;
use rune::{Color, TextHighlight};
use crate::{DiagramStyle, Drawing, Hover, canvas::Scene, scenes};

/// One prepared layout. Its title and labels can be indexed once, then drawn again with
/// different text highlights without parsing the source or laying the diagram out again.
#[derive(Clone)]
pub struct DrawingModel {
    scene: Rc<Scene>,
    style: DiagramStyle,
}

/// Prepare the same layout as [`crate::render_drawing`], retaining it for text lookup.
pub fn prepare_drawing(src: &str, style: &DiagramStyle, hover: Hover, color: Color) -> Option<DrawingModel> {
    let hover = (hover != Hover::Off).then_some((hover, color));
    Some(DrawingModel { scene: scenes::built(src, style, hover, None).0?, style: *style })
}

impl DrawingModel {
    /// `(element key, shown text)` in drawing order. Includes a displayed title, but not
    /// node IDs, source comments or the duplicate paint used to light a label on hover.
    pub fn labels(&self) -> impl Iterator<Item = (&str, &str)> {
        self.scene.title.as_deref().map(|s| ("title", s)).into_iter().chain(
            self.scene.label_keys.iter().zip(&self.scene.labels).map(|((key, _), l)| (key.as_str(), l.text.as_str()))
        )
    }

    /// Whether text ranges for `other` still describe this exact layout.
    pub fn same_layout(&self, other: &Self) -> bool { Rc::ptr_eq(&self.scene, &other.scene) }

    /// Draw with UTF-8 byte ranges local to each label. Keys are the ones returned by
    /// [`Self::labels`]. A hover copy uses the same marks as the label underneath it.
    pub fn draw(&self, marks: impl Fn(&str) -> Vec<TextHighlight>) -> Drawing {
        let (el, size) = self.scene.drawing(&self.style, Some(&marks));
        Drawing { el, size, surface: self.style.surface, surface_border: self.style.surface_border }
    }
}
