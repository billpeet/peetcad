//! STL export (binary and ASCII).
//!
//! Binary layout: an 80-byte header, a little-endian `u32` triangle count, then 50 bytes
//! per triangle (normal and three corners as `f32` triples, plus a `u16` attribute that is
//! always 0). Coordinates are written as given (PeetCAD models are in millimetres, the
//! usual assumption for STL).

use std::fmt::Write as _;

use peet_math::DVec3;

/// Length of the binary header.
pub const HEADER_LEN: usize = 80;

/// Bytes per triangle record in a binary STL.
pub const TRIANGLE_LEN: usize = 50;

/// Facet normal from the corners (counter-clockwise seen from outside); zero for a
/// degenerate triangle, which readers recompute or ignore.
fn facet_normal(t: &[DVec3; 3]) -> DVec3 {
    (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero()
}

/// The 80-byte header: the name, cut at a character boundary and padded with zeros. A name
/// starting with "solid" would make some readers mistake the file for ASCII, so it gets a
/// prefix.
fn header(name: &str) -> [u8; HEADER_LEN] {
    let mut text = String::new();
    if name
        .trim_start()
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("solid"))
    {
        text.push_str("binary ");
    }
    text.push_str(name);
    let mut end = text.len().min(HEADER_LEN);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = [0u8; HEADER_LEN];
    out[..end].copy_from_slice(&text.as_bytes()[..end]);
    out
}

/// Writes triangles as a binary STL file. Facet normals are computed from the vertices.
pub fn write_binary(name: &str, triangles: &[[DVec3; 3]]) -> Vec<u8> {
    // The format's count is 32 bits; more triangles than that cannot be represented.
    let count = u32::try_from(triangles.len()).unwrap_or(u32::MAX);
    let triangles = &triangles[..count as usize];
    let mut out = Vec::with_capacity(HEADER_LEN + 4 + triangles.len() * TRIANGLE_LEN);
    out.extend_from_slice(&header(name));
    out.extend_from_slice(&count.to_le_bytes());
    for t in triangles {
        for v in [facet_normal(t), t[0], t[1], t[2]] {
            for c in [v.x, v.y, v.z] {
                out.extend_from_slice(&(c as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

/// Writes triangles as an ASCII STL file. Facet normals are computed from the vertices.
pub fn write_ascii(name: &str, triangles: &[[DVec3; 3]]) -> String {
    // The solid's name is a single token-ish line: keep it printable and on one line.
    let name: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = name.trim();
    let mut out = String::with_capacity(64 + triangles.len() * 256);
    let _ = writeln!(out, "solid {name}");
    for t in triangles {
        let n = facet_normal(t);
        let _ = writeln!(
            out,
            "  facet normal {:e} {:e} {:e}",
            n.x as f32, n.y as f32, n.z as f32
        );
        out.push_str("    outer loop\n");
        for v in t {
            let _ = writeln!(
                out,
                "      vertex {:e} {:e} {:e}",
                v.x as f32, v.y as f32, v.z as f32
            );
        }
        out.push_str("    endloop\n  endfacet\n");
    }
    let _ = writeln!(out, "endsolid {name}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tetrahedron() -> Vec<[DVec3; 3]> {
        let (o, x, y, z) = (
            DVec3::ZERO,
            DVec3::X * 10.0,
            DVec3::Y * 20.0,
            DVec3::Z * 30.5,
        );
        vec![[o, y, x], [o, x, z], [o, z, y], [x, y, z]]
    }

    /// A parsed facet: `(normal, corners)`.
    type Facet = ([f32; 3], [[f32; 3]; 3]);

    /// A minimal binary STL reader: `(header, facets)`.
    fn parse_binary(data: &[u8]) -> (&[u8], Vec<Facet>) {
        let (head, rest) = data.split_at(HEADER_LEN);
        let count = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        let body = &rest[4..];
        assert_eq!(body.len(), count * TRIANGLE_LEN, "size matches the count");
        let f = |b: &[u8]| f32::from_le_bytes(b.try_into().unwrap());
        let vec3 = |b: &[u8]| [f(&b[0..4]), f(&b[4..8]), f(&b[8..12])];
        let tris = body
            .as_chunks::<TRIANGLE_LEN>()
            .0
            .iter()
            .map(|rec| {
                assert_eq!(&rec[48..50], &[0, 0], "attribute byte count is 0");
                (
                    vec3(&rec[0..12]),
                    [vec3(&rec[12..24]), vec3(&rec[24..36]), vec3(&rec[36..48])],
                )
            })
            .collect();
        (head, tris)
    }

    #[test]
    fn binary_layout_and_round_trip() {
        let tris = tetrahedron();
        let data = write_binary("bracket", &tris);
        assert_eq!(data.len(), 80 + 4 + 4 * 50);
        let (head, parsed) = parse_binary(&data);
        assert_eq!(&head[..7], b"bracket");
        assert!(head[7..].iter().all(|&b| b == 0));
        assert_eq!(parsed.len(), 4);
        for (t, (normal, corners)) in tris.iter().zip(&parsed) {
            for (v, c) in t.iter().zip(corners) {
                assert_eq!([v.x as f32, v.y as f32, v.z as f32], *c);
            }
            let n = facet_normal(t);
            assert_eq!([n.x as f32, n.y as f32, n.z as f32], *normal);
            assert!((n.length() - 1.0).abs() < 1e-12);
        }
        // Outward normals: the base faces down, the slanted face away from the origin.
        assert_eq!(parsed[0].0, [0.0, 0.0, -1.0]);
        assert!(parsed[3].0.iter().all(|&c| c > 0.0));
    }

    #[test]
    fn header_edge_cases() {
        let long = "é".repeat(100);
        let data = write_binary(&long, &[]);
        assert_eq!(data.len(), 84);
        assert_eq!(&data[80..], &[0, 0, 0, 0]);
        assert!(
            std::str::from_utf8(&data[..80]).is_ok(),
            "cut at a char boundary"
        );
        let data = write_binary("Solid part", &[]);
        assert!(!data[..80].to_ascii_lowercase().starts_with(b"solid"));
        // Degenerate triangles get a zero normal instead of NaN.
        let data = write_binary("x", &[[DVec3::ONE; 3]]);
        let (_, parsed) = parse_binary(&data);
        assert_eq!(parsed[0].0, [0.0; 3]);
    }

    #[test]
    fn ascii_round_trip() {
        let tris = tetrahedron();
        let text = write_ascii("my part\n", &tris);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("solid my part"));
        assert_eq!(text.lines().last(), Some("endsolid my part"));
        let numbers = |line: &str, skip: usize| -> Vec<f32> {
            line.split_whitespace()
                .skip(skip)
                .map(|x| x.parse().unwrap())
                .collect()
        };
        let mut vertices = Vec::new();
        let mut normals = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.starts_with("vertex") {
                vertices.push(numbers(line, 1));
            } else if line.starts_with("facet normal") {
                normals.push(numbers(line, 2));
            }
        }
        assert_eq!((normals.len(), vertices.len()), (4, 12));
        for (k, v) in tris.iter().flatten().enumerate() {
            assert_eq!(vertices[k], vec![v.x as f32, v.y as f32, v.z as f32]);
        }
        assert_eq!(normals[0], vec![0.0, 0.0, -1.0]);
        assert_eq!(text.matches("endfacet").count(), 4);
    }
}
