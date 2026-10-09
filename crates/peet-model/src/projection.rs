//! Persistent model geometry projected into a sketch before solving.
use crate::{EdgeRef, PlaneRef, PointRef};
use peet_kernel::Curve3;
use peet_math::{DVec2, Plane};
use peet_sketch::{EntityId, Geometry, Sketch};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Edge(EdgeRef),
    Point(PointRef),
    Plane(PlaneRef),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub entity: EntityId,
    pub source: Source,
}

pub enum Shape {
    Point(DVec2),
    Line(DVec2, DVec2),
    Circle(DVec2, f64),
    Arc(DVec2, DVec2, DVec2),
}

pub fn project_edge(edge: &peet_kernel::topo::Edge, plane: Plane) -> Result<Shape, String> {
    let uv = |p| plane.to_plane_coords(p);
    let a = uv(edge.point_at_fraction(0.0));
    let b = uv(edge.point_at_fraction(1.0));
    match &edge.curve {
        Curve3::Line(_) if a.distance(b) > 1e-9 => Ok(Shape::Line(a, b)),
        Curve3::Circle(c) if c.frame.z_axis().cross(plane.normal()).length() < 1e-9 => {
            let center = uv(c.frame.origin);
            if edge.is_closed() {
                Ok(Shape::Circle(center, a.distance(center)))
            } else {
                let mid = uv(edge.point_at_fraction(0.5));
                let angle = |p: DVec2| (p-center).y.atan2((p-center).x);
                let sweep = (angle(b)-angle(a)).rem_euclid(std::f64::consts::TAU);
                if (angle(mid)-angle(a)).rem_euclid(std::f64::consts::TAU) < sweep {
                    Ok(Shape::Arc(center, a, b))
                } else { Ok(Shape::Arc(center, b, a)) }
            }
        }
        _ => Err("Project a straight edge or a circular edge parallel to the sketch plane. This edge projects to a collapsed line or an unsupported curve.".into()),
    }
}

pub fn intersect_plane(sketch: Plane, other: Plane) -> Result<Shape, String> {
    let n = DVec2::new(
        other.normal().dot(sketch.frame.x_axis()),
        other.normal().dot(sketch.frame.y_axis()),
    );
    if n.length_squared() < 1e-18 {
        return Err(
            "The reference plane is parallel to the sketch. Pick a plane that intersects it."
                .into(),
        );
    }
    let a = n * (-other.signed_distance(sketch.origin()) / n.length_squared());
    Ok(Shape::Line(a, a + n.perp().normalize()))
}

impl Shape {
    pub fn add(&self, sketch: &mut Sketch, construction: bool) -> EntityId {
        let id = match *self {
            Self::Point(p) => sketch.add_point(p),
            Self::Line(a, b) => sketch.add_line(a, b),
            Self::Circle(c, r) => sketch.add_circle(c, r),
            Self::Arc(c, a, b) => sketch.add_arc(c, a, b),
        };
        sketch.set_construction(id, construction);
        let mut ids = sketch.entity(id).unwrap().geometry.points();
        ids.push(id);
        for id in ids {
            sketch.entity_mut(id).unwrap().locked = true;
        }
        id
    }

    pub fn update(&self, sketch: &mut Sketch, id: EntityId) -> Result<(), String> {
        let geometry = sketch
            .entity(id)
            .ok_or("The projected entity was deleted.")?
            .geometry
            .clone();
        let mut points = Vec::new();
        match (self, geometry) {
            (Self::Point(p), Geometry::Point { .. }) => points.push((id, *p)),
            (Self::Line(a, b), Geometry::Line { start, end }) => {
                points.extend([(start, *a), (end, *b)])
            }
            (Self::Circle(c, r), Geometry::Circle { center, .. }) => {
                points.push((center, *c));
                if let Geometry::Circle { radius, .. } =
                    &mut sketch.entity_mut(id).unwrap().geometry
                {
                    *radius = *r;
                }
            }
            (Self::Arc(c, a, b), Geometry::Arc { center, start, end }) => {
                points.extend([(center, *c), (start, *a), (end, *b)])
            }
            _ => return Err(
                "The projected edge changed geometry kind. Replace its projection and relations."
                    .into(),
            ),
        }
        for (id, pos) in points {
            sketch.entity_mut(id).unwrap().geometry = Geometry::Point { pos };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_kernel::VertexId;
    use peet_kernel::geom::{Circle3, Line3};
    use peet_math::{DVec3, Frame};
    use std::f64::consts::{FRAC_PI_2, TAU};

    fn edge(curve: Curve3, t1: f64, closed: bool) -> peet_kernel::topo::Edge {
        peet_kernel::topo::Edge {
            curve,
            t0: 0.,
            t1,
            start: VertexId(0),
            end: VertexId(u32::from(!closed)),
            coedges: Vec::new(),
        }
    }
    #[test]
    fn arc_projection_keeps_the_same_piece_when_viewed_from_below() {
        let edge = edge(
            Curve3::Circle(Circle3 {
                frame: Frame::WORLD,
                radius: 10.,
            }),
            FRAC_PI_2,
            false,
        );
        let plane = Plane::from_origin_normal_x(DVec3::ZERO, -DVec3::Z, DVec3::X).unwrap();
        let Shape::Arc(c, a, b) = project_edge(&edge, plane).unwrap() else {
            panic!("expected arc")
        };
        assert!(c.abs_diff_eq(DVec2::ZERO, 1e-9));
        assert!(a.abs_diff_eq(DVec2::new(0., -10.), 1e-9));
        assert!(b.abs_diff_eq(DVec2::new(10., 0.), 1e-9));
    }
    #[test]
    fn tilted_circle_and_collapsed_line_are_refused() {
        let circle = edge(
            Curve3::Circle(Circle3 {
                frame: Frame::WORLD,
                radius: 10.,
            }),
            TAU,
            true,
        );
        assert!(project_edge(&circle, Plane::front()).is_err());
        let line = edge(
            Curve3::Line(Line3 {
                origin: DVec3::ZERO,
                dir: DVec3::Z,
            }),
            5.,
            false,
        );
        assert!(project_edge(&line, Plane::TOP).is_err());
    }
}
