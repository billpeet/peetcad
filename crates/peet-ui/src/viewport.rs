//! The 3D viewport widget: navigation input, overlays and rendering via `peet-render`.

use std::collections::HashMap;

use eframe::egui_wgpu::RenderState;
use egui::{Color32, PointerButton, Rect, Sense, Ui, pos2};
use peet_math::{Aabb, DMat4, DQuat, DVec2, DVec3, Plane};
use peet_platform::Instant;
use peet_render::wgpu;
use peet_render::{
    Camera, CameraAnimation, FrameInput, MeshId, ObjectDraw, Overlay, Projection, RenderStats,
    StandardView, ViewStyle, ViewportRenderer,
};

use crate::bodies::GeomRef;
use crate::document::{Document, ItemId, ItemKind};
use crate::settings::{NavAction, Settings};
use crate::view_cube;

/// What happened in the viewport this frame that the app needs to react to.
#[derive(Default)]
pub struct ViewportEvents {
    /// Left click on empty space (clears the selection).
    pub clicked_background: bool,
    /// The viewport widget's response, for tools that handle their own mouse input
    /// (the sketcher).
    pub response: Option<egui::Response>,
    /// Left click on a body face or edge.
    pub clicked_geom: Option<GeomRef>,
    /// Left click on a reference plane (when no body is under the cursor).
    pub clicked_plane: Option<ItemId>,
}

pub struct ViewportParams<'a> {
    pub render_state: &'a RenderState,
    pub settings: &'a Settings,
    pub document: &'a Document,
    pub selected: Option<ItemId>,
    pub hovered: Option<ItemId>,
    pub dark: bool,
    /// The sketch open for editing: drawn by the sketch editor instead of the 3D overlay.
    pub editing_sketch: Option<ItemId>,
    /// Body face or edge under the cursor / selected, highlighted in the view.
    pub hovered_geom: Option<GeomRef>,
    pub selected_geom: &'a [GeomRef],
}

pub struct Viewport {
    pub camera: Camera,
    animation: Option<CameraAnimation>,
    renderer: ViewportRenderer,
    texture_id: Option<egui::TextureId>,
    overlay: Overlay,
    meshes: HashMap<ItemId, MeshId>,
    /// GPU meshes of the document's bodies, and the document revision they were built from.
    body_meshes: Vec<MeshId>,
    body_revision: Option<u64>,
    active_drag: Option<NavAction>,
    /// MSAA sample count in use.
    pub samples: u32,
    /// Statistics from the last rendered frame.
    pub stats: RenderStats,
    /// CPU time spent encoding and submitting the last frame, in milliseconds.
    pub render_cpu_ms: f64,
    /// Size of the viewport image in physical pixels.
    pub size_px: [u32; 2],
    /// Where the cursor ray hits the XY plane, if the cursor is over the viewport.
    pub cursor_on_ground: Option<DVec3>,
    /// Body face or edge under the cursor, from the last GPU pick.
    pub hovered_geom: Option<GeomRef>,
    /// Reference plane under the cursor (when no body is).
    pub hovered_plane: Option<ItemId>,
    /// Cursor position (physical pixels) of the last pick request still in flight.
    pick_pending: Option<[f32; 2]>,
    /// Half size of the reference plane quads, as last drawn.
    plane_half: f64,
    last_rect: Rect,
}

impl Viewport {
    pub fn new(render_state: &RenderState) -> Self {
        let features = render_state
            .adapter
            .get_texture_format_features(peet_render::COLOR_FORMAT);
        let samples = if features.flags.sample_count_supported(4) {
            4
        } else {
            1
        };
        log::info!("Viewport MSAA: {samples}x");
        Self {
            camera: Camera::default(),
            animation: None,
            renderer: ViewportRenderer::new(&render_state.device, samples),
            texture_id: None,
            overlay: Overlay::default(),
            meshes: HashMap::new(),
            body_meshes: Vec::new(),
            body_revision: None,
            active_drag: None,
            samples,
            stats: RenderStats::default(),
            render_cpu_ms: 0.0,
            size_px: [0, 0],
            cursor_on_ground: None,
            hovered_geom: None,
            hovered_plane: None,
            pick_pending: None,
            plane_half: 50.0,
            last_rect: Rect::NOTHING,
        }
    }

    fn aspect(&self) -> f64 {
        let r = self.last_rect;
        if r.height() > 0.0 {
            f64::from(r.width() / r.height())
        } else {
            1.0
        }
    }

    /// Moves the camera to `to`, animated if enabled in the settings.
    pub fn go_to(&mut self, to: Camera, animate: bool) {
        if animate {
            self.animation = Some(CameraAnimation::new(
                self.camera,
                to,
                CameraAnimation::DEFAULT_DURATION,
            ));
        } else {
            self.animation = None;
            self.camera = to;
        }
    }

    /// The camera state the view is heading to (the animation target, if one is running).
    fn destination(&self) -> Camera {
        self.animation.map_or(self.camera, |a| *a.target())
    }

    pub fn set_rotation(&mut self, rotation: DQuat, animate: bool) {
        let to = Camera {
            rotation,
            ..self.destination()
        };
        self.go_to(to, animate);
    }

    pub fn set_view(&mut self, view: StandardView, animate: bool) {
        self.set_rotation(view.rotation(), animate);
    }

    pub fn zoom_to_fit(&mut self, bounds: &Aabb, animate: bool) {
        let mut to = self.destination();
        let bounds = if bounds.is_empty() {
            // Nothing visible: frame the origin and the reference planes.
            Aabb::from_points([DVec3::splat(-60.0), DVec3::splat(60.0)])
        } else {
            *bounds
        };
        to.fit(&bounds, self.aspect());
        self.go_to(to, animate);
    }

    pub fn set_projection(&mut self, projection: Projection) {
        self.camera.projection = projection;
        if let Some(anim) = &mut self.animation {
            let mut target = *anim.target();
            target.projection = projection;
            *anim = CameraAnimation::new(self.camera, target, CameraAnimation::DEFAULT_DURATION);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, params: &ViewportParams<'_>) -> ViewportEvents {
        let rect = ui.available_rect_before_wrap();
        self.last_rect = rect;
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let mut events = ViewportEvents {
            response: Some(response.clone()),
            ..ViewportEvents::default()
        };

        self.handle_navigation(ui, &response, rect, params.settings);
        if response.clicked_by(PointerButton::Primary) {
            events.clicked_background = true;
        }

        // Advance the camera animation.
        if let Some(anim) = &mut self.animation {
            let dt = f64::from(ui.input(|i| i.stable_dt)).min(0.05);
            self.camera = anim.step(dt);
            if anim.is_finished() {
                self.animation = None;
            } else {
                ui.ctx().request_repaint();
            }
        }

        // Cursor position on the ground plane, for the status bar.
        self.cursor_on_ground = response.hover_pos().and_then(|p| {
            let ray = self.camera.ray(ndc(rect, p), self.aspect());
            let t = Plane::TOP.intersect_ray(&ray)?;
            (t > 0.0 || self.camera.projection == Projection::Orthographic).then(|| ray.at(t))
        });

        if rect.width() < 2.0 || rect.height() < 2.0 {
            return events;
        }

        self.sync_meshes(params);
        let wpp = self.camera.world_per_pixel(f64::from(rect.height()));
        self.build_overlay(params, wpp);
        self.body_overlay(params, wpp);

        let objects: Vec<ObjectDraw> = params
            .document
            .items
            .iter()
            .filter(|item| item.visible)
            .filter_map(|item| {
                let mesh = *self.meshes.get(&item.id)?;
                let highlight = if params.selected == Some(item.id) {
                    1.0
                } else if params.hovered == Some(item.id) {
                    0.5
                } else {
                    0.0
                };
                Some(ObjectDraw {
                    mesh,
                    transform: DMat4::IDENTITY,
                    show_edges: true,
                    highlight,
                    ..ObjectDraw::default()
                })
            })
            .chain(
                self.body_meshes
                    .iter()
                    .enumerate()
                    .map(|(i, mesh)| ObjectDraw {
                        mesh: *mesh,
                        transform: DMat4::IDENTITY,
                        show_edges: true,
                        highlight: 0.0,
                        pick_object: Some(i as u32),
                    }),
            )
            .collect();

        let mut scene_bounds = params.document.visible_body_bounds();
        for v in self.overlay.triangles.iter().chain(&self.overlay.lines) {
            scene_bounds.extend(DVec3::from(v.position.map(f64::from)));
        }

        let style = if params.dark {
            ViewStyle::dark()
        } else {
            ViewStyle::light()
        };
        let ppp = ui.ctx().pixels_per_point();
        let size_px = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        self.size_px = size_px;

        // Pick what's under the cursor (results arrive a frame or two later).
        let cursor_px = response
            .hover_pos()
            .filter(|_| self.active_drag.is_none())
            .map(|p| [(p.x - rect.left()) * ppp, (p.y - rect.top()) * ppp]);
        let pick = cursor_px.map(|c| peet_render::PickRequest {
            cursor_px: c,
            radius_px: 6.0 * ppp,
        });
        if cursor_px.is_none() {
            self.hovered_geom = None;
            self.hovered_plane = None;
        }
        self.hovered_plane = response
            .hover_pos()
            .and_then(|p| self.plane_at(params, rect, p));

        let rs = params.render_state;
        let start = Instant::now();
        let output = self.renderer.render(
            &rs.device,
            &rs.queue,
            &FrameInput {
                size_px,
                camera: &self.camera,
                style: &style,
                show_grid: params.settings.show_grid,
                objects: &objects,
                overlay: &self.overlay,
                scene_bounds,
                pick,
            },
        );
        self.stats = output.stats;
        if output.texture_changed {
            let mut egui_renderer = rs.renderer.write();
            match self.texture_id {
                None => {
                    self.texture_id = Some(egui_renderer.register_native_texture(
                        &rs.device,
                        output.view,
                        wgpu::FilterMode::Linear,
                    ));
                }
                Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                    &rs.device,
                    output.view,
                    wgpu::FilterMode::Linear,
                    id,
                ),
            }
        }
        self.render_cpu_ms = peet_platform::elapsed_ms(start);
        if let Some(result) = self.renderer.pick_result() {
            let edge = result
                .edge
                .filter(|e| e.distance_px <= 6.0 * ppp)
                .map(|e| GeomRef::Edge {
                    body: e.object as usize,
                    edge: peet_kernel::EdgeId(e.id),
                });
            let face = result.face.map(|f| GeomRef::Face {
                body: f.object as usize,
                face: peet_kernel::FaceId(f.id),
            });
            // A vertex wins when one of the picked edge's ends is right under the cursor.
            let vertex = match (edge, cursor_px) {
                (Some(GeomRef::Edge { body, edge }), Some(c)) => {
                    params.document.bodies.get(body).and_then(|b| {
                        let e = b.solid.edges.get(edge.index())?;
                        let view_proj = self.camera.view_projection(self.aspect(), &scene_bounds);
                        [e.start, e.end].into_iter().find_map(|v| {
                            let clip = view_proj * b.solid.vertex(v).point.extend(1.0);
                            if clip.w <= 0.0 {
                                return None;
                            }
                            let ndc = clip.truncate() / clip.w;
                            let px = (ndc.x as f32 + 1.0) * 0.5 * size_px[0] as f32;
                            let py = (1.0 - ndc.y as f32) * 0.5 * size_px[1] as f32;
                            let near = (px - c[0]).hypot(py - c[1]) <= 7.0 * ppp;
                            near.then_some(GeomRef::Vertex { body, vertex: v })
                        })
                    })
                }
                _ => None,
            };
            if cursor_px.is_some() {
                self.hovered_geom = vertex.or(edge).or(face);
            }
            if self.pick_pending == Some(result.cursor_px) {
                self.pick_pending = None;
            }
        }
        if let Some(c) = cursor_px
            && self.pick_pending != Some(c)
        {
            self.pick_pending = Some(c);
            // Keep frames coming until the answer for this position arrives.
            ui.ctx().request_repaint();
        } else if self.pick_pending.is_some() {
            ui.ctx().request_repaint();
        }
        if self.hovered_geom.is_some() {
            self.hovered_plane = None;
        }
        if events.clicked_background {
            events.clicked_geom = self.hovered_geom;
            events.clicked_plane = self.hovered_plane;
        }

        if let Some(id) = self.texture_id {
            ui.painter().image(
                id,
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        view_cube::axis_triad(ui, rect, &self.camera);
        if params.settings.show_view_cube
            && let Some(rotation) = view_cube::view_cube(ui, rect, &self.camera, params.dark)
        {
            self.set_rotation(rotation, params.settings.animate_views);
        }

        events
    }

    fn handle_navigation(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        rect: Rect,
        settings: &Settings,
    ) {
        let (delta, modifiers, scroll, pinch, press_origin) = ui.input(|i| {
            (
                i.pointer.delta(),
                i.modifiers,
                i.smooth_scroll_delta.y,
                i.zoom_delta(),
                i.pointer.press_origin(),
            )
        });
        let aspect = self.aspect();

        // Lock the navigation mode when a drag starts, so releasing a modifier mid-drag
        // doesn't switch from rotating to panning.
        for button in [
            PointerButton::Primary,
            PointerButton::Secondary,
            PointerButton::Middle,
        ] {
            if response.drag_started_by(button) {
                self.active_drag = settings.mouse_preset.action_for(button, modifiers);
            }
        }
        if response.drag_stopped() {
            self.active_drag = None;
        }

        let mut navigated = false;
        if response.dragged()
            && let Some(action) = self.active_drag
        {
            let d = DVec2::new(f64::from(delta.x), f64::from(delta.y));
            if d != DVec2::ZERO {
                navigated = true;
                match action {
                    NavAction::Orbit => self.camera.orbit(d, settings.orbit.style()),
                    NavAction::Pan => self.camera.pan(d, f64::from(rect.height())),
                    NavAction::Zoom => {
                        let anchor = press_origin.map_or(DVec2::ZERO, |p| ndc(rect, p));
                        self.camera.zoom_at((d.y * 0.006).exp(), anchor, aspect);
                    }
                }
            }
        }

        if let Some(hover) = response.hover_pos() {
            let anchor = ndc(rect, hover);
            let direction = if settings.invert_zoom { -1.0 } else { 1.0 };
            if scroll != 0.0 {
                navigated = true;
                let factor = (-f64::from(scroll) * 0.0022 * direction).exp();
                self.camera.zoom_at(factor, anchor, aspect);
            }
            if pinch != 1.0 {
                navigated = true;
                self.camera.zoom_at(1.0 / f64::from(pinch), anchor, aspect);
            }
        }

        if navigated {
            // Direct manipulation always wins over a running animation.
            if let Some(anim) = self.animation.take() {
                self.camera.projection = anim.target().projection;
            }
        }
    }

    fn sync_meshes(&mut self, params: &ViewportParams<'_>) {
        if self.body_revision != Some(params.document.revision) {
            for id in self.body_meshes.drain(..) {
                self.renderer.remove_mesh(id);
            }
            for body in &params.document.bodies {
                let id = self
                    .renderer
                    .upload_mesh(&params.render_state.device, &body.mesh);
                self.body_meshes.push(id);
            }
            self.body_revision = Some(params.document.revision);
        }
        for item in &params.document.items {
            if let ItemKind::Body { mesh } = &item.kind
                && !self.meshes.contains_key(&item.id)
            {
                let id = self.renderer.upload_mesh(&params.render_state.device, mesh);
                self.meshes.insert(item.id, id);
            }
        }
    }

    fn build_overlay(&mut self, params: &ViewportParams<'_>, wpp: f64) {
        self.overlay.clear();
        let doc = params.document;

        // Reference planes are sized to comfortably frame the visible bodies and sketches.
        let bounds = doc.visible_bounds();
        let half = if bounds.is_empty() {
            50.0
        } else {
            let extent = bounds.min.abs().max(bounds.max.abs());
            (extent.max_element() * 1.15).max(20.0)
        };
        self.plane_half = half;

        let (fill, outline, fill_hi, outline_hi): ([u8; 4], [u8; 4], [u8; 4], [u8; 4]) =
            if params.dark {
                (
                    [96, 146, 226, 14],
                    [118, 166, 238, 130],
                    [96, 160, 255, 48],
                    [140, 196, 255, 255],
                )
            } else {
                (
                    [56, 108, 200, 12],
                    [56, 108, 200, 120],
                    [56, 120, 230, 40],
                    [30, 100, 230, 255],
                )
            };

        for item in doc.items.iter().filter(|i| i.visible) {
            let highlighted = params.selected == Some(item.id)
                || params.hovered == Some(item.id)
                || self.hovered_plane == Some(item.id);
            match &item.kind {
                ItemKind::ReferencePlane(which) => {
                    let plane = which.plane();
                    let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                        .map(|(u, v)| plane.from_plane_coords(DVec2::new(u, v) * half));
                    if highlighted {
                        self.overlay.quad(corners, fill_hi, outline_hi);
                    } else {
                        self.overlay.quad(corners, fill, outline);
                    }
                }
                ItemKind::Origin => {
                    // Constant on-screen size, like other CAD tools' origin marker.
                    let len = wpp * 36.0;
                    let alpha = if highlighted { 255 } else { 210 };
                    self.overlay
                        .line(DVec3::ZERO, DVec3::X * len, [226, 86, 86, alpha]);
                    self.overlay
                        .line(DVec3::ZERO, DVec3::Y * len, [112, 196, 92, alpha]);
                    self.overlay
                        .line(DVec3::ZERO, DVec3::Z * len, [84, 146, 240, alpha]);
                }
                ItemKind::Body { .. } | ItemKind::Extrude(_) => {}
                ItemKind::Sketch(sketch) => {
                    if params.editing_sketch == Some(item.id) {
                        continue;
                    }
                    let color = if highlighted {
                        outline_hi
                    } else if params.dark {
                        [150, 190, 255, 230]
                    } else {
                        [40, 90, 190, 230]
                    };
                    let plane = sketch.plane;
                    for (id, e) in sketch.sketch.entities() {
                        if e.construction {
                            continue;
                        }
                        let Some(curve) = sketch.sketch.curve(id) else {
                            continue;
                        };
                        let pts = curve.tessellate(wpp * 0.5);
                        for w in pts.windows(2) {
                            self.overlay.line(
                                plane.from_plane_coords(w[0]),
                                plane.from_plane_coords(w[1]),
                                color,
                            );
                        }
                    }
                }
            }
        }
    }
}

impl Viewport {
    /// The visible reference plane under a screen position (nearest along the ray).
    fn plane_at(&self, params: &ViewportParams<'_>, rect: Rect, p: egui::Pos2) -> Option<ItemId> {
        let ray = self.camera.ray(ndc(rect, p), self.aspect());
        params
            .document
            .items
            .iter()
            .filter(|i| i.visible)
            .filter_map(|i| match i.kind {
                ItemKind::ReferencePlane(which) => {
                    let plane = which.plane();
                    let t = plane.intersect_ray(&ray)?;
                    let uv = plane.to_plane_coords(ray.at(t));
                    (uv.abs().max_element() <= self.plane_half).then_some((t, i.id))
                }
                _ => None,
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
    }

    /// Silhouettes of curved faces and highlights of hovered/selected faces and edges.
    fn body_overlay(&mut self, params: &ViewportParams<'_>, wpp: f64) {
        let view = match self.camera.projection {
            Projection::Orthographic => peet_kernel::tessellate::View::Orthographic {
                dir: self.camera.forward(),
            },
            Projection::Perspective => peet_kernel::tessellate::View::Perspective {
                eye: self.camera.eye(),
            },
        };
        let edge_color = if params.dark {
            [18, 20, 24, 255]
        } else {
            [28, 30, 36, 255]
        };
        for body in &params.document.bodies {
            for [a, b] in body.silhouettes.lines(view) {
                self.overlay.line(a, b, edge_color);
            }
        }
        let hover = params
            .hovered_geom
            .filter(|h| !params.selected_geom.contains(h));
        let marks = params
            .selected_geom
            .iter()
            .map(|g| (*g, true))
            .chain(hover.map(|g| (g, false)));
        for (geom, selected) in marks {
            let Some(body) = params.document.bodies.get(geom.body()) else {
                continue;
            };
            let (fill, line): ([u8; 4], [u8; 4]) = if selected {
                ([255, 150, 30, 110], [255, 150, 30, 255])
            } else {
                ([70, 140, 255, 80], [70, 150, 255, 255])
            };
            match geom {
                GeomRef::Face { face, .. } => {
                    if let Some(fm) = body.tess.faces.iter().find(|f| f.face == face) {
                        // Lift slightly along the normal so the highlight wins the depth test.
                        let lift = 1e-3 * body.solid.bounds().size().length().max(1.0);
                        for t in &fm.triangles {
                            for &i in t {
                                let i = i as usize;
                                let p = fm.positions[i] + fm.normals[i] * lift;
                                self.overlay
                                    .triangles
                                    .push(peet_render::ColorVertex::new(p, fill));
                            }
                        }
                    }
                }
                GeomRef::Edge { edge, .. } => {
                    if let Some(ep) = body.tess.edges.iter().find(|e| e.edge == edge) {
                        for w in ep.points.windows(2) {
                            self.overlay.line(w[0], w[1], line);
                        }
                    }
                }
                GeomRef::Vertex { vertex, .. } => {
                    // A small screen-facing diamond of constant on-screen size.
                    let Some(v) = body.solid.vertices.get(vertex.index()) else {
                        continue;
                    };
                    let r = wpp * 5.0;
                    let p = v.point - self.camera.forward() * (wpp * 2.0);
                    let (x, y) = (self.camera.right() * r, self.camera.up() * r);
                    for corner in [p + x, p + y, p - x, p + x, p - x, p - y] {
                        self.overlay
                            .triangles
                            .push(peet_render::ColorVertex::new(corner, line));
                    }
                }
            }
        }
    }
}

/// Converts a screen position to normalized device coordinates within `rect` (+Y up).
fn ndc(rect: Rect, p: egui::Pos2) -> DVec2 {
    DVec2::new(
        f64::from((p.x - rect.left()) / rect.width()) * 2.0 - 1.0,
        1.0 - f64::from((p.y - rect.top()) / rect.height()) * 2.0,
    )
}
