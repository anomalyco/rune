use std::error::Error;
use std::mem;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use tiny_skia::{
    BlendMode, Color, FillRule, Mask, Paint, Path, PathBuilder, Pixmap, PixmapMut, PixmapPaint,
    Point, Rect, Shader, Transform,
};

use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::protocol::wl_shm;
use smithay_client_toolkit::reexports::client::protocol::wl_subsurface::WlSubsurface;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Dispatch, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::csd_frame::{
    CursorIcon, DecorationsFrame, FrameAction, FrameClick, WindowManagerCapabilities, WindowState,
};

use smithay_client_toolkit::compositor::{CompositorState, Region, SurfaceData};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::{slot::SlotPool, Shm};
use smithay_client_toolkit::subcompositor::SubcompositorState;
use smithay_client_toolkit::subcompositor::SubsurfaceData;

mod buttons;
mod config;
mod parts;
#[cfg(test)]
mod tests;
mod pointer;
mod shadow;
pub mod theme;
mod title;
mod wl_typed;

use crate::theme::{
    ColorMap, ColorTheme, BORDER_SIZE, CORNER_RADIUS, HEADER_SIZE, RESIZE_HANDLE_CORNER_SIZE,
    VISIBLE_BORDER_SIZE,
};

use buttons::Buttons;
use config::get_button_layout_config;
pub use config::read_system_config_early;
use parts::DecorationParts;
use pointer::{Location, MouseState};
use shadow::Shadow;
use title::TitleText;
use wl_typed::WlTyped;

/// XXX this is not result, so `must_use` when needed.
type SkiaResult = Option<()>;

/// A simple set of decorations
#[derive(Debug)]
pub struct AdwaitaFrame<State> {
    /// The base surface used to create the window.
    base_surface: WlTyped<WlSurface, SurfaceData>,

    compositor: Arc<CompositorState>,

    /// Subcompositor to create/drop subsurfaces ondemand.
    subcompositor: Arc<SubcompositorState>,

    /// Queue handle to perform object creation.
    queue_handle: QueueHandle<State>,

    /// The drawable decorations, `None` when hidden.
    decorations: Option<DecorationParts>,

    /// Memory pool to allocate the buffers for the decorations.
    pool: SlotPool,

    /// Whether the frame should be redrawn.
    dirty: bool,

    /// Whether the drawing should be synced with the main surface.
    should_sync: bool,

    /// Scale factor used for the surface.
    scale_factor: u32,

    /// Wether the frame is resizable.
    resizable: bool,

    buttons: Buttons,
    state: WindowState,
    wm_capabilities: WindowManagerCapabilities,
    mouse: MouseState,
    theme: ColorTheme,
    title: Option<String>,
    title_text: Option<TitleText>,
    shadow: Shadow,
}

impl<State> AdwaitaFrame<State>
where
    State: Dispatch<WlSurface, SurfaceData> + Dispatch<WlSubsurface, SubsurfaceData> + 'static,
{
    pub fn new(
        base_surface: &impl WaylandSurface,
        shm: &Shm,
        compositor: Arc<CompositorState>,
        subcompositor: Arc<SubcompositorState>,
        queue_handle: QueueHandle<State>,
        frame_config: FrameConfig,
    ) -> Result<Self, Box<dyn Error>> {
        let base_surface = WlTyped::wrap::<State>(base_surface.wl_surface().clone());

        let pool = SlotPool::new(1, shm)?;

        let decorations = Some(DecorationParts::new(
            &base_surface,
            &subcompositor,
            &queue_handle,
        ));

        let theme = frame_config.theme;

        Ok(AdwaitaFrame {
            base_surface,
            decorations,
            pool,
            compositor,
            subcompositor,
            queue_handle,
            dirty: true,
            scale_factor: 1,
            should_sync: true,
            title: None,
            title_text: TitleText::new(theme.active.font_color),
            theme,
            buttons: Buttons::new(get_button_layout_config()),
            mouse: Default::default(),
            state: WindowState::empty(),
            wm_capabilities: WindowManagerCapabilities::all(),
            resizable: true,
            shadow: Shadow::default(),
        })
    }

    /// Update the current frame config.
    pub fn set_config(&mut self, config: FrameConfig) {
        self.theme = config.theme;
        self.dirty = true;
    }

    /// The radius, in logical points, of the arc this frame leaves the window's content at its
    /// bottom corners: the visible border is drawn within one point outside it. Zero when the
    /// frame draws the window square, as a maximized, tiled or fullscreen one is (and as the
    /// headerbar's top corners are).
    pub fn content_corner_radius(&self) -> f32 {
        content_radius(self.state)
    }

    fn precise_location(
        &self,
        location: Location,
        decoration: &DecorationParts,
        x: f64,
        y: f64,
    ) -> Location {
        let header_width = decoration.header().surface_rect.width;
        let side_height = decoration.side_height();

        let left_corner_x = BORDER_SIZE + RESIZE_HANDLE_CORNER_SIZE;
        let right_corner_x = (header_width + BORDER_SIZE).saturating_sub(RESIZE_HANDLE_CORNER_SIZE);
        let top_corner_y = RESIZE_HANDLE_CORNER_SIZE;
        let bottom_corner_y = side_height.saturating_sub(RESIZE_HANDLE_CORNER_SIZE);
        match location {
            Location::Head | Location::Button(_) => self.buttons.find_button(x, y),
            Location::Top | Location::TopLeft | Location::TopRight => {
                if x <= f64::from(left_corner_x) {
                    Location::TopLeft
                } else if x >= f64::from(right_corner_x) {
                    Location::TopRight
                } else {
                    Location::Top
                }
            }
            Location::Bottom | Location::BottomLeft | Location::BottomRight => {
                if x <= f64::from(left_corner_x) {
                    Location::BottomLeft
                } else if x >= f64::from(right_corner_x) {
                    Location::BottomRight
                } else {
                    Location::Bottom
                }
            }
            Location::Left => {
                if y <= f64::from(top_corner_y) {
                    Location::TopLeft
                } else if y >= f64::from(bottom_corner_y) {
                    Location::BottomLeft
                } else {
                    Location::Left
                }
            }
            Location::Right => {
                if y <= f64::from(top_corner_y) {
                    Location::TopRight
                } else if y >= f64::from(bottom_corner_y) {
                    Location::BottomRight
                } else {
                    Location::Right
                }
            }
            other => other,
        }
    }

    fn redraw_inner(&mut self) -> Option<bool> {
        let decorations = self.decorations.as_mut()?;

        // Reset the dirty bit.
        self.dirty = false;
        let should_sync = mem::take(&mut self.should_sync);

        // Don't draw borders if the frame explicitly hidden or fullscreened.
        if self.state.contains(WindowState::FULLSCREEN) {
            decorations.hide();
            return Some(true);
        }

        let colors = if self.state.contains(WindowState::ACTIVATED) {
            &self.theme.active
        } else {
            &self.theme.inactive
        };

        let draw_borders = if self.state.contains(WindowState::MAXIMIZED) {
            // Don't draw the borders.
            decorations.hide_borders();
            false
        } else {
            true
        };

        // Draw the borders.
        for (idx, part) in decorations
            .parts()
            .filter(|(idx, _)| *idx == DecorationParts::HEADER || draw_borders)
        {
            let scale = self.scale_factor;

            let mut rect = part.surface_rect;
            // XXX to perfectly align the visible borders we draw them with
            // the header, otherwise rounded corners won't look 'smooth' at the
            // start. To achieve that, we enlargen the width of the header by
            // 2 * `VISIBLE_BORDER_SIZE`, and move `x` by `VISIBLE_BORDER_SIZE`
            // to the left.
            if idx == DecorationParts::HEADER && draw_borders {
                rect.width += 2 * VISIBLE_BORDER_SIZE;
                rect.x -= VISIBLE_BORDER_SIZE as i32;
            }

            rect.width *= scale;
            rect.height *= scale;

            let (buffer, canvas) = match self.pool.create_buffer(
                rect.width as i32,
                rect.height as i32,
                rect.width as i32 * 4,
                wl_shm::Format::Argb8888,
            ) {
                Ok((buffer, canvas)) => (buffer, canvas),
                Err(_) => continue,
            };

            // Create the pixmap and fill with transparent color.
            let mut pixmap = PixmapMut::from_bytes(canvas, rect.width, rect.height)?;

            // Fill everything with transparent background, since we draw rounded corners and
            // do invisible borders to enlarge the input zone.
            pixmap.fill(Color::TRANSPARENT);

            // The bottom and side parts draw their shadow along with their border: the bottom one
            // is the part that reaches over the window, so its corners are cut out of the shadow it
            // draws there, and a side part's border stops where the bottom corner's arc takes over.
            if !matches!(
                idx,
                DecorationParts::BOTTOM | DecorationParts::LEFT | DecorationParts::RIGHT
            ) && !self.state.intersects(WindowState::TILED)
            {
                self.shadow.draw(
                    &mut pixmap,
                    scale,
                    self.state.contains(WindowState::ACTIVATED),
                    idx,
                    // The other parts have no rounded corners of their own to reach over the
                    // window with.
                    0.,
                );
            }

            match idx {
                DecorationParts::HEADER => {
                    if let Some(title_text) = self.title_text.as_mut() {
                        title_text.update_scale(scale);
                        title_text.update_color(colors.font_color);
                    }

                    draw_headerbar(
                        &mut pixmap,
                        self.title_text.as_ref().map(|t| t.pixmap()).unwrap_or(None),
                        scale as f32,
                        self.resizable,
                        &self.state,
                        &self.theme,
                        &self.buttons,
                        self.mouse.location,
                    );
                }
                DecorationParts::BOTTOM => {
                    draw_bottom(
                        &mut pixmap,
                        &mut self.shadow,
                        colors,
                        &self.state,
                        scale,
                        rect.width,
                    );
                }
                DecorationParts::LEFT => {
                    draw_side(&mut pixmap, &mut self.shadow, colors, &self.state, scale, false);
                }
                DecorationParts::RIGHT => {
                    draw_side(&mut pixmap, &mut self.shadow, colors, &self.state, scale, true);
                }
                _ => {}
            };

            if should_sync {
                part.subsurface.set_sync();
            } else {
                part.subsurface.set_desync();
            }

            part.surface.set_buffer_scale(scale as i32);

            part.subsurface.set_position(rect.x, rect.y);
            buffer.attach_to(&part.surface).ok()?;

            if part.surface.version() >= 4 {
                part.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            } else {
                part.surface.damage(0, 0, i32::MAX, i32::MAX);
            }

            if let Some(input_rect) = part.input_rect {
                let input_region = Region::new(&*self.compositor).ok()?;
                input_region.add(
                    input_rect.x,
                    input_rect.y,
                    input_rect.width as i32,
                    input_rect.height as i32,
                );

                part.surface
                    .set_input_region(Some(input_region.wl_region()));
            }

            part.surface.commit();
        }

        Some(should_sync)
    }
}

impl<State> DecorationsFrame for AdwaitaFrame<State>
where
    State: Dispatch<WlSurface, SurfaceData> + Dispatch<WlSubsurface, SubsurfaceData> + 'static,
{
    fn update_state(&mut self, state: WindowState) {
        let difference = self.state.symmetric_difference(state);
        self.state = state;
        self.dirty |= difference.intersects(
            WindowState::ACTIVATED
                | WindowState::FULLSCREEN
                | WindowState::MAXIMIZED
                | WindowState::TILED,
        );
    }

    fn update_wm_capabilities(&mut self, wm_capabilities: WindowManagerCapabilities) {
        self.dirty |= self.wm_capabilities != wm_capabilities;
        self.wm_capabilities = wm_capabilities;
        self.buttons.update_wm_capabilities(wm_capabilities);
    }

    fn set_hidden(&mut self, hidden: bool) {
        if hidden {
            self.dirty = false;
            let _ = self.pool.resize(1);
            self.decorations = None;
        } else if self.decorations.is_none() {
            self.decorations = Some(DecorationParts::new(
                &self.base_surface,
                &self.subcompositor,
                &self.queue_handle,
            ));
            self.dirty = true;
            self.should_sync = true;
        }
    }

    fn set_resizable(&mut self, resizable: bool) {
        self.dirty |= self.resizable != resizable;
        self.resizable = resizable;
    }

    fn resize(&mut self, width: NonZeroU32, height: NonZeroU32) {
        let Some(decorations) = self.decorations.as_mut() else {
            log::error!("trying to resize the hidden frame.");
            return;
        };

        decorations.resize(width.get(), height.get());
        self.buttons
            .arrange(width.get(), get_margin_h_lp(&self.state));
        self.dirty = true;
        self.should_sync = true;
    }

    fn draw(&mut self) -> bool {
        self.redraw_inner().unwrap_or(true)
    }

    fn subtract_borders(
        &self,
        width: NonZeroU32,
        height: NonZeroU32,
    ) -> (Option<NonZeroU32>, Option<NonZeroU32>) {
        if self.decorations.is_none() || self.state.contains(WindowState::FULLSCREEN) {
            (Some(width), Some(height))
        } else {
            (
                Some(width),
                NonZeroU32::new(height.get().saturating_sub(HEADER_SIZE)),
            )
        }
    }

    fn add_borders(&self, width: u32, height: u32) -> (u32, u32) {
        if self.decorations.is_none() || self.state.contains(WindowState::FULLSCREEN) {
            (width, height)
        } else {
            (width, height + HEADER_SIZE)
        }
    }

    fn location(&self) -> (i32, i32) {
        if self.decorations.is_none() || self.state.contains(WindowState::FULLSCREEN) {
            (0, 0)
        } else {
            (0, -(HEADER_SIZE as i32))
        }
    }

    fn set_title(&mut self, title: impl Into<String>) {
        let new_title = title.into();
        if let Some(title_text) = self.title_text.as_mut() {
            title_text.update_title(new_title.clone());
        }

        self.title = Some(new_title);
        self.dirty = true;
    }

    fn on_click(
        &mut self,
        timestamp: Duration,
        click: FrameClick,
        pressed: bool,
    ) -> Option<FrameAction> {
        match click {
            FrameClick::Normal => self.mouse.click(
                timestamp,
                pressed,
                self.resizable,
                &self.state,
                &self.wm_capabilities,
            ),
            FrameClick::Alternate => self.mouse.alternate_click(pressed, &self.wm_capabilities),
            _ => None,
        }
    }

    fn set_scaling_factor(&mut self, scale_factor: f64) {
        // NOTE: Clamp it just in case to some ok-ish range.
        self.scale_factor = scale_factor.clamp(0.1, 64.).ceil() as u32;
        self.dirty = true;
        self.should_sync = true;
    }

    fn click_point_moved(
        &mut self,
        _timestamp: Duration,
        surface: &ObjectId,
        x: f64,
        y: f64,
    ) -> Option<CursorIcon> {
        let decorations = self.decorations.as_ref()?;
        let location = decorations.find_surface(surface);
        if location == Location::None {
            return None;
        }

        let old_location = self.mouse.location;

        let location = self.precise_location(location, decorations, x, y);
        let new_cursor = self.mouse.moved(location, x, y, self.resizable);

        // Set dirty if we moved the cursor between the buttons.
        self.dirty |= (matches!(old_location, Location::Button(_))
            || matches!(self.mouse.location, Location::Button(_)))
            && old_location != self.mouse.location;

        Some(new_cursor)
    }

    fn click_point_left(&mut self) {
        self.mouse.left()
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn is_hidden(&self) -> bool {
        self.decorations.is_none()
    }
}

/// The configuration for the [`AdwaitaFrame`] frame.
#[derive(Debug, Clone)]
pub struct FrameConfig {
    pub theme: ColorTheme,
}

impl FrameConfig {
    /// Create the new configuration with the given `theme`.
    pub fn new(theme: ColorTheme) -> Self {
        Self { theme }
    }

    /// This is equivalent of calling `FrameConfig::new(ColorTheme::auto())`.
    ///
    /// For details see [`ColorTheme::auto`].
    pub fn auto() -> Self {
        Self {
            theme: ColorTheme::auto(),
        }
    }

    /// This is equivalent of calling `FrameConfig::new(ColorTheme::light())`.
    ///
    /// For details see [`ColorTheme::light`].
    pub fn light() -> Self {
        Self {
            theme: ColorTheme::light(),
        }
    }

    /// This is equivalent of calling `FrameConfig::new(ColorTheme::dark())`.
    ///
    /// For details see [`ColorTheme::dark`].
    pub fn dark() -> Self {
        Self {
            theme: ColorTheme::dark(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_headerbar(
    pixmap: &mut PixmapMut,
    text_pixmap: Option<&Pixmap>,
    scale: f32,
    resizable: bool,
    state: &WindowState,
    theme: &ColorTheme,
    buttons: &Buttons,
    mouse: Location,
) {
    let colors = theme.for_state(state.contains(WindowState::ACTIVATED));

    let _ = draw_headerbar_bg(pixmap, scale, colors, state);

    // Horizontal margin.
    let margin_h = get_margin_h_lp(state) * 2.0;

    let canvas_w = pixmap.width() as f32;
    let canvas_h = pixmap.height() as f32;

    let header_w = canvas_w - margin_h * 2.0;
    let header_h = canvas_h;

    if let Some(text_pixmap) = text_pixmap {
        const TEXT_OFFSET: f32 = 10.;
        let offset_x = TEXT_OFFSET * scale;

        let text_w = text_pixmap.width() as f32;
        let text_h = text_pixmap.height() as f32;

        let x = margin_h + header_w / 2. - text_w / 2.;
        let y = header_h / 2. - text_h / 2.;

        let left_buttons_end_x = buttons.left_buttons_end_x().unwrap_or(0.0) * scale;
        let right_buttons_start_x =
            buttons.right_buttons_start_x().unwrap_or(header_w / scale) * scale;

        {
            // We have enough space to center text
            let (x, y, text_canvas_start_x) = if (x + text_w < right_buttons_start_x - offset_x)
                && (x > left_buttons_end_x + offset_x)
            {
                let text_canvas_start_x = x;

                (x, y, text_canvas_start_x)
            } else {
                let x = left_buttons_end_x + offset_x;
                let text_canvas_start_x = left_buttons_end_x + offset_x;

                (x, y, text_canvas_start_x)
            };

            let text_canvas_end_x = right_buttons_start_x - x - offset_x;
            // Ensure that text start within the bounds.
            let x = x.max(margin_h + offset_x);

            if let Some(clip) =
                Rect::from_xywh(text_canvas_start_x, 0., text_canvas_end_x, canvas_h)
            {
                if let Some(mut mask) = Mask::new(canvas_w as u32, canvas_h as u32) {
                    mask.fill_path(
                        &PathBuilder::from_rect(clip),
                        FillRule::Winding,
                        false,
                        Transform::identity(),
                    );
                    pixmap.draw_pixmap(
                        x.round() as i32,
                        y as i32,
                        text_pixmap.as_ref(),
                        &PixmapPaint::default(),
                        Transform::identity(),
                        Some(&mask),
                    );
                } else {
                    log::error!(
                        "Invalid mask width and height: w: {}, h: {}",
                        canvas_w as u32,
                        canvas_h as u32
                    );
                }
            }
        }
    }

    // Draw the buttons.
    buttons.draw(
        margin_h, header_w, scale, colors, mouse, pixmap, resizable, state,
    );
}

#[must_use]
fn draw_headerbar_bg(
    pixmap: &mut PixmapMut,
    scale: f32,
    colors: &ColorMap,
    state: &WindowState,
) -> SkiaResult {
    let w = pixmap.width() as f32;
    let h = pixmap.height() as f32;

    let radius = if state.intersects(WindowState::MAXIMIZED | WindowState::TILED) {
        0.
    } else {
        CORNER_RADIUS as f32 * scale
    };

    let bg = rounded_headerbar_shape(0., 0., w, h, radius)?;

    pixmap.fill_path(
        &bg,
        &colors.headerbar_paint(),
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    pixmap.fill_rect(
        Rect::from_xywh(0., h - 1., w, h)?,
        &colors.border_paint(),
        Transform::identity(),
        None,
    );

    Some(())
}

fn rounded_headerbar_shape(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Option<Path> {
    // https://stackoverflow.com/a/27863181
    let cubic_bezier_circle = 0.552_284_8 * radius;

    let mut pb = PathBuilder::new();
    let mut cursor = Point::from_xy(x, y);

    // !!!
    // This code is heavily "inspired" by https://gitlab.com/snakedye/snui/
    // So technically it should be licensed under MPL-2.0, sorry about that 🥺 👉👈
    // !!!

    // Positioning the cursor
    cursor.y += radius;
    pb.move_to(cursor.x, cursor.y);

    // Drawing the outline
    let next = Point::from_xy(cursor.x + radius, cursor.y - radius);
    pb.cubic_to(
        cursor.x,
        cursor.y - cubic_bezier_circle,
        next.x - cubic_bezier_circle,
        next.y,
        next.x,
        next.y,
    );
    cursor = next;
    pb.line_to(
        {
            cursor.x = x + width - radius;
            cursor.x
        },
        cursor.y,
    );
    let next = Point::from_xy(cursor.x + radius, cursor.y + radius);
    pb.cubic_to(
        cursor.x + cubic_bezier_circle,
        cursor.y,
        next.x,
        next.y - cubic_bezier_circle,
        next.x,
        next.y,
    );
    cursor = next;
    pb.line_to(cursor.x, {
        cursor.y = y + height;
        cursor.y
    });
    pb.line_to(
        {
            cursor.x = x;
            cursor.x
        },
        cursor.y,
    );

    pb.close();

    pb.finish()
}

/// The radius, in logical points, of the arc a frame leaves the window's content at its bottom
/// corners. The frame's one-point visible border is drawn just outside that arc, so the window's
/// own corners are [`VISIBLE_BORDER_SIZE`] more, and the shadow's rounded cut-out is the same
/// disc.
pub const CONTENT_CORNER_RADIUS: u32 = CORNER_RADIUS - VISIBLE_BORDER_SIZE;

/// The radius, in logical points, of the arc this frame leaves the window's content at its bottom
/// corners in `state`: zero when the window is square, as a maximized, tiled or fullscreen one is.
fn content_radius(state: WindowState) -> f32 {
    if square(state) {
        0.
    } else {
        CONTENT_CORNER_RADIUS as f32
    }
}

/// The radius, in logical points, of the visible border's outer edge around the window's bottom
/// corners in `state`: [`CORNER_RADIUS`], or zero when the window is square.
fn border_radius(state: WindowState) -> f32 {
    if square(state) {
        0.
    } else {
        CORNER_RADIUS as f32
    }
}

/// Whether `state` draws the window's corners square, as the headerbar's top ones are: maximized,
/// tiled and fullscreen windows have no corners to round.
fn square(state: WindowState) -> bool {
    state.intersects(WindowState::MAXIMIZED | WindowState::TILED | WindowState::FULLSCREEN)
}

/// Draws the bottom part: its shadow, the arcs cut out of it where the content's own rounded
/// corners show through, and the visible border around them.
fn draw_bottom(
    pixmap: &mut PixmapMut,
    shadow: &mut Shadow,
    colors: &ColorMap,
    state: &WindowState,
    scale: u32,
    width: u32,
) {
    let border = (VISIBLE_BORDER_SIZE * scale) as f32;
    // The part is placed one corner radius above the window's bottom edge, and spans it plus the
    // shadow and border around the window: `left` and `right` are the window's outer edges.
    let left = (BORDER_SIZE * scale) as f32 - border;
    let right = width as f32 - left;
    let bottom = (CORNER_RADIUS * scale) as f32;
    let radius = border_radius(*state) * scale as f32;

    if !state.intersects(WindowState::TILED) {
        shadow.draw(pixmap, scale, state.contains(WindowState::ACTIVATED), DecorationParts::BOTTOM, radius);
    }

    if radius < border {
        if let Some(rect) = Rect::from_xywh(left, bottom, (right - left).max(0.), border) {
            pixmap.fill_rect(rect, &colors.border_paint(), Transform::identity(), None);
        }
        return;
    }
    // The band above the window's bottom edge carries the shadow's corner crescents, so the arcs
    // are cleared out of it before the border is drawn along them: the shadow stays outside the
    // arcs the content leaves, and nothing stays where the content shows through.
    clear_bottom_corner_holes(pixmap, left, right, bottom, border, radius);
    if let Some(shape) = bottom_border_shape(left, right, bottom, border, radius) {
        pixmap.fill_path(&shape, &colors.border_paint(), FillRule::EvenOdd, Transform::identity(), None);
    }
}

/// Draws a side part (`LEFT` or `RIGHT`): the shadow down that side of the window, and the visible
/// border along it, from under the headerbar to where the bottom corner's arc takes over.
fn draw_side(
    pixmap: &mut PixmapMut,
    shadow: &mut Shadow,
    colors: &ColorMap,
    state: &WindowState,
    scale: u32,
    right: bool,
) {
    let idx = if right { DecorationParts::RIGHT } else { DecorationParts::LEFT };
    if !state.intersects(WindowState::TILED) {
        shadow.draw(pixmap, scale, state.contains(WindowState::ACTIVATED), idx, 0.);
    }
    let border = (VISIBLE_BORDER_SIZE * scale) as f32;
    let top = (HEADER_SIZE * scale) as f32;
    let corner = (border_radius(*state) * scale as f32 - border).max(0.);
    let height = (pixmap.height() as f32 - top - corner).max(0.);
    let x = if right { 0. } else { (BORDER_SIZE * scale) as f32 - border };
    if let Some(rect) = Rect::from_xywh(x, top, border, height) {
        pixmap.fill_rect(rect, &colors.border_paint(), Transform::identity(), None);
    }
}

/// Clears the two discs of radius `radius` the bottom part's shadow is cut out of, so the part
/// leaves the content's rounded corners alone.
fn clear_bottom_corner_holes(pixmap: &mut PixmapMut, left: f32, right: f32, bottom: f32, border: f32, radius: f32) {
    // Each arc's centre is the content's: `border` up from the content's bottom edge, `radius` in
    // from the window's edge.
    let centre_y = bottom + border - radius;
    let paint = Paint {
        shader: Shader::SolidColor(Color::TRANSPARENT),
        blend_mode: BlendMode::Clear,
        anti_alias: true,
        ..Paint::default()
    };
    for centre_x in [left + radius, right - radius] {
        if let Some(circle) = circle_path(centre_x, centre_y, radius) {
            pixmap.fill_path(&circle, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// A circle of `radius` around `(cx, cy)`, as four cubic arcs.
fn circle_path(cx: f32, cy: f32, radius: f32) -> Option<Path> {
    if radius <= 0. {
        return None;
    }
    // https://stackoverflow.com/a/27863181, as `rounded_headerbar_shape`.
    let k = 0.552_284_8 * radius;
    let mut pb = PathBuilder::new();
    pb.move_to(cx + radius, cy);
    pb.cubic_to(cx + radius, cy + k, cx + k, cy + radius, cx, cy + radius);
    pb.cubic_to(cx - k, cy + radius, cx - radius, cy + k, cx - radius, cy);
    pb.cubic_to(cx - radius, cy - k, cx - k, cy - radius, cx, cy - radius);
    pb.cubic_to(cx + k, cy - radius, cx + radius, cy - k, cx + radius, cy);
    pb.close();
    pb.finish()
}

/// The visible border under a window whose bottom corners are rounded with `radius`: a one-point
/// band along `bottom`, the content's bottom edge, from `left` to `right`, the window's outer
/// edges, curving around each corner outside the arc the content leaves (centred one radius inside
/// the window's corner, so the band sits between that arc and the shadow). A window too narrow for
/// both arcs rounds them as far as it can.
fn bottom_border_shape(
    left: f32,
    right: f32,
    bottom: f32,
    border: f32,
    radius: f32,
) -> Option<Path> {
    if !(left.is_finite() && right.is_finite() && bottom.is_finite() && border.is_finite() && radius.is_finite())
        || border <= 0.
        || right <= left
    {
        return None;
    }
    // https://stackoverflow.com/a/27863181, as `rounded_headerbar_shape`.
    let k = 0.552_284_8;
    let radius = radius.min((right - left) / 2.).max(border);
    let outer = bottom + border;
    let (outer_radius, inner_radius) = (radius, radius - border);
    // One disc per corner: its centre is the content's arc centre, `outer` less the outer radius.
    let centre_y = outer - outer_radius;
    let (left_centre, right_centre) = (left + outer_radius, right - outer_radius);

    let mut pb = PathBuilder::new();
    // The window's outline: down the left edge, around the corner, along the bottom, around the
    // other corner, and up the right edge.
    pb.move_to(left, 0.);
    pb.line_to(left, centre_y);
    pb.cubic_to(
        left,
        centre_y + k * outer_radius,
        left_centre - k * outer_radius,
        outer,
        left_centre,
        outer,
    );
    pb.line_to(right_centre, outer);
    pb.cubic_to(
        right_centre + k * outer_radius,
        outer,
        right,
        centre_y + k * outer_radius,
        right,
        centre_y,
    );
    pb.line_to(right, 0.);
    pb.close();
    // And the content's, one point inside it: filling both even-odd leaves the border between.
    pb.move_to(left + border, 0.);
    pb.line_to(left + border, centre_y);
    pb.cubic_to(
        left + border,
        centre_y + k * inner_radius,
        left_centre - k * inner_radius,
        bottom,
        left_centre,
        bottom,
    );
    pb.line_to(right_centre, bottom);
    pb.cubic_to(
        right_centre + k * inner_radius,
        bottom,
        right - border,
        centre_y + k * inner_radius,
        right - border,
        centre_y,
    );
    pb.line_to(right - border, 0.);
    pb.close();

    pb.finish()
}

// returns horizontal margin, logical points
fn get_margin_h_lp(state: &WindowState) -> f32 {
    if state.intersects(WindowState::MAXIMIZED | WindowState::TILED) {
        0.
    } else {
        VISIBLE_BORDER_SIZE as f32
    }
}
