//! Simple solids built directly: blocks and balls. Features use them as tools (a block
//! to trim with, a ball for a rounded corner), and tests as subjects.

use std::f64::consts::TAU;

use peet_math::{DVec2, DVec3, Frame, Plane};
use peet_sketch::region::{Loop, LoopEdge, Region};
use peet_sketch::{Curve, EntityId};

use crate::geom::Surface;
use crate::revolve::{RevolveAxis, revolve};
use crate::topo::{EdgeId, VertexId};
use crate::{KernelError, Solid};

/// An axis-aligned box from `min` to `max`, with outward-facing faces.
pub fn cuboid(min: DVec3, max: DVec3) -> Solid {
    let mut s = Solid::new();
    let shell = s.add_shell();
    let c = |i: usize| {
        DVec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    };
    let v: Vec<VertexId> = (0..8).map(|i| s.add_vertex(c(i))).collect();
    // Each face: four corner indices, counter-clockwise seen from outside, and its normal.
    let faces: [([usize; 4], DVec3); 6] = [
        ([0, 2, 3, 1], -DVec3::Z),
        ([4, 5, 7, 6], DVec3::Z),
        ([0, 1, 5, 4], -DVec3::Y),
        ([2, 6, 7, 3], DVec3::Y),
        ([0, 4, 6, 2], -DVec3::X),
        ([1, 3, 7, 5], DVec3::X),
    ];
    let mut edges: std::collections::HashMap<(usize, usize), EdgeId> = Default::default();
    for (corners, normal) in faces {
        let origin = c(corners[0]);
        let x = c(corners[1]) - origin;
        let plane = Plane::from_origin_normal_x(origin, normal, x).expect("valid face");
        let face = s.add_face(shell, Surface::Plane(plane), false);
        let mut uses = Vec::new();
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            let key = (a.min(b), a.max(b));
            let edge = *edges
                .entry(key)
                .or_insert_with(|| s.add_line_edge(v[key.0], v[key.1]));
            uses.push((edge, a > b));
        }
        s.add_loop(face, &uses);
    }
    s
}

/// A ball of `radius` centred on the frame's origin, its poles on the frame's Z axis and
/// its seam towards the frame's X axis.
pub fn ball(frame: &Frame, radius: f64) -> Result<Solid, KernelError> {
    let arc = Curve::Arc {
        center: DVec2::ZERO,
        radius,
        start_angle: -std::f64::consts::FRAC_PI_2,
        sweep: std::f64::consts::PI,
    };
    let line = Curve::Line {
        a: DVec2::new(0.0, radius),
        b: DVec2::new(0.0, -radius),
    };
    let edge = |entity: u32, curve: Curve| LoopEdge {
        entity: EntityId(entity),
        curve,
        reversed: false,
    };
    let region = Region {
        outer: Loop {
            edges: vec![edge(0, arc), edge(1, line)],
            signed_area: 0.5 * std::f64::consts::PI * radius * radius,
        },
        holes: Vec::new(),
    };
    // A plane holding the frame's Z axis as its own Y axis, and the frame's X as its X.
    let plane = Plane::from_origin_normal_x(
        frame.origin,
        frame.x_axis().cross(frame.z_axis()),
        frame.x_axis(),
    )
    .ok_or_else(|| KernelError::InvalidInput("the ball's frame is degenerate".to_owned()))?;
    let axis = RevolveAxis {
        origin: DVec2::ZERO,
        dir: DVec2::Y,
    };
    revolve(&plane, &[region], &axis, 0.0, TAU)
}
