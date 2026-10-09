//! The sketch data model: entities, constraints and dimensions.
//!
//! A sketch is a flat list of entities and a flat list of constraints, both addressed by
//! stable ids. Ids are never reused within a sketch, so a removed entity's id stays dead.
//!
//! **Points carry the geometry.** Lines, arcs, circles and splines reference point entities
//! (their endpoints, centres and fit points) instead of storing coordinates, so
//! "coincident" is a constraint between two points and every solver variable is either a
//! point coordinate or a circle radius. Points created as part of a curve are *owned* by
//! it: they are hidden from the user as separate items and removed together with the curve.
//!
//! **Splines** pass through their fit points and have no other data: the curve is derived
//! from where the points are ([`crate::spline`]), so a spline adds no equations to the
//! solver and its points take the same relations and dimensions as a line's ends.

use peet_math::DVec2;
use serde::{Deserialize, Serialize};

use crate::curve::Curve;

/// Stable identifier of an entity within its sketch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityId(pub u32);

/// Stable identifier of a constraint (or dimension) within its sketch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConstraintId(pub u32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Geometry {
    Point {
        pos: DVec2,
    },
    /// A line segment between two point entities.
    Line {
        start: EntityId,
        end: EntityId,
    },
    /// A full circle. The radius is a solver variable.
    Circle {
        center: EntityId,
        radius: f64,
    },
    /// A circular arc running **counter-clockwise** from `start` to `end` around `center`.
    ///
    /// The radius is `|start - center|`. The solver adds an implicit equation keeping
    /// `|end - center|` equal to it; outside the solver it may differ slightly, and
    /// [`Sketch::curve`] uses the start radius.
    Arc {
        center: EntityId,
        start: EntityId,
        end: EntityId,
    },
    /// A smooth curve through `points`, in order. A closed spline returns to its first
    /// point (which is not repeated in the list) and is smooth there too.
    ///
    /// The curve is a function of the points' positions alone (see [`crate::spline`]).
    Spline {
        points: Vec<EntityId>,
        closed: bool,
    },
}

impl Geometry {
    /// The point entities this geometry references (empty for a point).
    pub fn points(&self) -> Vec<EntityId> {
        match *self {
            Self::Point { .. } => Vec::new(),
            Self::Line { start, end } => vec![start, end],
            Self::Circle { center, .. } => vec![center],
            Self::Arc { center, start, end } => vec![center, start, end],
            Self::Spline { ref points, .. } => points.clone(),
        }
    }
}

/// What kind of entity something is, without its data. Handy for matching on selections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Point,
    Line,
    Circle,
    Arc,
    Spline,
}

impl EntityKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Point => "Point",
            Self::Line => "Line",
            Self::Circle => "Circle",
            Self::Arc => "Arc",
            Self::Spline => "Spline",
        }
    }

    /// Circles and arcs: anything with a centre and a radius.
    pub fn is_circular(self) -> bool {
        matches!(self, Self::Circle | Self::Arc)
    }

    /// Lines, circles, arcs and splines.
    pub fn is_curve(self) -> bool {
        !matches!(self, Self::Point)
    }

    /// Lines, circles and arcs: the curves with an equation the solver can hold a point
    /// to, or make another curve tangent to.
    pub fn is_analytic(self) -> bool {
        matches!(self, Self::Line | Self::Circle | Self::Arc)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub geometry: Geometry,
    /// Construction geometry helps constrain the sketch but is ignored by profiles/regions.
    pub construction: bool,
    /// For points: the curve that created this point, if any. Owned points are removed with
    /// their owner and are not listed as separate items in the UI.
    pub owner: Option<EntityId>,
    /// Locked (reference) entities are constants for the solver: the sketch origin now,
    /// projected model edges later. The user cannot move or delete them.
    pub locked: bool,
}

impl Entity {
    pub fn kind(&self) -> EntityKind {
        match self.geometry {
            Geometry::Point { .. } => EntityKind::Point,
            Geometry::Line { .. } => EntityKind::Line,
            Geometry::Circle { .. } => EntityKind::Circle,
            Geometry::Arc { .. } => EntityKind::Arc,
            Geometry::Spline { .. } => EntityKind::Spline,
        }
    }
}

/// A geometric relation or a dimension.
///
/// Sign and branch choices (which side of a line a point lies on, internal or external
/// tangency, the direction of an angle) are not stored: the solver keeps whichever the
/// current geometry has at the start of each solve. Geometry changes continuously while
/// editing, so this keeps the sketch in the configuration the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConstraintKind {
    // ---- Geometric relations ----
    /// Two points at the same position.
    Coincident(EntityId, EntityId),
    /// A point on a curve: on the infinite line through a line segment, or on the full
    /// circle of a circle or arc.
    PointOnCurve {
        point: EntityId,
        curve: EntityId,
    },
    /// A line parallel to the sketch X axis.
    Horizontal(EntityId),
    /// A line parallel to the sketch Y axis.
    Vertical(EntityId),
    /// Two points at the same Y coordinate.
    HorizontalPoints(EntityId, EntityId),
    /// Two points at the same X coordinate.
    VerticalPoints(EntityId, EntityId),
    Parallel(EntityId, EntityId),
    Perpendicular(EntityId, EntityId),
    /// Line–circle/arc or circle/arc–circle/arc tangency.
    Tangent(EntityId, EntityId),
    /// Equal length (two lines) or equal radius (two circles/arcs).
    Equal(EntityId, EntityId),
    /// Two circles/arcs sharing a centre.
    Concentric(EntityId, EntityId),
    /// A point at the midpoint of a line.
    Midpoint {
        point: EntityId,
        line: EntityId,
    },
    /// Two points mirrored about a line.
    Symmetric {
        a: EntityId,
        b: EntityId,
        axis: EntityId,
    },
    /// A point held at `at`. Fixing a curve adds one `Fix` per defining point (and a radius
    /// dimension for circles), so this only ever applies to points.
    Fix {
        point: EntityId,
        at: DVec2,
    },

    // ---- Dimensions (need `Constraint::dimension`) ----
    /// Point–point distance, or point–line (perpendicular) distance. For a line's length,
    /// use [`ConstraintKind::Length`].
    Distance(EntityId, EntityId),
    Length(EntityId),
    /// `|b.x - a.x|` between two points.
    HorizontalDistance(EntityId, EntityId),
    /// `|b.y - a.y|` between two points.
    VerticalDistance(EntityId, EntityId),
    Radius(EntityId),
    Diameter(EntityId),
    /// Angle between two lines, in degrees (0..=180).
    Angle(EntityId, EntityId),
}

impl ConstraintKind {
    /// All entities the constraint refers to.
    pub fn entities(&self) -> Vec<EntityId> {
        use ConstraintKind::*;
        match *self {
            Horizontal(a) | Vertical(a) | Length(a) | Radius(a) | Diameter(a) => vec![a],
            Fix { point, .. } => vec![point],
            Coincident(a, b)
            | HorizontalPoints(a, b)
            | VerticalPoints(a, b)
            | Parallel(a, b)
            | Perpendicular(a, b)
            | Tangent(a, b)
            | Equal(a, b)
            | Concentric(a, b)
            | Distance(a, b)
            | HorizontalDistance(a, b)
            | VerticalDistance(a, b)
            | Angle(a, b) => vec![a, b],
            PointOnCurve { point, curve } => vec![point, curve],
            Midpoint { point, line } => vec![point, line],
            Symmetric { a, b, axis } => vec![a, b, axis],
        }
    }

    /// Whether this kind is a dimension (has a value) rather than a geometric relation.
    pub fn is_dimension(&self) -> bool {
        use ConstraintKind::*;
        matches!(
            self,
            Distance(..)
                | Length(_)
                | HorizontalDistance(..)
                | VerticalDistance(..)
                | Radius(_)
                | Diameter(_)
                | Angle(..)
        )
    }

    /// Whether the dimension value is an angle in degrees (otherwise a length in mm).
    pub fn is_angular(&self) -> bool {
        matches!(self, ConstraintKind::Angle(..))
    }

    pub fn label(&self) -> &'static str {
        use ConstraintKind::*;
        match self {
            Coincident(..) => "Coincident",
            PointOnCurve { .. } => "Point on curve",
            Horizontal(_) | HorizontalPoints(..) => "Horizontal",
            Vertical(_) | VerticalPoints(..) => "Vertical",
            Parallel(..) => "Parallel",
            Perpendicular(..) => "Perpendicular",
            Tangent(..) => "Tangent",
            Equal(..) => "Equal",
            Concentric(..) => "Concentric",
            Midpoint { .. } => "Midpoint",
            Symmetric { .. } => "Symmetric",
            Fix { .. } => "Fix",
            Distance(..) => "Distance",
            Length(_) => "Length",
            HorizontalDistance(..) => "Horizontal distance",
            VerticalDistance(..) => "Vertical distance",
            Radius(_) => "Radius",
            Diameter(_) => "Diameter",
            Angle(..) => "Angle",
        }
    }
}

/// The value part of a dimension.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    /// The value in display units: mm for lengths, degrees for angles. For driven
    /// dimensions this is the measured value, refreshed after each solve.
    pub value: f64,
    /// The expression the value came from (`"2 * height + 5"`), if it's not a plain number.
    pub expression: Option<String>,
    /// Driving dimensions constrain the geometry. Driven (reference) ones only measure it.
    pub driving: bool,
    /// Name used to refer to this dimension in expressions (`"d1"`). Unique in the sketch.
    pub name: String,
    /// Where the label sits, relative to the dimension's anchor, in sketch units.
    pub label_offset: DVec2,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub kind: ConstraintKind,
    /// `Some` exactly when `kind.is_dimension()`.
    pub dimension: Option<Dimension>,
}

impl Constraint {
    /// Whether the solver should turn this into equations (relations, and driving dimensions).
    pub fn is_driving(&self) -> bool {
        self.dimension.as_ref().is_none_or(|d| d.driving)
    }
}

/// Why an edit to a sketch was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SketchError {
    /// An id doesn't refer to a live entity.
    MissingEntity(EntityId),
    /// An id doesn't refer to a live constraint.
    MissingConstraint(ConstraintId),
    /// The entities don't fit the constraint (for example "parallel" on two points).
    WrongEntityKinds {
        constraint: &'static str,
        expected: &'static str,
    },
    /// Locked entities (the origin, projected geometry) can't be moved or removed.
    Locked(EntityId),
    /// A spline was given too few points to pass through.
    TooFewPoints { closed: bool },
}

impl std::fmt::Display for SketchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEntity(id) => write!(f, "entity {} does not exist", id.0),
            Self::MissingConstraint(id) => write!(f, "constraint {} does not exist", id.0),
            Self::WrongEntityKinds {
                constraint,
                expected,
            } => write!(f, "{constraint} needs {expected}"),
            Self::Locked(_) => write!(f, "reference geometry can't be changed"),
            Self::TooFewPoints { closed: false } => write!(
                f,
                "a spline needs at least two different points to pass through"
            ),
            Self::TooFewPoints { closed: true } => write!(
                f,
                "a closed spline needs at least three different points to pass through; add a \
                 point, or leave it open"
            ),
        }
    }
}

impl std::error::Error for SketchError {}

/// A 2D sketch: entities plus the constraints between them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sketch {
    entities: Vec<Option<Entity>>,
    constraints: Vec<Option<Constraint>>,
    next_dimension_number: u32,
}

impl Default for Sketch {
    fn default() -> Self {
        Self::new()
    }
}

impl Sketch {
    /// The sketch origin: a locked point at (0, 0), present in every sketch.
    pub const ORIGIN: EntityId = EntityId(0);

    pub fn new() -> Self {
        Self {
            entities: vec![Some(Entity {
                geometry: Geometry::Point { pos: DVec2::ZERO },
                construction: true,
                owner: None,
                locked: true,
            })],
            constraints: Vec::new(),
            next_dimension_number: 1,
        }
    }

    // ---- Queries ----

    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.get(id.0 as usize)?.as_ref()
    }

    pub fn entity_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.get_mut(id.0 as usize)?.as_mut()
    }

    pub fn kind(&self, id: EntityId) -> Option<EntityKind> {
        self.entity(id).map(Entity::kind)
    }

    /// All live entities, in id order.
    pub fn entities(&self) -> impl Iterator<Item = (EntityId, &Entity)> {
        self.entities
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((EntityId(i as u32), e.as_ref()?)))
    }

    /// Number of id slots (live or dead). Useful for dense per-entity tables.
    pub fn entity_capacity(&self) -> usize {
        self.entities.len()
    }

    pub fn constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.get(id.0 as usize)?.as_ref()
    }

    pub fn constraint_mut(&mut self, id: ConstraintId) -> Option<&mut Constraint> {
        self.constraints.get_mut(id.0 as usize)?.as_mut()
    }

    /// All live constraints and dimensions, in id order.
    pub fn constraints(&self) -> impl Iterator<Item = (ConstraintId, &Constraint)> {
        self.constraints
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((ConstraintId(i as u32), c.as_ref()?)))
    }

    pub fn constraint_capacity(&self) -> usize {
        self.constraints.len()
    }

    /// Constraints that reference `entity` directly.
    pub fn constraints_on(&self, entity: EntityId) -> impl Iterator<Item = ConstraintId> + '_ {
        self.constraints()
            .filter(move |(_, c)| c.kind.entities().contains(&entity))
            .map(|(id, _)| id)
    }

    /// Position of a point entity. Panics if `id` is not a live point: callers hold ids
    /// they got from this sketch, so a bad id is a programming error.
    pub fn point(&self, id: EntityId) -> DVec2 {
        self.try_point(id)
            .unwrap_or_else(|| panic!("entity {} is not a point", id.0))
    }

    pub fn try_point(&self, id: EntityId) -> Option<DVec2> {
        match self.entity(id)?.geometry {
            Geometry::Point { pos } => Some(pos),
            _ => None,
        }
    }

    /// Sets a point's position. Ignores locked points and non-points.
    pub fn set_point(&mut self, id: EntityId, pos: DVec2) {
        if let Some(Entity {
            geometry: Geometry::Point { pos: p },
            locked: false,
            ..
        }) = self.entity_mut(id)
        {
            *p = pos;
        }
    }

    /// Resolved geometry of a line, circle, arc or spline (`None` for points and dead ids).
    pub fn curve(&self, id: EntityId) -> Option<Curve> {
        match self.entity(id)?.geometry {
            Geometry::Spline { ref points, closed } => {
                let through = points
                    .iter()
                    .map(|p| self.try_point(*p))
                    .collect::<Option<Vec<DVec2>>>()?;
                Some(Curve::spline_through(&through, closed))
            }
            Geometry::Point { .. } => None,
            Geometry::Line { start, end } => Some(Curve::Line {
                a: self.try_point(start)?,
                b: self.try_point(end)?,
            }),
            Geometry::Circle { center, radius } => Some(Curve::Circle {
                center: self.try_point(center)?,
                radius,
            }),
            Geometry::Arc { center, start, end } => {
                let c = self.try_point(center)?;
                Some(Curve::arc_from_points(
                    c,
                    self.try_point(start)?,
                    self.try_point(end)?,
                ))
            }
        }
    }

    /// Endpoints of a line, arc or open spline (start, end). `None` for other entities.
    pub fn endpoints(&self, id: EntityId) -> Option<(EntityId, EntityId)> {
        match self.entity(id)?.geometry {
            Geometry::Line { start, end } | Geometry::Arc { start, end, .. } => Some((start, end)),
            Geometry::Spline {
                ref points,
                closed: false,
            } => Some((*points.first()?, *points.last()?)),
            _ => None,
        }
    }

    /// The fit points of a spline, in order, and whether it is closed. `None` for other
    /// entities.
    pub fn spline_points(&self, id: EntityId) -> Option<(&[EntityId], bool)> {
        match &self.entity(id)?.geometry {
            Geometry::Spline { points, closed } => Some((points, *closed)),
            _ => None,
        }
    }

    /// Centre point of a circle or arc.
    pub fn center(&self, id: EntityId) -> Option<EntityId> {
        match self.entity(id)?.geometry {
            Geometry::Circle { center, .. } | Geometry::Arc { center, .. } => Some(center),
            _ => None,
        }
    }

    /// Curves that use `point` as an endpoint or centre.
    pub fn curves_using(&self, point: EntityId) -> impl Iterator<Item = EntityId> + '_ {
        self.entities()
            .filter(move |(_, e)| e.geometry.points().contains(&point))
            .map(|(id, _)| id)
    }

    /// The current measured value of a dimension kind, in display units (mm or degrees).
    /// Returns `None` if the entities don't fit the kind.
    pub fn measure(&self, kind: &ConstraintKind) -> Option<f64> {
        use ConstraintKind::*;
        match *kind {
            Distance(a, b) => match (self.kind(a)?, self.kind(b)?) {
                (EntityKind::Point, EntityKind::Point) => {
                    Some(self.point(a).distance(self.point(b)))
                }
                (EntityKind::Point, EntityKind::Line) => {
                    Some(self.curve(b)?.distance_to_line(self.point(a)))
                }
                (EntityKind::Line, EntityKind::Point) => {
                    Some(self.curve(a)?.distance_to_line(self.point(b)))
                }
                _ => None,
            },
            Length(line) => match self.curve(line)? {
                Curve::Line { a, b } => Some(a.distance(b)),
                _ => None,
            },
            HorizontalDistance(a, b) => Some((self.try_point(b)?.x - self.try_point(a)?.x).abs()),
            VerticalDistance(a, b) => Some((self.try_point(b)?.y - self.try_point(a)?.y).abs()),
            Radius(c) => self.curve(c)?.radius(),
            Diameter(c) => self.curve(c)?.radius().map(|r| r * 2.0),
            Angle(l1, l2) => {
                let d1 = self.curve(l1)?.line_direction()?;
                let d2 = self.curve(l2)?.line_direction()?;
                Some(d1.perp_dot(d2).atan2(d1.dot(d2)).abs().to_degrees())
            }
            _ => None,
        }
    }

    // ---- Adding geometry ----

    fn push_entity(&mut self, entity: Entity) -> EntityId {
        let id = EntityId(self.entities.len() as u32);
        self.entities.push(Some(entity));
        id
    }

    fn push_owned_point(&mut self, pos: DVec2, owner: EntityId, construction: bool) -> EntityId {
        self.push_entity(Entity {
            geometry: Geometry::Point { pos },
            construction,
            owner: Some(owner),
            locked: false,
        })
    }

    /// Adds a free-standing point.
    pub fn add_point(&mut self, pos: DVec2) -> EntityId {
        self.push_entity(Entity {
            geometry: Geometry::Point { pos },
            construction: false,
            owner: None,
            locked: false,
        })
    }

    /// Adds a line segment, with two new endpoint points owned by it.
    pub fn add_line(&mut self, a: DVec2, b: DVec2) -> EntityId {
        let id = EntityId(self.entities.len() as u32);
        let start = EntityId(id.0 + 1);
        let end = EntityId(id.0 + 2);
        self.push_entity(Entity {
            geometry: Geometry::Line { start, end },
            construction: false,
            owner: None,
            locked: false,
        });
        self.push_owned_point(a, id, false);
        self.push_owned_point(b, id, false);
        id
    }

    /// Adds a circle, with a new centre point owned by it.
    pub fn add_circle(&mut self, center: DVec2, radius: f64) -> EntityId {
        let id = EntityId(self.entities.len() as u32);
        self.push_entity(Entity {
            geometry: Geometry::Circle {
                center: EntityId(id.0 + 1),
                radius: radius.abs(),
            },
            construction: false,
            owner: None,
            locked: false,
        });
        self.push_owned_point(center, id, false);
        id
    }

    /// Adds a counter-clockwise arc from `start` to `end` around `center`, with three new
    /// points owned by it. `end` is moved onto the circle through `start` if needed.
    pub fn add_arc(&mut self, center: DVec2, start: DVec2, end: DVec2) -> EntityId {
        let radius = start.distance(center);
        let end = center + (end - center).normalize_or(DVec2::X) * radius;
        let id = EntityId(self.entities.len() as u32);
        self.push_entity(Entity {
            geometry: Geometry::Arc {
                center: EntityId(id.0 + 1),
                start: EntityId(id.0 + 2),
                end: EntityId(id.0 + 3),
            },
            construction: false,
            owner: None,
            locked: false,
        });
        self.push_owned_point(center, id, false);
        self.push_owned_point(start, id, false);
        self.push_owned_point(end, id, false);
        id
    }

    /// Adds a spline through `through`, in order, with a new point for each owned by it.
    /// `closed` brings it back to the first point (which must not be repeated at the
    /// end). An open spline needs two different points, a closed one three.
    pub fn add_spline(&mut self, through: &[DVec2], closed: bool) -> Result<EntityId, SketchError> {
        let mut different: Vec<DVec2> = Vec::new();
        for p in through {
            if !different.iter().any(|q| q.distance(*p) <= 1e-9) {
                different.push(*p);
            }
        }
        if different.len() < if closed { 3 } else { 2 } || through.iter().any(|p| !p.is_finite()) {
            return Err(SketchError::TooFewPoints { closed });
        }
        let id = EntityId(self.entities.len() as u32);
        let points = (1..=through.len() as u32)
            .map(|i| EntityId(id.0 + i))
            .collect();
        self.push_entity(Entity {
            geometry: Geometry::Spline { points, closed },
            construction: false,
            owner: None,
            locked: false,
        });
        for p in through {
            self.push_owned_point(*p, id, false);
        }
        Ok(id)
    }

    /// Marks an entity (and the points it owns) as construction geometry or not.
    pub fn set_construction(&mut self, id: EntityId, construction: bool) {
        let Some(entity) = self.entity_mut(id) else {
            return;
        };
        if entity.locked {
            return;
        }
        entity.construction = construction;
        let owned: Vec<EntityId> = entity.geometry.points();
        for p in owned {
            if let Some(e) = self.entity_mut(p)
                && e.owner == Some(id)
            {
                e.construction = construction;
            }
        }
    }

    // ---- Constraints ----

    /// Adds a geometric relation after checking that the entities fit it.
    pub fn add_constraint(&mut self, kind: ConstraintKind) -> Result<ConstraintId, SketchError> {
        self.validate(&kind)?;
        if kind.is_dimension() {
            let value = self.measure(&kind).unwrap_or(0.0);
            return self.add_dimension(kind, value);
        }
        Ok(self.push_constraint(Constraint {
            kind,
            dimension: None,
        }))
    }

    /// Adds a driving dimension with the given value (mm or degrees) and an automatic name.
    pub fn add_dimension(
        &mut self,
        kind: ConstraintKind,
        value: f64,
    ) -> Result<ConstraintId, SketchError> {
        self.validate(&kind)?;
        if !kind.is_dimension() {
            return self.add_constraint(kind);
        }
        let name = self.fresh_dimension_name();
        Ok(self.push_constraint(Constraint {
            kind,
            dimension: Some(Dimension {
                value,
                expression: None,
                driving: true,
                name,
                label_offset: DVec2::ZERO,
            }),
        }))
    }

    fn fresh_dimension_name(&mut self) -> String {
        loop {
            let name = format!("d{}", self.next_dimension_number);
            self.next_dimension_number += 1;
            if !self
                .constraints()
                .any(|(_, c)| c.dimension.as_ref().is_some_and(|d| d.name == name))
            {
                return name;
            }
        }
    }

    fn push_constraint(&mut self, c: Constraint) -> ConstraintId {
        let id = ConstraintId(self.constraints.len() as u32);
        self.constraints.push(Some(c));
        id
    }

    /// Checks that a constraint's entities exist and have suitable kinds.
    pub fn validate(&self, kind: &ConstraintKind) -> Result<(), SketchError> {
        use ConstraintKind::*;
        use EntityKind as K;
        for id in kind.entities() {
            if self.entity(id).is_none() {
                return Err(SketchError::MissingEntity(id));
            }
        }
        let k = |id| self.kind(id).expect("checked above");
        let wrong = |expected| {
            Err(SketchError::WrongEntityKinds {
                constraint: kind.label(),
                expected,
            })
        };
        let ok = match *kind {
            Coincident(a, b) => k(a) == K::Point && k(b) == K::Point && a != b,
            PointOnCurve { point, curve } => k(point) == K::Point && k(curve).is_analytic(),
            Horizontal(l) | Vertical(l) | Length(l) => k(l) == K::Line,
            HorizontalPoints(a, b)
            | VerticalPoints(a, b)
            | HorizontalDistance(a, b)
            | VerticalDistance(a, b) => k(a) == K::Point && k(b) == K::Point && a != b,
            Parallel(a, b) | Perpendicular(a, b) | Angle(a, b) => {
                k(a) == K::Line && k(b) == K::Line && a != b
            }
            Tangent(a, b) => {
                a != b
                    && ((k(a) == K::Line && k(b).is_circular())
                        || (k(a).is_circular() && (k(b) == K::Line || k(b).is_circular())))
            }
            Equal(a, b) => {
                a != b
                    && ((k(a) == K::Line && k(b) == K::Line)
                        || (k(a).is_circular() && k(b).is_circular()))
            }
            Concentric(a, b) => a != b && k(a).is_circular() && k(b).is_circular(),
            Midpoint { point, line } => k(point) == K::Point && k(line) == K::Line,
            Symmetric { a, b, axis } => {
                k(a) == K::Point && k(b) == K::Point && a != b && k(axis) == K::Line
            }
            Fix { point, .. } => k(point) == K::Point,
            Distance(a, b) => {
                a != b
                    && matches!(
                        (k(a), k(b)),
                        (K::Point, K::Point) | (K::Point, K::Line) | (K::Line, K::Point)
                    )
            }
            Radius(c) | Diameter(c) => k(c).is_circular(),
        };
        if ok {
            Ok(())
        } else {
            wrong(expected_kinds(kind))
        }
    }

    /// Adds a `Fix` for the entity: one per defining point, plus a radius dimension for a
    /// circle. Returns the new constraints.
    pub fn fix(&mut self, id: EntityId) -> Result<Vec<ConstraintId>, SketchError> {
        let entity = self.entity(id).ok_or(SketchError::MissingEntity(id))?;
        let mut added = Vec::new();
        let points = match &entity.geometry {
            Geometry::Point { .. } => vec![id],
            g => g.points(),
        };
        let is_circle = entity.kind() == EntityKind::Circle;
        for p in points {
            if self.entity(p).is_some_and(|e| e.locked) {
                continue;
            }
            let at = self.point(p);
            added.push(self.push_constraint(Constraint {
                kind: ConstraintKind::Fix { point: p, at },
                dimension: None,
            }));
        }
        if is_circle {
            added.push(self.add_constraint(ConstraintKind::Radius(id))?);
        }
        Ok(added)
    }

    pub fn remove_constraint(&mut self, id: ConstraintId) -> Option<Constraint> {
        self.constraints.get_mut(id.0 as usize)?.take()
    }

    /// Removes an entity, the points it owns (unless other curves still use them) and every
    /// constraint referring to anything removed. Returns the removed entity ids.
    ///
    /// Removing a point removes the curves that use it, except that a spline with points
    /// to spare just stops passing through it.
    pub fn remove_entity(&mut self, id: EntityId) -> Result<Vec<EntityId>, SketchError> {
        let entity = self.entity(id).ok_or(SketchError::MissingEntity(id))?;
        if entity.locked {
            return Err(SketchError::Locked(id));
        }
        let mut removed = vec![id];
        let owned: Vec<EntityId> = entity
            .geometry
            .points()
            .into_iter()
            .filter(|p| self.entity(*p).is_some_and(|e| e.owner == Some(id)))
            .collect();
        match entity.kind() {
            EntityKind::Point => {
                // Removing a curve's point removes the curve too.
                let users: Vec<EntityId> = self.curves_using(id).collect();
                self.entities[id.0 as usize] = None;
                for user in users {
                    if self.drop_spline_point(user, id) {
                        continue;
                    }
                    if self.entity(user).is_some() {
                        removed.extend(self.remove_entity(user)?);
                    }
                }
            }
            _ => {
                self.entities[id.0 as usize] = None;
                for p in owned {
                    if self.curves_using(p).next().is_none() {
                        self.entities[p.0 as usize] = None;
                        removed.push(p);
                    } else if let Some(e) = self.entity_mut(p) {
                        e.owner = None;
                    }
                }
            }
        }
        self.remove_dangling_constraints();
        Ok(removed)
    }

    /// Takes `point` out of the spline `curve` if the spline has enough other points left
    /// (two, or three if it is closed). Returns whether it did.
    fn drop_spline_point(&mut self, curve: EntityId, point: EntityId) -> bool {
        let Some(Entity {
            geometry: Geometry::Spline { points, closed },
            ..
        }) = self.entity_mut(curve)
        else {
            return false;
        };
        let left = points.iter().filter(|p| **p != point).count();
        if left < if *closed { 3 } else { 2 } {
            return false;
        }
        points.retain(|p| *p != point);
        true
    }

    /// Removes constraints that reference entities that no longer exist.
    pub fn remove_dangling_constraints(&mut self) {
        for i in 0..self.constraints.len() {
            let dangling = self.constraints[i]
                .as_ref()
                .is_some_and(|c| c.kind.entities().iter().any(|e| self.entity(*e).is_none()));
            if dangling {
                self.constraints[i] = None;
            }
        }
    }

    /// Replaces a point in a curve's definition (for example to share an endpoint with
    /// another curve instead of using a coincident constraint). Returns false if `curve`
    /// doesn't use `old` or `new` isn't a point.
    pub fn replace_curve_point(&mut self, curve: EntityId, old: EntityId, new: EntityId) -> bool {
        if self.kind(new) != Some(EntityKind::Point) {
            return false;
        }
        let Some(entity) = self.entity_mut(curve) else {
            return false;
        };
        let mut changed = false;
        let mut swap = |p: &mut EntityId| {
            if *p == old {
                *p = new;
                changed = true;
            }
        };
        match &mut entity.geometry {
            Geometry::Point { .. } => {}
            Geometry::Line { start, end } => {
                swap(start);
                swap(end);
            }
            Geometry::Circle { center, .. } => swap(center),
            Geometry::Arc { center, start, end } => {
                swap(center);
                swap(start);
                swap(end);
            }
            Geometry::Spline { points, .. } => points.iter_mut().for_each(swap),
        }
        changed
    }

    /// Sets a circle's radius. Ignores other entities.
    pub fn set_radius(&mut self, id: EntityId, r: f64) {
        if let Some(Entity {
            geometry: Geometry::Circle { radius, .. },
            locked: false,
            ..
        }) = self.entity_mut(id)
        {
            *radius = r.abs();
        }
    }

    /// Finds an existing dimension by name.
    pub fn dimension_by_name(&self, name: &str) -> Option<ConstraintId> {
        self.constraints()
            .find(|(_, c)| c.dimension.as_ref().is_some_and(|d| d.name == name))
            .map(|(id, _)| id)
    }

    /// Refreshes the value of every driven (reference) dimension from the geometry.
    pub fn update_driven_dimensions(&mut self) {
        for i in 0..self.constraints.len() {
            let Some(c) = &self.constraints[i] else {
                continue;
            };
            if c.dimension.as_ref().is_some_and(|d| !d.driving)
                && let Some(v) = self.measure(&c.kind)
                && let Some(Some(Constraint {
                    dimension: Some(d), ..
                })) = self.constraints.get_mut(i)
            {
                d.value = v;
            }
        }
    }
}

fn expected_kinds(kind: &ConstraintKind) -> &'static str {
    use ConstraintKind::*;
    match kind {
        Coincident(..) | HorizontalPoints(..) | VerticalPoints(..) => "two different points",
        HorizontalDistance(..) | VerticalDistance(..) => "two different points",
        PointOnCurve { .. } => {
            "a point and a line, circle or arc (a point can't be held on a spline: make it \
             coincident with one of the spline's points instead)"
        }
        Horizontal(_) | Vertical(_) | Length(_) => "a line",
        Parallel(..) | Perpendicular(..) | Angle(..) => "two lines",
        Tangent(..) => "a line and a circle/arc, or two circles/arcs",
        Equal(..) => "two lines, or two circles/arcs",
        Concentric(..) => "two circles/arcs",
        Midpoint { .. } => "a point and a line",
        Symmetric { .. } => "two points and a line",
        Fix { .. } => "a point",
        Distance(..) => "two points, or a point and a line",
        Radius(_) | Diameter(_) => "a circle or arc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_is_locked() {
        let mut s = Sketch::new();
        assert_eq!(s.point(Sketch::ORIGIN), DVec2::ZERO);
        assert_eq!(
            s.remove_entity(Sketch::ORIGIN),
            Err(SketchError::Locked(Sketch::ORIGIN))
        );
        s.set_point(Sketch::ORIGIN, DVec2::ONE);
        assert_eq!(s.point(Sketch::ORIGIN), DVec2::ZERO);
    }

    #[test]
    fn line_owns_its_points() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let (a, b) = s.endpoints(l).unwrap();
        assert_eq!(s.entity(a).unwrap().owner, Some(l));
        assert_eq!(s.measure(&ConstraintKind::Length(l)), Some(10.0));
        let c = s.add_constraint(ConstraintKind::Horizontal(l)).unwrap();
        let removed = s.remove_entity(l).unwrap();
        assert_eq!(removed.len(), 3);
        assert!(s.entity(a).is_none() && s.entity(b).is_none());
        assert!(
            s.constraint(c).is_none(),
            "constraint removed with the line"
        );
    }

    #[test]
    fn shared_point_survives() {
        let mut s = Sketch::new();
        let l1 = s.add_line(DVec2::ZERO, DVec2::X);
        let l2 = s.add_line(DVec2::X, DVec2::ONE);
        let (_, b1) = s.endpoints(l1).unwrap();
        let (a2, _) = s.endpoints(l2).unwrap();
        assert!(s.replace_curve_point(l2, a2, b1));
        s.remove_entity(a2).unwrap();
        s.remove_entity(l1).unwrap();
        assert!(s.entity(b1).is_some(), "still used by l2");
        assert_eq!(s.entity(b1).unwrap().owner, None);
        // Removing the point removes the curve using it.
        let removed = s.remove_entity(b1).unwrap();
        assert!(removed.contains(&l2));
    }

    #[test]
    fn validation() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::X);
        let c = s.add_circle(DVec2::ZERO, 2.0);
        assert!(s.add_constraint(ConstraintKind::Parallel(l, c)).is_err());
        assert!(s.add_constraint(ConstraintKind::Tangent(l, c)).is_ok());
        assert!(s.add_constraint(ConstraintKind::Tangent(l, l)).is_err());
        assert!(matches!(
            s.add_constraint(ConstraintKind::Horizontal(EntityId(999))),
            Err(SketchError::MissingEntity(_))
        ));
    }

    #[test]
    fn dimensions_get_names_and_values() {
        let mut s = Sketch::new();
        let c = s.add_circle(DVec2::ZERO, 2.5);
        let d = s.add_constraint(ConstraintKind::Diameter(c)).unwrap();
        let dim = s.constraint(d).unwrap().dimension.as_ref().unwrap();
        assert_eq!(dim.name, "d1");
        assert_eq!(dim.value, 5.0);
        let d2 = s.add_dimension(ConstraintKind::Radius(c), 3.0).unwrap();
        assert_eq!(s.dimension_by_name("d2"), Some(d2));
    }

    #[test]
    fn spline_owns_its_fit_points() {
        let mut s = Sketch::new();
        let through = [
            DVec2::ZERO,
            DVec2::new(5.0, 4.0),
            DVec2::new(10.0, -2.0),
            DVec2::new(15.0, 0.0),
        ];
        let id = s.add_spline(&through, false).unwrap();
        assert_eq!(s.kind(id), Some(EntityKind::Spline));
        let (points, closed) = s.spline_points(id).unwrap();
        let points = points.to_vec();
        assert!(!closed);
        assert_eq!(points.len(), 4);
        assert_eq!(s.endpoints(id), Some((points[0], points[3])));
        for (p, at) in points.iter().zip(through) {
            assert_eq!(s.entity(*p).unwrap().owner, Some(id));
            assert_eq!(s.point(*p), at);
            // The curve passes through every one of them.
            assert!(s.curve(id).unwrap().distance(at) < 1e-9);
        }
        // Moving a point moves the curve.
        s.set_point(points[1], DVec2::new(5.0, 9.0));
        assert!(s.curve(id).unwrap().distance(DVec2::new(5.0, 9.0)) < 1e-9);
        // Its points take the relations a line's ends do, but nothing rides on the curve.
        let l = s.add_line(DVec2::new(15.0, 0.0), DVec2::new(20.0, 0.0));
        let (start, _) = s.endpoints(l).unwrap();
        assert!(
            s.add_constraint(ConstraintKind::Coincident(points[3], start))
                .is_ok()
        );
        let err = s
            .add_constraint(ConstraintKind::PointOnCurve {
                point: start,
                curve: id,
            })
            .unwrap_err();
        assert!(err.to_string().contains("spline"), "{err}");
        assert!(s.add_constraint(ConstraintKind::Tangent(l, id)).is_err());
        assert_eq!(s.fix(id).unwrap().len(), 4);
        // A point to spare can go; the spline stays.
        let removed = s.remove_entity(points[1]).unwrap();
        assert_eq!(removed, vec![points[1]]);
        assert_eq!(s.spline_points(id).unwrap().0.len(), 3);
        s.remove_entity(points[2]).unwrap();
        assert!(s.entity(id).is_some());
        // With two left, removing one removes the spline.
        let removed = s.remove_entity(points[0]).unwrap();
        assert!(removed.contains(&id));
        assert!(s.entity(points[3]).is_none());
    }

    #[test]
    fn splines_need_enough_points() {
        let mut s = Sketch::new();
        let p = DVec2::new(1.0, 2.0);
        assert_eq!(
            s.add_spline(&[p], false),
            Err(SketchError::TooFewPoints { closed: false })
        );
        assert_eq!(
            s.add_spline(&[p, p], false),
            Err(SketchError::TooFewPoints { closed: false })
        );
        let err = s.add_spline(&[p, DVec2::ZERO], true).unwrap_err();
        assert!(err.to_string().contains("three"), "{err}");
        assert_eq!(s.entities().count(), 1, "nothing was added");
        let closed = s
            .add_spline(&[p, DVec2::ZERO, DVec2::new(4.0, -3.0)], true)
            .unwrap();
        assert!(s.curve(closed).unwrap().is_closed());
        assert_eq!(s.endpoints(closed), None);
        s.set_construction(closed, true);
        let (points, _) = s.spline_points(closed).unwrap();
        assert!(points.iter().all(|p| s.entity(*p).unwrap().construction));
    }

    #[test]
    fn angle_measure() {
        let mut s = Sketch::new();
        let a = s.add_line(DVec2::ZERO, DVec2::X);
        let b = s.add_line(DVec2::ZERO, DVec2::new(1.0, 1.0));
        let m = s.measure(&ConstraintKind::Angle(a, b)).unwrap();
        assert!((m - 45.0).abs() < 1e-9);
    }
}
