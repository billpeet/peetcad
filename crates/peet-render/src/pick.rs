//! GPU picking: which face is under the cursor, and which edge is nearest to it.
//!
//! # How it works
//!
//! When a [`PickRequest`] comes with a frame, the renderer draws the pickable objects a
//! second time, without MSAA, into a small [`PICK_SIZE`]² window of id textures centred on
//! the cursor pixel. The window is selected with an extra screen-space scale and offset
//! applied after the view-projection ([`pick_matrix`]), so pick pixels line up exactly
//! with screen pixels and no viewport offsets or scissors are needed.
//!
//! Two passes share one depth buffer:
//! 1. **faces** write their object id, per-vertex face id and depth;
//! 2. **edges** are depth tested against the faces with the same line bias as the visible
//!    edges, so an edge that is visible on screen is pickable and a hidden one is not.
//!
//! The two passes write separate textures, so a face is still reported when the cursor
//! sits on one of its edges.
//!
//! # Encoding
//!
//! Every value is a `u32` packed little-endian into an `Rgba8Unorm` texel (R = low byte).
//! `Rgba8Unorm` is renderable and readable everywhere, including WebGL2 without
//! extensions (integer and float render targets are not guaranteed there), and
//! `byte / 255` round-trips exactly through unorm conversion. Each pass has three
//! attachments (WebGL2 guarantees four):
//!
//! | attachment | value |
//! |---|---|
//! | 0 | object id + 1 (0 = nothing drawn) |
//! | 1 | element id (face id or edge id, from the mesh) |
//! | 2 | the bits of the `f32` window depth (reversed-Z, 0 = far) |
//!
//! # Readback
//!
//! The six textures are copied into a `MAP_READ` buffer that is mapped asynchronously.
//! Nothing ever blocks: completed results are collected by
//! [`ViewportRenderer::pick_result`](crate::ViewportRenderer::pick_result), typically one
//! or two frames after the request. A small ring of buffers lets a request go out every
//! frame; if all of them are still in flight, the request for that frame is skipped.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use peet_math::{DMat4, DVec2, DVec3, DVec4};

/// Side length of the square pick window, in pixels. With 4 bytes per texel a row is
/// exactly 256 bytes, which is [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
pub(crate) const PICK_SIZE: u32 = 64;
/// Window coordinate of the cursor pixel (both axes).
const CENTER: i32 = (PICK_SIZE / 2) as i32;
/// The largest edge search radius the pick window can hold.
pub const MAX_PICK_RADIUS_PX: f32 = (PICK_SIZE / 2 - 1) as f32;
/// Format of every pick attachment; see the module docs.
pub(crate) const PICK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Face object, face element, face depth, edge object, edge element, edge depth.
pub(crate) const PLANE_COUNT: usize = 6;
/// How many readbacks may be in flight at once.
const READBACK_SLOTS: usize = 3;

/// Ask the renderer to pick at a cursor position this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickRequest {
    /// Cursor position in physical pixels from the top-left of the viewport.
    pub cursor_px: [f32; 2],
    /// How far from the cursor to look for edges, in pixels (clamped to
    /// [`MAX_PICK_RADIUS_PX`]). Around 6 works well.
    pub radius_px: f32,
}

/// One picked element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickHit {
    /// The [`ObjectDraw::pick_object`](crate::ObjectDraw::pick_object) of the hit object.
    pub object: u32,
    /// The face or edge id from the mesh's `pick_ids` / `edge_pick_ids`.
    pub id: u32,
    /// Distance from the cursor pixel to the hit pixel (0 for faces).
    pub distance_px: f32,
    /// Window depth of the hit (reversed-Z: 1 = near plane, 0 = far plane).
    pub depth: f32,
    /// The hit point in world space, unprojected from the hit pixel's centre and depth
    /// with the view-projection of the frame the pick was rendered in. Edge points are
    /// pulled towards the eye by the (tiny) line depth bias.
    pub world: DVec3,
}

/// The outcome of a [`PickRequest`].
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PickResult {
    /// The cursor position of the request this answers.
    pub cursor_px: [f32; 2],
    /// The face under the cursor pixel, if any.
    pub face: Option<PickHit>,
    /// The visible edge nearest to the cursor within the search radius, if any.
    pub edge: Option<PickHit>,
}

/// Value written to the object attachment for a pickable object (0 means "nothing").
pub(crate) fn encode_object(object: u32) -> u32 {
    debug_assert!(object != u32::MAX, "u32::MAX is not a valid pick object id");
    object.wrapping_add(1)
}

pub(crate) fn decode_object(value: u32) -> Option<u32> {
    value.checked_sub(1)
}

/// Bytes per row of a texture-to-buffer copy, padded to the copy alignment.
pub(crate) fn padded_bytes_per_row(width: u32, bytes_per_texel: u32) -> u32 {
    (width * bytes_per_texel).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

/// Bytes of one pick texture in the readback buffer.
pub(crate) fn plane_bytes() -> u64 {
    u64::from(padded_bytes_per_row(PICK_SIZE, 4)) * u64::from(PICK_SIZE)
}

/// Top-left screen pixel of the pick window for a cursor position.
pub(crate) fn window_origin(cursor_px: [f32; 2]) -> [i32; 2] {
    cursor_px.map(|c| c.floor() as i32 - CENTER)
}

/// Maps screen clip space to the clip space of a `size`² window whose top-left pixel is
/// `origin` (in pixels from the top-left of a `viewport`-sized screen). Applied after the
/// view-projection, it renders exactly that window, pixel for pixel. Depth is unchanged.
pub(crate) fn pick_matrix(viewport: [u32; 2], origin: [i32; 2], size: u32) -> DMat4 {
    let (w, h) = (f64::from(viewport[0]), f64::from(viewport[1]));
    let s = f64::from(size);
    let (ox, oy) = (f64::from(origin[0]), f64::from(origin[1]));
    // x_pick = x_ndc * w/s + (w - 2 ox)/s - 1; y_pick = y_ndc * h/s + 1 + (2 oy - h)/s
    // (y is up in NDC but down in pixels). In clip space the offsets scale with w_clip.
    DMat4::from_cols(
        DVec4::new(w / s, 0.0, 0.0, 0.0),
        DVec4::new(0.0, h / s, 0.0, 0.0),
        DVec4::Z,
        DVec4::new((w - 2.0 * ox) / s - 1.0, 1.0 + (2.0 * oy - h) / s, 0.0, 1.0),
    )
}

/// Normalized device coordinates of a point given in pixels from the top-left.
pub(crate) fn pixel_to_ndc(px: DVec2, viewport: [u32; 2]) -> DVec2 {
    DVec2::new(
        px.x / f64::from(viewport[0]) * 2.0 - 1.0,
        1.0 - px.y / f64::from(viewport[1]) * 2.0,
    )
}

/// World position of an NDC point at a given (reversed-Z) depth.
pub(crate) fn unproject(inv_view_proj: &DMat4, ndc: DVec2, depth: f64) -> DVec3 {
    inv_view_proj.project_point3(DVec3::new(ndc.x, ndc.y, depth))
}

/// What is needed to interpret a pick readback.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PickFrame {
    pub request: PickRequest,
    pub viewport: [u32; 2],
    pub origin: [i32; 2],
    pub inv_view_proj: DMat4,
}

impl PickFrame {
    pub fn new(request: PickRequest, viewport: [u32; 2], view_proj: &DMat4) -> Self {
        Self {
            request,
            viewport,
            origin: window_origin(request.cursor_px),
            inv_view_proj: view_proj.inverse(),
        }
    }
}

/// The readback buffer contents: [`PLANE_COUNT`] padded textures, back to back.
struct Planes<'a> {
    data: &'a [u8],
    row_texels: usize,
    plane_len: usize,
}

impl<'a> Planes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            row_texels: padded_bytes_per_row(PICK_SIZE, 4) as usize / 4,
            plane_len: plane_bytes() as usize,
        }
    }

    fn texel(&self, plane: usize, x: i32, y: i32) -> u32 {
        let i = plane * self.plane_len + (y as usize * self.row_texels + x as usize) * 4;
        self.data
            .get(i..i + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Decodes a readback buffer (see [`Planes`]) into a pick result.
pub(crate) fn decode(frame: &PickFrame, data: &[u8]) -> PickResult {
    let planes = Planes::new(data);
    let on_screen = |x: i32, y: i32| {
        let (sx, sy) = (frame.origin[0] + x, frame.origin[1] + y);
        sx >= 0 && sy >= 0 && (sx as u32) < frame.viewport[0] && (sy as u32) < frame.viewport[1]
    };
    let hit = |base: usize, x: i32, y: i32, distance_px: f32| -> Option<PickHit> {
        let object = decode_object(planes.texel(base, x, y))?;
        let depth = f32::from_bits(planes.texel(base + 2, x, y));
        let center = DVec2::new(
            f64::from(frame.origin[0] + x) + 0.5,
            f64::from(frame.origin[1] + y) + 0.5,
        );
        let ndc = pixel_to_ndc(center, frame.viewport);
        Some(PickHit {
            object,
            id: planes.texel(base + 1, x, y),
            distance_px,
            depth,
            world: unproject(&frame.inv_view_proj, ndc, f64::from(depth)),
        })
    };

    let face = on_screen(CENTER, CENTER)
        .then(|| hit(0, CENTER, CENTER, 0.0))
        .flatten();
    let edge = nearest_set_pixel(
        |x, y| on_screen(x, y) && planes.texel(3, x, y) != 0,
        |x, y| f32::from_bits(planes.texel(5, x, y)),
        frame.request.radius_px,
    )
    .and_then(|(x, y, d)| hit(3, x, y, d));

    PickResult {
        cursor_px: frame.request.cursor_px,
        face,
        edge,
    }
}

/// Finds the pixel nearest to the window centre (within `radius` pixels) for which `is_set`
/// holds. Ties go to the pixel nearest the eye (largest reversed-Z `depth`).
/// Returns window coordinates and the distance in pixels.
fn nearest_set_pixel(
    is_set: impl Fn(i32, i32) -> bool,
    depth: impl Fn(i32, i32) -> f32,
    radius: f32,
) -> Option<(i32, i32, f32)> {
    let radius = radius.clamp(0.0, MAX_PICK_RADIUS_PX);
    let r = radius.floor() as i32;
    let max_sq = radius * radius;
    let mut best: Option<(i32, i32, i32, f32)> = None;
    for dy in -r..=r {
        for dx in -r..=r {
            let d_sq = dx * dx + dy * dy;
            if d_sq as f32 > max_sq {
                continue;
            }
            let (x, y) = (CENTER + dx, CENTER + dy);
            if !is_set(x, y) {
                continue;
            }
            let z = depth(x, y);
            let better = best.is_none_or(|(_, _, bd, bz)| d_sq < bd || (d_sq == bd && z > bz));
            if better {
                best = Some((x, y, d_sq, z));
            }
        }
    }
    best.map(|(x, y, d_sq, _)| (x, y, (d_sq as f32).sqrt()))
}

const IDLE: u8 = 0;
const MAPPING: u8 = 1;
const READY: u8 = 2;
const FAILED: u8 = 3;

struct ReadbackSlot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// The request whose results are (or will be) in the buffer, with its sequence number.
    frame: Option<(u64, PickFrame)>,
}

/// The pick render targets and the asynchronous readback ring.
pub(crate) struct Picker {
    device: wgpu::Device,
    /// Face object, element, depth, then edge object, element, depth.
    textures: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    depth: wgpu::TextureView,
    slots: Vec<ReadbackSlot>,
    next_seq: u64,
    latest: Option<(u64, PickResult)>,
}

impl Picker {
    pub fn new(device: &wgpu::Device) -> Self {
        let extent = wgpu::Extent3d {
            width: PICK_SIZE,
            height: PICK_SIZE,
            depth_or_array_layers: 1,
        };
        let texture = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let textures: Vec<wgpu::Texture> = (0..PLANE_COUNT)
            .map(|_| {
                texture(
                    "pick_id",
                    PICK_FORMAT,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                )
            })
            .collect();
        let views = textures
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let depth = texture(
            "pick_depth",
            crate::renderer::DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let slots = (0..READBACK_SLOTS)
            .map(|_| ReadbackSlot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pick_readback"),
                    size: plane_bytes() * PLANE_COUNT as u64,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(IDLE)),
                frame: None,
            })
            .collect();
        Self {
            device: device.clone(),
            textures,
            views,
            depth,
            slots,
            next_seq: 0,
            latest: None,
        }
    }

    /// The colour attachment views of the face pass (`edges = false`) or the edge pass.
    pub fn pass_views(&self, edges: bool) -> &[wgpu::TextureView] {
        if edges {
            &self.views[3..6]
        } else {
            &self.views[0..3]
        }
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth
    }

    /// A readback slot that is free to receive a new pick, if any.
    pub fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.frame.is_none())
    }

    /// Records the copies of all pick textures into a slot's buffer.
    pub fn encode_copy(&self, encoder: &mut wgpu::CommandEncoder, slot: usize) {
        let bytes_per_row = padded_bytes_per_row(PICK_SIZE, 4);
        for (i, texture) in self.textures.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.slots[slot].buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: plane_bytes() * i as u64,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(PICK_SIZE),
                    },
                },
                wgpu::Extent3d {
                    width: PICK_SIZE,
                    height: PICK_SIZE,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Starts mapping a slot's buffer. Call after the copies have been submitted.
    pub fn start_readback(&mut self, slot: usize, frame: PickFrame) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let s = &mut self.slots[slot];
        s.frame = Some((seq, frame));
        s.state.store(MAPPING, Ordering::Release);
        let state = s.state.clone();
        s.buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let value = if result.is_ok() { READY } else { FAILED };
            state.store(value, Ordering::Release);
        });
    }

    /// True while a readback is in flight.
    pub fn pending(&self) -> bool {
        self.slots.iter().any(|s| s.frame.is_some())
    }

    /// Lets wgpu fire map callbacks (without blocking) and decodes finished readbacks.
    pub fn collect(&mut self) {
        if !self.pending() {
            return;
        }
        // Never blocks. On WebGPU this is a no-op and the browser fires the callbacks.
        if let Err(e) = self.device.poll(wgpu::PollType::Poll) {
            log::warn!("pick readback poll failed: {e}");
        }
        for slot in &mut self.slots {
            let Some((seq, frame)) = slot.frame else {
                continue;
            };
            match slot.state.load(Ordering::Acquire) {
                READY => {
                    let result = match slot.buffer.get_mapped_range(..) {
                        Ok(view) => Some(decode(&frame, &view)),
                        Err(e) => {
                            log::warn!("pick readback failed: {e}");
                            None
                        }
                    };
                    slot.buffer.unmap();
                    if let Some(result) = result
                        && self.latest.is_none_or(|(s, _)| s < seq)
                    {
                        self.latest = Some((seq, result));
                    }
                }
                FAILED => log::warn!("pick readback buffer could not be mapped"),
                _ => continue,
            }
            slot.state.store(IDLE, Ordering::Release);
            slot.frame = None;
        }
    }

    /// Takes the newest completed result.
    pub fn take_result(&mut self) -> Option<PickResult> {
        self.collect();
        self.latest.take().map(|(_, r)| r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, Projection};
    use peet_math::Aabb;

    /// Mirrors `pack_u32` in pick.wgsl followed by unorm8 storage and readback.
    fn through_rgba8_unorm(value: u32) -> u32 {
        let bytes = [0, 8, 16, 24].map(|shift| {
            let channel = ((value >> shift) & 255) as f32 / 255.0;
            // Unorm conversion: clamp, scale, round to nearest.
            (channel.clamp(0.0, 1.0) * 255.0).round() as u8
        });
        u32::from_le_bytes(bytes)
    }

    #[test]
    fn ids_round_trip_through_rgba8() {
        for v in [
            0,
            1,
            2,
            255,
            256,
            0x1234_5678,
            0x00ff_00ff,
            u32::MAX - 1,
            u32::MAX,
        ] {
            assert_eq!(through_rgba8_unorm(v), v);
        }
        for byte in 0..=255u32 {
            assert_eq!(through_rgba8_unorm(byte * 0x0101_0101), byte * 0x0101_0101);
        }
    }

    #[test]
    fn depth_round_trips_through_rgba8() {
        for d in [0.0f32, 1.0, 0.5, 1e-7, 0.999_999_9, 0.123_456_79] {
            assert_eq!(f32::from_bits(through_rgba8_unorm(d.to_bits())), d);
        }
    }

    #[test]
    fn object_encoding_reserves_zero() {
        assert_eq!(decode_object(0), None);
        for o in [0, 1, 7, 1_000_000, u32::MAX - 1] {
            assert_ne!(encode_object(o), 0);
            assert_eq!(decode_object(encode_object(o)), Some(o));
        }
    }

    #[test]
    fn row_padding() {
        assert_eq!(padded_bytes_per_row(64, 4), 256);
        assert_eq!(padded_bytes_per_row(1, 4), 256);
        assert_eq!(padded_bytes_per_row(65, 4), 512);
        assert_eq!(padded_bytes_per_row(128, 4), 512);
        assert_eq!(padded_bytes_per_row(100, 1), 256);
        assert_eq!(plane_bytes(), 64 * 256);
        assert_eq!(plane_bytes() % wgpu::MAP_ALIGNMENT, 0);
    }

    #[test]
    fn pick_matrix_maps_screen_pixels_to_window_pixels() {
        let viewport = [1280, 720];
        let origin = [-20, 690];
        let m = pick_matrix(viewport, origin, PICK_SIZE);
        for (i, j) in [(0, 0), (5, 63), (63, 0), (32, 32)] {
            let screen = DVec2::new(
                f64::from(origin[0] + i) + 0.5,
                f64::from(origin[1] + j) + 0.5,
            );
            let ndc = pixel_to_ndc(screen, viewport);
            // A clip-space point with w != 1 must land on the same pixel.
            let w = 3.5;
            let clip = m * DVec4::new(ndc.x * w, ndc.y * w, 0.25 * w, w);
            let picked = clip.truncate() / clip.w;
            let expected = pixel_to_ndc(
                DVec2::new(f64::from(i) + 0.5, f64::from(j) + 0.5),
                [PICK_SIZE, PICK_SIZE],
            );
            assert!((picked.truncate() - expected).length() < 1e-9, "{i},{j}");
            assert!((picked.z - 0.25).abs() < 1e-12);
        }
    }

    #[test]
    fn window_is_centred_on_cursor_pixel() {
        assert_eq!(window_origin([100.7, 50.2]), [100 - 32, 50 - 32]);
        assert_eq!(window_origin([0.0, 0.0]), [-32, -32]);
    }

    #[test]
    fn unprojection_inverts_projection() {
        let bounds = Aabb::from_points([DVec3::splat(-100.0), DVec3::splat(100.0)]);
        for projection in [Projection::Perspective, Projection::Orthographic] {
            let camera = Camera {
                projection,
                ..Camera::default()
            };
            let vp = camera.view_projection(1.6, &bounds);
            let inv = vp.inverse();
            for p in [
                DVec3::ZERO,
                DVec3::new(30.0, -20.0, 55.0),
                DVec3::splat(-90.0),
            ] {
                let ndc = vp.project_point3(p);
                assert!(
                    (0.0..=1.0).contains(&ndc.z),
                    "{projection:?} depth {}",
                    ndc.z
                );
                let back = unproject(&inv, ndc.truncate(), ndc.z);
                assert!((back - p).length() < 1e-6, "{projection:?}: {back} vs {p}");
            }
        }
    }

    /// Builds a readback buffer from per-plane closures over window coordinates.
    fn buffer(fill: impl Fn(usize, i32, i32) -> u32) -> Vec<u8> {
        let row = padded_bytes_per_row(PICK_SIZE, 4) as usize;
        let mut data = vec![0u8; plane_bytes() as usize * PLANE_COUNT];
        for plane in 0..PLANE_COUNT {
            for y in 0..PICK_SIZE as i32 {
                for x in 0..PICK_SIZE as i32 {
                    let i = plane * plane_bytes() as usize + y as usize * row + x as usize * 4;
                    data[i..i + 4].copy_from_slice(&fill(plane, x, y).to_le_bytes());
                }
            }
        }
        data
    }

    fn frame(cursor: [f32; 2], radius: f32) -> PickFrame {
        let camera = Camera::default();
        let bounds = Aabb::from_points([DVec3::splat(-100.0), DVec3::splat(100.0)]);
        let viewport = [800, 600];
        let vp = camera.view_projection(800.0 / 600.0, &bounds);
        PickFrame::new(
            PickRequest {
                cursor_px: cursor,
                radius_px: radius,
            },
            viewport,
            &vp,
        )
    }

    #[test]
    fn decodes_face_under_cursor_and_nearest_edge() {
        let f = frame([400.5, 300.5], 6.0);
        let depth = 0.6f32;
        let data = buffer(|plane, x, y| match plane {
            // Face 7 of object 3 everywhere.
            0 => encode_object(3),
            1 => 7,
            2 => depth.to_bits(),
            // Edges: id 11 of object 4 three pixels right; id 12 at five pixels down.
            3 if (x, y) == (CENTER + 3, CENTER) => encode_object(4),
            3 if (x, y) == (CENTER, CENTER + 5) => encode_object(4),
            4 if y == CENTER + 5 => 12,
            4 => 11,
            5 => 0.7f32.to_bits(),
            _ => 0,
        });
        let r = decode(&f, &data);
        assert_eq!(r.cursor_px, [400.5, 300.5]);
        let face = r.face.expect("face");
        assert_eq!((face.object, face.id, face.distance_px), (3, 7, 0.0));
        assert_eq!(face.depth, depth);
        // The world point projects back to the cursor pixel centre at the same depth.
        let vp = f.inv_view_proj.inverse();
        let p = vp.project_point3(face.world);
        let expected = pixel_to_ndc(DVec2::new(400.5, 300.5), f.viewport);
        assert!((p.truncate() - expected).length() < 1e-6);
        assert!((p.z - f64::from(depth)).abs() < 1e-6);

        let edge = r.edge.expect("edge");
        assert_eq!((edge.object, edge.id, edge.distance_px), (4, 11, 3.0));
        assert_eq!(edge.depth, 0.7);
    }

    #[test]
    fn nothing_hit() {
        let r = decode(&frame([10.0, 10.0], 6.0), &buffer(|_, _, _| 0));
        assert_eq!(r.face, None);
        assert_eq!(r.edge, None);
    }

    #[test]
    fn edges_outside_radius_or_screen_are_ignored() {
        // An edge 7 px away is outside a 6 px radius (diagonal 5,5 = 7.07 too).
        let data = buffer(|plane, x, y| match plane {
            3 if (x, y) == (CENTER + 7, CENTER) || (x, y) == (CENTER + 5, CENTER + 5) => 1,
            _ => 0,
        });
        assert_eq!(decode(&frame([400.0, 300.0], 6.0), &data).edge, None);
        assert!(decode(&frame([400.0, 300.0], 7.5), &data).edge.is_some());

        // An edge left of the screen (cursor at x = 2, edge 3 px left) is ignored.
        let data = buffer(|plane, x, y| match plane {
            3 if (x, y) == (CENTER - 3, CENTER) => 1,
            _ => 0,
        });
        assert_eq!(decode(&frame([2.0, 300.0], 6.0), &data).edge, None);
        assert!(decode(&frame([20.0, 300.0], 6.0), &data).edge.is_some());
    }

    #[test]
    fn nearest_prefers_distance_then_nearer_depth() {
        let set = [
            (CENTER + 2, CENTER, 0.2),
            (CENTER, CENTER - 2, 0.9),
            (CENTER + 1, CENTER + 2, 1.0),
        ];
        let found = nearest_set_pixel(
            |x, y| set.iter().any(|&(sx, sy, _)| (sx, sy) == (x, y)),
            |x, y| {
                set.iter()
                    .find(|&&(sx, sy, _)| (sx, sy) == (x, y))
                    .map_or(0.0, |s| s.2)
            },
            31.0,
        );
        assert_eq!(found, Some((CENTER, CENTER - 2, 2.0)));
        // A radius of zero only looks at the cursor pixel.
        assert_eq!(
            nearest_set_pixel(|_, _| true, |_, _| 0.0, 0.0),
            Some((CENTER, CENTER, 0.0))
        );
        // Radii beyond the window are clamped, so the search never leaves it.
        nearest_set_pixel(
            |x, y| {
                assert!((0..PICK_SIZE as i32).contains(&x) && (0..PICK_SIZE as i32).contains(&y));
                false
            },
            |_, _| 0.0,
            1000.0,
        );
    }
}
