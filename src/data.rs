//! Application state.

use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use druid::kurbo::{Affine, BezPath, Point, Rect, Size};
use druid::{Data, WindowId};
use norad::glyph::{Contour, ContourPoint, Glyph, GlyphName, PointType};
use norad::{FontInfo, MetaInfo, Ufo};

use crate::edit_session::EditSession;

/// This is by convention.
const DEFAULT_UNITS_PER_EM: f64 = 1000.;

#[derive(Clone, Data, Default)]
pub struct AppState {
    pub file: Arc<FontObject>,
    /// glyphs that are already open in an editor window
    pub open_glyphs: Arc<HashMap<GlyphName, WindowId>>,
    pub sessions: Arc<HashMap<GlyphName, Arc<EditSession>>>,
}

#[derive(Clone, Data)]
pub struct FontObject {
    pub path: Option<Arc<Path>>,
    #[druid(ignore)]
    pub ufo: Ufo,
    placeholder: Arc<BezPath>,
}

/// The main data type for the grid view.
#[derive(Clone, Data)]
pub struct GlyphSet {
    pub font: Arc<FontObject>,
    pub sessions: Arc<HashMap<GlyphName, Arc<EditSession>>>,
}

/// A glyph, plus access to the main UFO in order to resolve components in that
/// glyph.
#[derive(Clone, Data)]
pub struct GlyphPlus {
    pub glyph: Arc<Glyph>,
    outline: Arc<BezPath>,
}

/// Things in `FontInfo` that are relevant while editing or drawing.
#[derive(Clone, Data)]
pub struct FontMetrics {
    pub units_per_em: f64,
    pub descender: Option<f64>,
    pub x_height: Option<f64>,
    pub cap_height: Option<f64>,
    pub ascender: Option<f64>,
    pub italic_angle: Option<f64>,
}

/// The state for an editor view.
#[derive(Clone, Data)]
pub struct EditorState {
    pub metrics: FontMetrics,
    pub font: GlyphSet,
    pub session: Arc<EditSession>,
}

impl AppState {
    pub fn set_file(&mut self, ufo: Ufo, path: impl Into<Option<PathBuf>>) {
        let obj = FontObject {
            path: path.into().map(Into::into),
            ufo,
            placeholder: Arc::new(placeholder_outline()),
        };
        self.file = obj.into();
    }

    pub fn save(&mut self) -> Result<(), Box<dyn Error>> {
        let font_obj = Arc::make_mut(&mut self.file);
        if let Some(path) = font_obj.path.as_ref() {
            log::info!("saving to {:?}", path);
            // flush all open sessions
            for session in self.sessions.values() {
                let glyph = session.to_norad_glyph();
                font_obj
                    .ufo
                    .get_default_layer_mut()
                    .unwrap()
                    .insert_glyph(glyph);
            }
            let tmp_file_name = format!(
                "{}.savefile",
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled")
            );
            let tmp_path = path.with_file_name(tmp_file_name);
            if tmp_path.exists() {
                fs::remove_dir_all(&tmp_path)?;
            }
            font_obj.ufo.save(&tmp_path)?;
            if path.exists() {
                fs::remove_dir_all(path)?;
            }
            // see docs for fs::rename; the target directory must exist on unix.
            // http://doc.rust-lang.org/1.39.0/std/fs/fn.rename.html
            if cfg!(unix) {
                fs::create_dir(path)?;
            }
            fs::rename(&tmp_path, path)?;
        } else {
            log::error!("save called with no path set");
        }
        Ok(())
    }
}

impl GlyphSet {
    /// Given a glyph name, a `Ufo`, and an optional cache, returns the fully resolved
    /// (including all sub components) `BezPath` for this glyph.
    pub fn get_bezier(&self, name: &GlyphName) -> Option<Arc<BezPath>> {
        let glyph = self
            .sessions
            .get(name)
            .map(|s| &s.glyph)
            .or_else(|| self.font.ufo.get_glyph(name))?;
        let path = path_for_glyph(glyph)?;
        Some(self.resolve_components(glyph, path))
    }

    /// takes a glyph outline and appends the outlines of any components,
    /// resolving them as necessary, and caching the results.
    fn resolve_components(&self, glyph: &Glyph, mut bez: BezPath) -> Arc<BezPath> {
        for comp in glyph
            .outline
            .as_ref()
            .iter()
            .flat_map(|o| o.components.iter())
        {
            match self.get_bezier(&comp.base) {
                Some(comp_path) => {
                    let affine: Affine = comp.transform.clone().into();
                    for comp_elem in (affine * &*comp_path).elements() {
                        bez.push(*comp_elem);
                    }
                }
                None => log::warn!("missing component {} in glyph {}", comp.base, glyph.name),
            }
        }

        Arc::new(bez)
    }
}

impl GlyphPlus {
    /// Get the fully resolved (including components) bezier path for this glyph.
    ///
    /// Returns the placeholder glyph if this glyph has no outline.
    pub fn get_bezier(&self) -> Arc<BezPath> {
        self.outline.clone()
    }
}

impl EditorState {
    /// Returns a rect representing the metric bounds of this glyph; that is,
    /// taking into account the font metrics (ascender, descender) as well as the
    /// glyph's width.
    ///
    /// This rect is in the same coordinate space as the glyph; the origin
    /// is at the intersection of the baseline and the left sidebearing,
    /// and y is up.
    fn layout_bounds(&self) -> Rect {
        let upm = self.metrics.units_per_em;
        let ascender = self.metrics.ascender.unwrap_or(upm * 0.8);
        let descender = self.metrics.descender.unwrap_or(upm * -0.2);
        let width = self
            .session
            .glyph
            .advance
            .as_ref()
            .map(|a| a.width as f64)
            .unwrap_or(upm * 0.5);

        let work_size = Size::new(width, ascender + descender.abs());
        let work_origin = Point::new(0., descender);
        Rect::from_origin_size(work_origin, work_size)
    }

    /// Returns a `Rect` representing, in the coordinate space of the canvas,
    /// the total region occupied by outlines, components, anchors, and the metric
    /// bounds.
    pub fn content_region(&self) -> Rect {
        let result = self.layout_bounds().union(self.session.work_bounds());
        Rect::from_points((result.x0, -result.y0), (result.x1, -result.y1))
    }

    pub fn session_mut(&mut self) -> &mut EditSession {
        Arc::make_mut(&mut self.session)
    }
}

impl Default for FontObject {
    fn default() -> FontObject {
        let font_info = FontInfo {
            family_name: Some(String::from("Untitled")),
            ..Default::default()
        };

        let mut ufo = Ufo::new(MetaInfo::default());
        ufo.font_info = Some(font_info);

        FontObject {
            path: None,
            ufo,
            placeholder: Arc::new(placeholder_outline()),
        }
    }
}

impl<'a> From<&'a FontInfo> for FontMetrics {
    fn from(src: &'a FontInfo) -> FontMetrics {
        FontMetrics {
            units_per_em: src.units_per_em.unwrap_or(DEFAULT_UNITS_PER_EM),
            descender: src.descender,
            x_height: src.x_height,
            cap_height: src.cap_height,
            ascender: src.ascender,
            italic_angle: src.italic_angle,
        }
    }
}

impl Default for FontMetrics {
    fn default() -> Self {
        FontMetrics {
            units_per_em: DEFAULT_UNITS_PER_EM,
            descender: None,
            x_height: None,
            cap_height: None,
            ascender: None,
            italic_angle: None,
        }
    }
}

impl From<&AppState> for GlyphSet {
    fn from(src: &AppState) -> GlyphSet {
        GlyphSet {
            sessions: src.sessions.clone(),
            font: src.file.clone(),
        }
    }
}

pub mod lenses {
    pub mod app_state {
        use std::sync::Arc;

        use druid::{Data, Lens};
        use norad::GlyphName;

        use super::super::{AppState, EditorState as EditorState_, GlyphSet as GlyphSet_};

        /// AppState -> GlyphSet_
        pub struct GlyphSet;

        /// AppState -> EditorState
        pub struct EditorState(pub GlyphName);

        impl Lens<AppState, GlyphSet_> for GlyphSet {
            fn with<V, F: FnOnce(&GlyphSet_) -> V>(&self, data: &AppState, f: F) -> V {
                let glyphs = data.into();
                f(&glyphs)
            }
            fn with_mut<V, F: FnOnce(&mut GlyphSet_) -> V>(&self, data: &mut AppState, f: F) -> V {
                let mut glyphs = (&*data).into();
                let v = f(&mut glyphs);
                let GlyphSet_ { font, sessions } = glyphs;
                if !font.same(&data.file) {
                    data.file = font;
                }
                if !sessions.same(&data.sessions) {
                    data.sessions = sessions;
                }
                v
            }
        }

        impl Lens<AppState, EditorState_> for EditorState {
            fn with<V, F: FnOnce(&EditorState_) -> V>(&self, data: &AppState, f: F) -> V {
                let metrics = data
                    .file
                    .ufo
                    .font_info
                    .as_ref()
                    .map(Into::into)
                    .unwrap_or_default();
                let session = data.sessions.get(&self.0).cloned().unwrap();
                let font = data.into();
                let glyph = EditorState_ {
                    font,
                    metrics,
                    session,
                };
                f(&glyph)
            }

            fn with_mut<V, F: FnOnce(&mut EditorState_) -> V>(
                &self,
                data: &mut AppState,
                f: F,
            ) -> V {
                //FIXME: this is creating a new copy and then throwing it away
                //this is just so that the signatures work for now, we aren't actually doing any
                let metrics = data
                    .file
                    .ufo
                    .font_info
                    .as_ref()
                    .map(Into::into)
                    .unwrap_or_default();
                let session = data.sessions.get(&self.0).unwrap().to_owned();
                let font = (&*data).into();
                let mut glyph = EditorState_ {
                    font,
                    metrics,
                    session,
                };
                let v = f(&mut glyph);
                if !data
                    .sessions
                    .get(&self.0)
                    .map(|s| s.same(&glyph.session))
                    .unwrap_or(true)
                {
                    Arc::make_mut(&mut data.sessions).insert(self.0.clone(), glyph.session);
                }
                v
            }
        }
    }

    pub mod glyph_set {
        use druid::Lens;
        use norad::GlyphName;
        use std::sync::Arc;

        use super::super::{GlyphPlus, GlyphSet as GlyphSet_};

        /// GlyphSet_ -> GlyphPlus
        pub struct Glyph(pub GlyphName);

        impl Lens<GlyphSet_, GlyphPlus> for Glyph {
            fn with<V, F: FnOnce(&GlyphPlus) -> V>(&self, data: &GlyphSet_, f: F) -> V {
                let glyph = data
                    .font
                    .ufo
                    .get_glyph(&self.0)
                    .expect("missing glyph in lens");
                let outline = data
                    .get_bezier(&glyph.name)
                    .unwrap_or_else(|| data.font.placeholder.clone());
                let glyph = GlyphPlus {
                    glyph: Arc::clone(glyph),
                    outline,
                };
                f(&glyph)
            }

            fn with_mut<V, F: FnOnce(&mut GlyphPlus) -> V>(&self, data: &mut GlyphSet_, f: F) -> V {
                //FIXME: this is creating a new copy and then throwing it away
                //this is just so that the signatures work for now, we aren't actually doing any
                //mutating
                let glyph = data
                    .font
                    .ufo
                    .get_glyph(&self.0)
                    .expect("missing glyph in lens");
                let outline = data
                    .get_bezier(&glyph.name)
                    .unwrap_or_else(|| data.font.placeholder.clone());
                let mut glyph = GlyphPlus {
                    glyph: Arc::clone(glyph),
                    outline,
                };
                f(&mut glyph)
            }
        }
    }
}

/// Convert this glyph's path from the UFO representation into a `kurbo::BezPath`
/// (which we know how to draw.)
fn path_for_glyph(glyph: &Glyph) -> Option<BezPath> {
    /// An outline can have multiple contours, which correspond to subpaths
    fn add_contour(path: &mut BezPath, contour: &Contour) {
        let mut close: Option<&ContourPoint> = None;

        if contour.points.is_empty() {
            return;
        }

        let first = &contour.points[0];
        path.move_to((first.x as f64, first.y as f64));
        if first.typ != PointType::Move {
            close = Some(first);
        }

        let mut idx = 1;
        let mut controls = Vec::with_capacity(2);

        let mut add_curve = |to_point: Point, controls: &mut Vec<Point>| {
            match controls.as_slice() {
                &[] => path.line_to(to_point),
                &[a] => path.quad_to(a, to_point),
                &[a, b] => path.curve_to(a, b, to_point),
                _illegal => panic!("existence of second point implies first"),
            };
            controls.clear();
        };

        while idx < contour.points.len() {
            let next = &contour.points[idx];
            let point = Point::new(next.x as f64, next.y as f64);
            match next.typ {
                PointType::OffCurve => controls.push(point),
                PointType::Line => {
                    debug_assert!(controls.is_empty(), "line type cannot follow offcurve");
                    add_curve(point, &mut controls);
                }
                PointType::Curve => add_curve(point, &mut controls),
                PointType::QCurve => {
                    log::warn!("quadratic curves are currently ignored");
                    add_curve(point, &mut controls);
                }
                PointType::Move => debug_assert!(false, "illegal move point in path?"),
            }
            idx += 1;
        }

        if let Some(to_close) = close.take() {
            add_curve((to_close.x as f64, to_close.y as f64).into(), &mut controls);
        }
    }

    if let Some(outline) = glyph.outline.as_ref() {
        let mut path = BezPath::new();
        outline
            .contours
            .iter()
            .for_each(|c| add_contour(&mut path, c));
        Some(path)
    } else {
        None
    }
}

/// a poorly drawn question mark glyph, used as a placeholder
fn placeholder_outline() -> BezPath {
    let mut bez = BezPath::new();

    bez.move_to((51.0, 482.0));
    bez.line_to((112.0, 482.0));
    bez.curve_to((112.0, 482.0), (134.0, 631.0), (246.0, 631.0));
    bez.curve_to((308.0, 631.0), (356.0, 625.0), (356.0, 551.0));
    bez.curve_to((356.0, 487.0), (202.0, 432.0), (202.0, 241.0));
    bez.line_to((275.0, 241.0));
    bez.curve_to((276.0, 417.0), (430.0, 385.0), (430.0, 562.0));
    bez.curve_to((430.0, 699.0), (301.0, 700.0), (246.0, 700.0));
    bez.curve_to((201.0, 700.0), (51.0, 653.0), (51.0, 482.0));
    bez.close_path();

    bez.move_to((202.0, 172.0));
    bez.line_to((275.0, 172.0));
    bez.line_to((275.0, 105.0));
    bez.line_to((203.0, 105.0));
    bez.line_to((202.0, 172.0));
    bez.close_path();
    bez
}
