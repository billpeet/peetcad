//! The binary container of the native `.peet` file format.
//!
//! A `.peet` file is a small table of typed sections. This module only knows the container:
//! what a section means (document metadata, the parametric model, the caches) is decided by
//! the code that fills it. Every section is serialized with `postcard` and compressed with
//! LZ4 on its own, so a reader can show the mesh cache before it touches the model.
//!
//! # Byte layout
//!
//! All integers are little-endian. There is no padding and no alignment.
//!
//! ```text
//! [header: 16 bytes][section table: section_count * entry_len bytes][section data ...]
//! ```
//!
//! Header:
//!
//! | Offset | Size | Field            | Meaning                                                    |
//! |-------:|-----:|------------------|------------------------------------------------------------|
//! |      0 |    4 | `magic`          | The bytes `50 45 45 54` (`"PEET"`)                         |
//! |      4 |    2 | `format_version` | `u16`, the container version, [`FORMAT_VERSION`]           |
//! |      6 |    2 | `header_len`     | `u16`, offset of the section table; 16 in version 1        |
//! |      8 |    2 | `entry_len`      | `u16`, size of one table entry; 28 in version 1            |
//! |     10 |    2 | `section_count`  | `u16`, number of table entries                             |
//! |     12 |    4 | `table_checksum` | `u32`, CRC-32 of bytes `0..12` followed by the bytes from  |
//! |        |      |                  | offset 16 to the end of the section table                  |
//!
//! Section table entry, `section_count` of them starting at `header_len`:
//!
//! | Offset | Size | Field              | Meaning                                                  |
//! |-------:|-----:|--------------------|----------------------------------------------------------|
//! |      0 |    4 | `kind`             | `u32`, what the section holds, see [`SectionKind`]       |
//! |      4 |    2 | `schema_version`   | `u16`, version of the section's own contents             |
//! |      6 |    2 | `flags`            | `u16`, bit 0 set: the data is one LZ4 block; all other   |
//! |        |      |                    | bits are 0                                               |
//! |      8 |    8 | `offset`           | `u64`, position of the data from the start of the file   |
//! |     16 |    4 | `stored_len`       | `u32`, number of bytes of data in the file               |
//! |     20 |    4 | `uncompressed_len` | `u32`, number of bytes after decompression               |
//! |     24 |    4 | `checksum`         | `u32`, CRC-32 of the uncompressed bytes                  |
//!
//! Section data is the `postcard` encoding of the section's value, either as is (`flags` bit 0
//! clear, `stored_len == uncompressed_len`) or as a single raw LZ4 block without a size prefix
//! or frame (`flags` bit 0 set). The writer compresses a section only when that makes it
//! smaller.
//!
//! The checksum is CRC-32 as used by zlib and PNG: reflected polynomial `0xEDB88320`, initial
//! value and final XOR `0xFFFFFFFF`. The CRC-32 of the ASCII text `123456789` is `0xCBF43926`.
//!
//! # Rules
//!
//! - A section kind appears at most once. Sections lie after the table, inside the file, and
//!   don't overlap. They can be in any order, and bytes that no section covers are ignored.
//! - A section is at most [`MAX_SECTION_LEN`] bytes uncompressed. A compressed section can't
//!   claim more than 255 times its stored size, which is the most LZ4 can expand.
//! - Compatible additions keep `format_version`: new section kinds (readers skip kinds they
//!   don't know), and new header or entry fields appended behind the existing ones, announced
//!   by a larger `header_len` or `entry_len` (readers ignore the extra bytes). A section with
//!   an unknown `flags` bit can't be decoded by this version, but the rest of the file can.
//! - Incompatible changes raise `format_version`. A reader refuses a file with a higher
//!   version than it knows.
//!
//! # Reading untrusted files
//!
//! [`Reader::new`] reads only the header and the table and checks every offset and length
//! against the file size. A section is decompressed and its checksum verified when it is asked
//! for, so damage in one section doesn't stop the others from being read. Damaged, truncated
//! or hostile input gives a [`PeetError`]; it never panics and never allocates more than the
//! limits above allow.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The first four bytes of every `.peet` file.
pub const MAGIC: [u8; 4] = *b"PEET";

/// The container version this code writes, and the newest it reads.
pub const FORMAT_VERSION: u16 = 1;

/// Size of the file header in bytes.
pub const HEADER_LEN: usize = 16;

/// Size of one section table entry in bytes.
pub const ENTRY_LEN: usize = 28;

/// The largest section, in uncompressed bytes (1 GiB).
pub const MAX_SECTION_LEN: usize = 1 << 30;

/// The most sections a file can hold (the count is a `u16`).
pub const MAX_SECTIONS: usize = u16::MAX as usize;

/// `flags` bit 0: the section data is an LZ4 block.
const FLAG_LZ4: u16 = 1;

/// LZ4 can't expand a block by more than this factor.
const MAX_LZ4_RATIO: usize = 255;

/// What a section holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SectionKind(pub u32);

impl SectionKind {
    /// Units, material, author, thumbnail.
    pub const METADATA: Self = Self(1);
    /// Parameters, sketches and the feature tree: the source of truth.
    pub const MODEL: Self = Self(2);
    /// The last rebuilt geometry. Can be thrown away and regenerated.
    pub const BREP_CACHE: Self = Self(3);
    /// Tessellated display meshes. Can be thrown away and regenerated.
    pub const MESH_CACHE: Self = Self(4);

    /// A name for messages: "metadata", "model", "B-rep cache", "mesh cache" or "unknown".
    pub fn name(self) -> &'static str {
        match self {
            Self::METADATA => "metadata",
            Self::MODEL => "model",
            Self::BREP_CACHE => "B-rep cache",
            Self::MESH_CACHE => "mesh cache",
            _ => "unknown",
        }
    }

    /// The section as it is called in an error message: "model section", "section 77".
    fn label(self) -> String {
        match self.name() {
            "unknown" => format!("section {}", self.0),
            name => format!("{name} section"),
        }
    }
}

/// Why a `.peet` file could not be read or written. The message is meant for the user.
#[derive(Debug, Clone, PartialEq)]
pub struct PeetError {
    pub message: String,
}

impl PeetError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn damaged(what: impl fmt::Display) -> Self {
        Self::new(format!("The file is damaged: {what}"))
    }
}

impl fmt::Display for PeetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PeetError {}

/// CRC-32 (the zlib and PNG variant) lookup table.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// CRC-32 of the parts, as if they were one run of bytes.
fn crc32_parts(parts: &[&[u8]]) -> u32 {
    let mut c = u32::MAX;
    for part in parts {
        for &b in *part {
            c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    !c
}

/// CRC-32 of the bytes, the checksum used throughout the format.
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_parts(&[bytes])
}

fn too_large(kind: SectionKind, len: usize) -> PeetError {
    PeetError::new(format!(
        "The {} is too large to save ({len} bytes; the limit is {MAX_SECTION_LEN})",
        kind.label()
    ))
}

/// Builds a `.peet` file from sections.
#[derive(Debug, Clone, Default)]
pub struct Writer {
    sections: Vec<PendingSection>,
}

#[derive(Debug, Clone)]
struct PendingSection {
    kind: SectionKind,
    schema_version: u16,
    /// The serialized, uncompressed bytes.
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serializes `value` with postcard and adds it as a section (replacing one of the same
    /// kind).
    pub fn section<T: Serialize>(
        &mut self,
        kind: SectionKind,
        schema_version: u16,
        value: &T,
    ) -> Result<&mut Self, PeetError> {
        let bytes = postcard::to_stdvec(value)
            .map_err(|e| PeetError::new(format!("The {} could not be saved: {e}", kind.label())))?;
        if bytes.len() > MAX_SECTION_LEN {
            return Err(too_large(kind, bytes.len()));
        }
        self.put(kind, schema_version, bytes);
        Ok(self)
    }

    /// Adds already-serialized bytes as a section (replacing one of the same kind).
    ///
    /// Bytes beyond [`MAX_SECTION_LEN`] are accepted here and reported by
    /// [`try_finish`](Self::try_finish).
    pub fn raw_section(
        &mut self,
        kind: SectionKind,
        schema_version: u16,
        bytes: &[u8],
    ) -> &mut Self {
        self.put(kind, schema_version, bytes.to_vec());
        self
    }

    /// Removes a section. Returns whether it was there.
    pub fn remove(&mut self, kind: SectionKind) -> bool {
        let before = self.sections.len();
        self.sections.retain(|s| s.kind != kind);
        self.sections.len() != before
    }

    fn put(&mut self, kind: SectionKind, schema_version: u16, bytes: Vec<u8>) {
        let section = PendingSection {
            kind,
            schema_version,
            bytes,
        };
        // A replaced section keeps its place, so saving twice gives the same bytes.
        match self.sections.iter_mut().find(|s| s.kind == kind) {
            Some(existing) => *existing = section,
            None => self.sections.push(section),
        }
    }

    /// The bytes of the file.
    ///
    /// # Panics
    ///
    /// If the file can't be represented: a section added with
    /// [`raw_section`](Self::raw_section) is larger than [`MAX_SECTION_LEN`], or there are
    /// more than [`MAX_SECTIONS`] sections. Use [`try_finish`](Self::try_finish) to get an
    /// error instead.
    pub fn finish(&self) -> Vec<u8> {
        match self.try_finish() {
            Ok(bytes) => bytes,
            Err(e) => panic!("{e}"),
        }
    }

    /// The bytes of the file, or an error if a section is larger than [`MAX_SECTION_LEN`] or
    /// there are more than [`MAX_SECTIONS`] sections.
    pub fn try_finish(&self) -> Result<Vec<u8>, PeetError> {
        let count = u16::try_from(self.sections.len()).map_err(|_| {
            PeetError::new(format!(
                "The file has too many sections to save ({}; the limit is {MAX_SECTIONS})",
                self.sections.len()
            ))
        })?;
        let table_end = HEADER_LEN + self.sections.len() * ENTRY_LEN;

        let mut table = Vec::with_capacity(self.sections.len() * ENTRY_LEN);
        let mut data = Vec::new();
        for section in &self.sections {
            let raw = &section.bytes;
            // The limit also guarantees that the lengths fit the table's `u32` fields.
            let uncompressed_len = u32::try_from(raw.len())
                .ok()
                .filter(|_| raw.len() <= MAX_SECTION_LEN)
                .ok_or_else(|| too_large(section.kind, raw.len()))?;
            let compressed = lz4_flex::block::compress(raw);
            let (flags, stored) = if compressed.len() < raw.len() {
                (FLAG_LZ4, compressed.as_slice())
            } else {
                (0, raw.as_slice())
            };
            // Never longer than `raw`, which fits.
            let stored_len = u32::try_from(stored.len()).unwrap_or(uncompressed_len);
            let offset = (table_end + data.len()) as u64;

            table.extend_from_slice(&section.kind.0.to_le_bytes());
            table.extend_from_slice(&section.schema_version.to_le_bytes());
            table.extend_from_slice(&flags.to_le_bytes());
            table.extend_from_slice(&offset.to_le_bytes());
            table.extend_from_slice(&stored_len.to_le_bytes());
            table.extend_from_slice(&uncompressed_len.to_le_bytes());
            table.extend_from_slice(&crc32(raw).to_le_bytes());
            data.extend_from_slice(stored);
        }

        let mut out = Vec::with_capacity(table_end + data.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        out.extend_from_slice(&(ENTRY_LEN as u16).to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        let checksum = crc32_parts(&[&out, &table]);
        out.extend_from_slice(&checksum.to_le_bytes());
        out.extend_from_slice(&table);
        out.extend_from_slice(&data);
        Ok(out)
    }
}

/// What the section table says about a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionInfo {
    pub kind: SectionKind,
    /// Version of the section's own contents.
    pub schema_version: u16,
    /// Bytes the section takes in the file.
    pub stored_len: u64,
    /// Bytes of the section once decompressed, as the file declares it.
    pub uncompressed_len: u64,
    /// Whether the section is stored compressed.
    pub compressed: bool,
}

/// One validated table entry: `start..end` lies inside the file.
#[derive(Debug, Clone, Copy)]
struct Entry {
    kind: SectionKind,
    schema_version: u16,
    flags: u16,
    start: usize,
    end: usize,
    uncompressed_len: u32,
    checksum: u32,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let b = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(b.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(b.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    let b = bytes.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(b.try_into().ok()?))
}

/// Reads sections from the bytes of a `.peet` file, one at a time and only when asked.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    format_version: u16,
    entries: Vec<Entry>,
}

impl<'a> Reader<'a> {
    /// Reads the header and the section table. No section is decompressed yet.
    pub fn new(bytes: &'a [u8]) -> Result<Self, PeetError> {
        if bytes.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err(PeetError::new("This file is not a PeetCAD file"));
        }
        let short = || PeetError::damaged("it ends before the header is complete");
        // The version comes first: a newer file may have a header we can't make sense of.
        let format_version = u16_at(bytes, 4).ok_or_else(short)?;
        if format_version > FORMAT_VERSION {
            return Err(PeetError::new(format!(
                "This file was saved by a newer version of PeetCAD (format {format_version}); \
                 this version reads up to {FORMAT_VERSION}"
            )));
        }
        if format_version == 0 {
            return Err(PeetError::damaged("the format version is 0"));
        }
        let header_len = usize::from(u16_at(bytes, 6).ok_or_else(short)?);
        let entry_len = usize::from(u16_at(bytes, 8).ok_or_else(short)?);
        let count = usize::from(u16_at(bytes, 10).ok_or_else(short)?);
        let table_checksum = u32_at(bytes, 12).ok_or_else(short)?;
        if header_len < HEADER_LEN || entry_len < ENTRY_LEN {
            return Err(PeetError::damaged("the header is not valid"));
        }

        let table_end = count
            .checked_mul(entry_len)
            .and_then(|len| len.checked_add(header_len))
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| PeetError::damaged("it ends before the list of sections is complete"))?;
        if crc32_parts(&[&bytes[..12], &bytes[HEADER_LEN..table_end]]) != table_checksum {
            return Err(PeetError::damaged(
                "the checksum of the list of sections doesn't match",
            ));
        }

        let invalid = || PeetError::damaged("the list of sections is not valid");
        let mut entries: Vec<Entry> = Vec::with_capacity(count);
        for i in 0..count {
            // In range: `table_end` was checked above.
            let at = header_len + i * entry_len;
            let kind = SectionKind(u32_at(bytes, at).ok_or_else(invalid)?);
            let schema_version = u16_at(bytes, at + 4).ok_or_else(invalid)?;
            let flags = u16_at(bytes, at + 6).ok_or_else(invalid)?;
            let offset = u64_at(bytes, at + 8).ok_or_else(invalid)?;
            let stored_len = u32_at(bytes, at + 16).ok_or_else(invalid)?;
            let uncompressed_len = u32_at(bytes, at + 20).ok_or_else(invalid)?;
            let checksum = u32_at(bytes, at + 24).ok_or_else(invalid)?;

            let outside = || {
                PeetError::damaged(format!(
                    "the {} lies outside the file (the file may be incomplete)",
                    kind.label()
                ))
            };
            let start = usize::try_from(offset).map_err(|_| outside())?;
            let end = usize::try_from(stored_len)
                .ok()
                .and_then(|len| start.checked_add(len))
                .filter(|&end| end <= bytes.len())
                .ok_or_else(outside)?;
            if start < table_end {
                return Err(PeetError::damaged(format!(
                    "the {} overlaps the list of sections",
                    kind.label()
                )));
            }
            entries.push(Entry {
                kind,
                schema_version,
                flags,
                start,
                end,
                uncompressed_len,
                checksum,
            });
        }

        // Sorting finds both duplicates and overlaps in O(n log n), so a table with 65 535
        // entries costs nothing to check.
        let mut order: Vec<&Entry> = entries.iter().collect();
        order.sort_unstable_by_key(|e| e.kind.0);
        if let Some(pair) = order.windows(2).find(|pair| pair[0].kind == pair[1].kind) {
            return Err(PeetError::damaged(format!(
                "the {} appears more than once",
                pair[0].kind.label()
            )));
        }
        order.sort_unstable_by_key(|e| (e.start, e.end));
        if let Some(pair) = order.windows(2).find(|pair| pair[0].end > pair[1].start) {
            return Err(PeetError::damaged(format!(
                "the {} and the {} overlap",
                pair[0].kind.label(),
                pair[1].kind.label()
            )));
        }

        Ok(Self {
            bytes,
            format_version,
            entries,
        })
    }

    /// The container version the file was written with.
    pub fn format_version(&self) -> u16 {
        self.format_version
    }

    /// Every section in the file, in table order, including kinds this version doesn't know.
    pub fn sections(&self) -> impl Iterator<Item = SectionInfo> + '_ {
        self.entries.iter().map(Entry::info)
    }

    /// What the table says about a section, if the file has it.
    pub fn info(&self, kind: SectionKind) -> Option<SectionInfo> {
        self.entry(kind).map(Entry::info)
    }

    pub fn has(&self, kind: SectionKind) -> bool {
        self.entry(kind).is_some()
    }

    /// The version of a section's contents, if the file has the section.
    pub fn schema_version(&self, kind: SectionKind) -> Option<u16> {
        self.entry(kind).map(|e| e.schema_version)
    }

    fn entry(&self, kind: SectionKind) -> Option<&Entry> {
        self.entries.iter().find(|e| e.kind == kind)
    }

    /// Decompressed, checksum-verified bytes of a section. `Ok(None)` if absent.
    pub fn raw(&self, kind: SectionKind) -> Result<Option<Vec<u8>>, PeetError> {
        let Some(entry) = self.entry(kind) else {
            return Ok(None);
        };
        let label = kind.label();
        if entry.flags & !FLAG_LZ4 != 0 {
            return Err(PeetError::new(format!(
                "The {label} is stored in a way this version of PeetCAD doesn't understand; \
                 the file was probably saved by a newer version"
            )));
        }
        // In range: checked in `new`.
        let stored = &self.bytes[entry.start..entry.end];
        let bad_size = || {
            PeetError::damaged(format!(
                "the {label} declares a size of {} bytes, which can't be right",
                entry.uncompressed_len
            ))
        };
        let len = usize::try_from(entry.uncompressed_len)
            .ok()
            .filter(|&len| len <= MAX_SECTION_LEN)
            .ok_or_else(bad_size)?;

        let data = if entry.flags & FLAG_LZ4 == 0 {
            if len != stored.len() {
                return Err(bad_size());
            }
            stored.to_vec()
        } else {
            // Bounds the allocation by the size of the file, whatever the table claims.
            if len > stored.len().saturating_mul(MAX_LZ4_RATIO) {
                return Err(bad_size());
            }
            let mut data = Vec::new();
            data.try_reserve_exact(len).map_err(|_| {
                PeetError::new(format!(
                    "There is not enough memory to read the {label} ({len} bytes)"
                ))
            })?;
            data.resize(len, 0);
            match lz4_flex::block::decompress_into(stored, &mut data) {
                Ok(written) if written == len => data,
                _ => {
                    return Err(PeetError::damaged(format!(
                        "the {label} could not be decompressed"
                    )));
                }
            }
        };

        if crc32(&data) != entry.checksum {
            return Err(PeetError::damaged(format!(
                "the {label}'s checksum doesn't match"
            )));
        }
        Ok(Some(data))
    }

    /// Decompresses and deserializes a section. `Ok(None)` if absent.
    ///
    /// Check [`schema_version`](Self::schema_version) first: the bytes of a different schema
    /// may decode into nonsense rather than fail.
    pub fn read<T: DeserializeOwned>(&self, kind: SectionKind) -> Result<Option<T>, PeetError> {
        let Some(bytes) = self.raw(kind)? else {
            return Ok(None);
        };
        postcard::from_bytes(&bytes).map(Some).map_err(|e| {
            PeetError::new(format!(
                "The {} could not be read; the file is damaged or was saved by an incompatible \
                 version of PeetCAD ({e})",
                kind.label()
            ))
        })
    }
}

impl Entry {
    fn info(&self) -> SectionInfo {
        SectionInfo {
            kind: self.kind,
            schema_version: self.schema_version,
            stored_len: (self.end - self.start) as u64,
            uncompressed_len: u64::from(self.uncompressed_len),
            compressed: self.flags & FLAG_LZ4 != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde::Deserialize;

    use super::*;

    const KINDS: [SectionKind; 6] = [
        SectionKind::METADATA,
        SectionKind::MODEL,
        SectionKind::BREP_CACHE,
        SectionKind::MESH_CACHE,
        SectionKind(77),
        SectionKind(u32::MAX),
    ];

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Meta {
        units: String,
        author: Option<String>,
        thumbnail: Vec<u8>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    enum Feature {
        Sketch { plane: u8, points: Vec<(f64, f64)> },
        Extrude { sketch: u32, depth: f64 },
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Model {
        parameters: Vec<(String, f64)>,
        features: Vec<Feature>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Mesh {
        positions: Vec<[f32; 3]>,
        indices: Vec<u32>,
    }

    fn meta() -> Meta {
        Meta {
            units: "mm".into(),
            author: Some("Bill".into()),
            thumbnail: vec![0x89, b'P', b'N', b'G'],
        }
    }

    fn model() -> Model {
        Model {
            parameters: vec![("width".into(), 120.0), ("thickness".into(), 1.5)],
            features: vec![
                Feature::Sketch {
                    plane: 0,
                    points: (0..40).map(|i| (f64::from(i), 2.0)).collect(),
                },
                Feature::Extrude {
                    sketch: 0,
                    depth: 12.5,
                },
            ],
        }
    }

    /// Repetitive, so it compresses.
    fn mesh() -> Mesh {
        Mesh {
            positions: (0..600).map(|i| [(i % 7) as f32, 1.0, 0.0]).collect(),
            indices: (0..1800).map(|i| i % 600).collect(),
        }
    }

    /// Metadata, model and mesh cache, in that order.
    fn sample() -> Vec<u8> {
        let mut w = Writer::new();
        w.section(SectionKind::METADATA, 1, &meta()).unwrap();
        w.section(SectionKind::MODEL, 3, &model()).unwrap();
        w.section(SectionKind::MESH_CACHE, 2, &mesh()).unwrap();
        w.finish()
    }

    /// Recomputes the table checksum, so a test can damage the table and still get past it.
    /// Leaves files alone whose header is too broken to locate the table.
    fn fix_table_checksum(bytes: &mut [u8]) {
        let (Some(header_len), Some(entry_len), Some(count)) =
            (u16_at(bytes, 6), u16_at(bytes, 8), u16_at(bytes, 10))
        else {
            return;
        };
        let table_end = usize::from(header_len) + usize::from(entry_len) * usize::from(count);
        if table_end > bytes.len() || table_end < HEADER_LEN {
            return;
        }
        let checksum = crc32_parts(&[&bytes[..12], &bytes[HEADER_LEN..table_end]]);
        bytes[12..16].copy_from_slice(&checksum.to_le_bytes());
    }

    /// Offset of a field of table entry `index` in a file written by `Writer`.
    fn entry_field(index: usize, field: usize) -> usize {
        HEADER_LEN + index * ENTRY_LEN + field
    }

    fn set_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn set_u64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn open_error(bytes: &[u8]) -> String {
        Reader::new(bytes).unwrap_err().message
    }

    /// Opens and reads everything, in every way. Returns nothing: the point is that it returns.
    fn exercise(bytes: &[u8]) {
        let Ok(reader) = Reader::new(bytes) else {
            return;
        };
        let _ = reader.format_version();
        let listed: Vec<SectionInfo> = reader.sections().collect();
        for info in &listed {
            assert!(reader.has(info.kind));
            assert_eq!(reader.schema_version(info.kind), Some(info.schema_version));
            if let Ok(Some(data)) = reader.raw(info.kind) {
                assert_eq!(data.len() as u64, info.uncompressed_len);
                assert!(data.len() <= MAX_SECTION_LEN);
            }
        }
        for kind in KINDS {
            let _ = reader.raw(kind);
            let _ = reader.read::<Meta>(kind);
            let _ = reader.read::<Model>(kind);
            let _ = reader.read::<Mesh>(kind);
        }
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32_parts(&[b"1234", b"", b"56789"]), 0xCBF4_3926);
    }

    #[test]
    fn section_names() {
        assert_eq!(SectionKind::METADATA.name(), "metadata");
        assert_eq!(SectionKind::MODEL.name(), "model");
        assert_eq!(SectionKind::BREP_CACHE.name(), "B-rep cache");
        assert_eq!(SectionKind::MESH_CACHE.name(), "mesh cache");
        assert_eq!(SectionKind(0).name(), "unknown");
        assert_eq!(SectionKind(77).label(), "section 77");
        assert_eq!(SectionKind::MODEL.label(), "model section");
    }

    #[test]
    fn round_trip_of_several_sections() {
        let bytes = sample();
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.format_version(), FORMAT_VERSION);
        assert_eq!(
            r.sections().map(|s| s.kind).collect::<Vec<_>>(),
            [
                SectionKind::METADATA,
                SectionKind::MODEL,
                SectionKind::MESH_CACHE
            ]
        );
        assert_eq!(r.read::<Meta>(SectionKind::METADATA).unwrap(), Some(meta()));
        assert_eq!(r.read::<Model>(SectionKind::MODEL).unwrap(), Some(model()));
        assert_eq!(
            r.read::<Mesh>(SectionKind::MESH_CACHE).unwrap(),
            Some(mesh())
        );
        assert_eq!(r.schema_version(SectionKind::METADATA), Some(1));
        assert_eq!(r.schema_version(SectionKind::MODEL), Some(3));
        assert_eq!(r.schema_version(SectionKind::MESH_CACHE), Some(2));
        assert_eq!(
            r.raw(SectionKind::MODEL).unwrap().unwrap(),
            postcard::to_stdvec(&model()).unwrap()
        );
    }

    #[test]
    fn absent_section_is_none() {
        let bytes = sample();
        let r = Reader::new(&bytes).unwrap();
        assert!(!r.has(SectionKind::BREP_CACHE));
        assert_eq!(r.schema_version(SectionKind::BREP_CACHE), None);
        assert_eq!(r.info(SectionKind::BREP_CACHE), None);
        assert_eq!(r.raw(SectionKind::BREP_CACHE), Ok(None));
        assert_eq!(r.read::<Mesh>(SectionKind::BREP_CACHE), Ok(None));
    }

    #[test]
    fn empty_file_has_only_a_header() {
        let bytes = Writer::new().finish();
        assert_eq!(bytes.len(), HEADER_LEN);
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.sections().count(), 0);
        assert_eq!(r.raw(SectionKind::MODEL), Ok(None));
    }

    #[test]
    fn empty_sections_round_trip() {
        let mut w = Writer::new();
        w.raw_section(SectionKind::METADATA, 1, &[])
            .raw_section(SectionKind::MODEL, 1, b"x")
            .raw_section(SectionKind::MESH_CACHE, 1, &[]);
        let bytes = w.finish();
        assert_eq!(bytes.len(), HEADER_LEN + 3 * ENTRY_LEN + 1);
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.raw(SectionKind::METADATA), Ok(Some(vec![])));
        assert_eq!(r.raw(SectionKind::MODEL), Ok(Some(b"x".to_vec())));
        assert_eq!(r.raw(SectionKind::MESH_CACHE), Ok(Some(vec![])));
    }

    #[test]
    fn replacing_a_section_keeps_one_copy_in_place() {
        let mut w = Writer::new();
        w.raw_section(SectionKind::METADATA, 1, b"first")
            .raw_section(SectionKind::MODEL, 1, b"model")
            .raw_section(SectionKind::METADATA, 2, b"second");
        let bytes = w.finish();
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(
            r.sections().map(|s| s.kind).collect::<Vec<_>>(),
            [SectionKind::METADATA, SectionKind::MODEL]
        );
        assert_eq!(r.schema_version(SectionKind::METADATA), Some(2));
        assert_eq!(r.raw(SectionKind::METADATA), Ok(Some(b"second".to_vec())));

        assert!(w.remove(SectionKind::METADATA));
        assert!(!w.remove(SectionKind::METADATA));
        let bytes = w.finish();
        assert!(!Reader::new(&bytes).unwrap().has(SectionKind::METADATA));
    }

    #[test]
    fn unknown_kinds_are_skipped() {
        let mut w = Writer::new();
        w.raw_section(SectionKind(9001), 7, b"from the future")
            .section(SectionKind::MODEL, 1, &model())
            .unwrap();
        let bytes = w.finish();
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.read::<Model>(SectionKind::MODEL).unwrap(), Some(model()));
        // Still listed and readable as bytes, so a tool can carry it along.
        assert!(r.has(SectionKind(9001)));
        assert_eq!(r.sections().count(), 2);
        assert_eq!(
            r.raw(SectionKind(9001)),
            Ok(Some(b"from the future".to_vec()))
        );
    }

    #[test]
    fn compresses_only_when_it_helps() {
        let zeros = vec![0u8; 1 << 20];
        let noise: Vec<u8> = (0u32..300)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let mut w = Writer::new();
        w.raw_section(SectionKind::MESH_CACHE, 1, &zeros)
            .raw_section(SectionKind::METADATA, 1, &noise);
        let bytes = w.finish();
        let r = Reader::new(&bytes).unwrap();

        let big = r.info(SectionKind::MESH_CACHE).unwrap();
        assert!(big.compressed);
        assert_eq!(big.uncompressed_len, 1 << 20);
        // Even the best case stays inside the ratio the reader accepts.
        assert!(big.stored_len * MAX_LZ4_RATIO as u64 >= big.uncompressed_len);
        assert!(big.stored_len < 5000);
        assert_eq!(r.raw(SectionKind::MESH_CACHE), Ok(Some(zeros)));

        let small = r.info(SectionKind::METADATA).unwrap();
        assert!(!small.compressed);
        assert_eq!(small.stored_len, 300);
        assert_eq!(r.raw(SectionKind::METADATA), Ok(Some(noise)));
    }

    #[test]
    fn reading_is_lazy() {
        let mut bytes = sample();
        let (model_at, mesh_at) = {
            let r = Reader::new(&bytes).unwrap();
            let offset = |kind| {
                let index = r.sections().position(|s| s.kind == kind).unwrap();
                u64_at(&bytes, entry_field(index, 8)).unwrap() as usize
            };
            (offset(SectionKind::MODEL), offset(SectionKind::MESH_CACHE))
        };
        // Damage the payloads of the model and of the mesh cache.
        bytes[model_at + 3] ^= 0x55;
        bytes[mesh_at + 3] ^= 0x55;

        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.read::<Meta>(SectionKind::METADATA).unwrap(), Some(meta()));
        let e = r.read::<Model>(SectionKind::MODEL).unwrap_err();
        assert!(
            e.message
                .starts_with("The file is damaged: the model section")
        );
        let e = r.raw(SectionKind::MESH_CACHE).unwrap_err();
        assert!(
            e.message
                .starts_with("The file is damaged: the mesh cache section")
        );
    }

    #[test]
    fn wrong_magic() {
        for bytes in [
            &b""[..],
            b"PEE",
            b"PEEX\x01\x00\x10\x00\x1c\x00\x00\x00\x00\x00\x00\x00",
            b"solid cube",
            b"\x89PNG\r\n\x1a\n",
        ] {
            assert_eq!(open_error(bytes), "This file is not a PeetCAD file");
        }
        let mut bytes = sample();
        bytes[0] = b'p';
        assert_eq!(open_error(&bytes), "This file is not a PeetCAD file");
    }

    #[test]
    fn truncation_at_every_length_is_an_error() {
        let full = sample();
        assert!(Reader::new(&full).is_ok());
        for len in 0..full.len() {
            let cut = &full[..len];
            // The last section is not empty, so no shorter file is complete.
            assert!(Reader::new(cut).is_err(), "prefix of {len} bytes opened");
        }
        assert_eq!(
            open_error(&full[..10]),
            "The file is damaged: it ends before the header is complete"
        );
        assert_eq!(
            open_error(&full[..40]),
            "The file is damaged: it ends before the list of sections is complete"
        );
        assert_eq!(
            open_error(&full[..full.len() - 1]),
            "The file is damaged: the mesh cache section lies outside the file (the file may be \
             incomplete)"
        );
    }

    #[test]
    fn truncation_with_a_trailing_empty_section_never_panics() {
        let mut w = Writer::new();
        w.raw_section(SectionKind::MODEL, 1, b"model")
            .raw_section(SectionKind::MESH_CACHE, 1, &[]);
        let full = w.finish();
        for len in 0..=full.len() {
            exercise(&full[..len]);
        }
    }

    #[test]
    fn newer_format_version() {
        let mut bytes = sample();
        bytes[4..6].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            open_error(&bytes),
            "This file was saved by a newer version of PeetCAD (format 3); this version reads up \
             to 1"
        );
        // Reported even if nothing else of the header is there.
        assert!(open_error(&bytes[..6]).contains("newer version"));

        bytes[4..6].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            open_error(&bytes),
            "The file is damaged: the format version is 0"
        );
    }

    #[test]
    fn damaged_table_is_caught_by_its_checksum() {
        let full = sample();
        for at in 6..HEADER_LEN + 3 * ENTRY_LEN {
            let mut bytes = full.clone();
            bytes[at] ^= 0x01;
            assert!(Reader::new(&bytes).is_err(), "flip at {at} went unnoticed");
        }
        let mut bytes = full;
        bytes[entry_field(1, 4)] ^= 0x01;
        assert_eq!(
            open_error(&bytes),
            "The file is damaged: the checksum of the list of sections doesn't match"
        );
    }

    #[test]
    fn invalid_header_sizes() {
        for (at, value) in [(6, 15u16), (8, 27), (6, 0), (8, 0)] {
            let mut bytes = sample();
            bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
            fix_table_checksum(&mut bytes);
            assert_eq!(
                open_error(&bytes),
                "The file is damaged: the header is not valid"
            );
        }
        let mut bytes = sample();
        bytes[10..12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(
            open_error(&bytes),
            "The file is damaged: it ends before the list of sections is complete"
        );
    }

    #[test]
    fn bad_checksum() {
        let mut bytes = sample();
        let at = entry_field(1, 24);
        bytes[at] ^= 0xFF;
        fix_table_checksum(&mut bytes);
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(
            r.raw(SectionKind::MODEL).unwrap_err().message,
            "The file is damaged: the model section's checksum doesn't match"
        );
        assert_eq!(
            r.read::<Model>(SectionKind::MODEL).unwrap_err().message,
            "The file is damaged: the model section's checksum doesn't match"
        );
        assert!(r.raw(SectionKind::METADATA).is_ok());

        // A flipped payload byte of a stored (not compressed) section.
        let mut w = Writer::new();
        w.raw_section(SectionKind(77), 1, b"abc");
        let mut bytes = w.finish();
        *bytes.last_mut().unwrap() ^= 1;
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(
            r.raw(SectionKind(77)).unwrap_err().message,
            "The file is damaged: the section 77's checksum doesn't match"
        );
    }

    #[test]
    fn oversize_declared_length() {
        // The mesh cache is entry 2 and is compressed.
        let stored = Reader::new(&sample())
            .unwrap()
            .info(SectionKind::MESH_CACHE)
            .unwrap();
        assert!(stored.compressed);
        let too_much_for_lz4 = stored.stored_len as u32 * MAX_LZ4_RATIO as u32 + 1;
        for declared in [u32::MAX, (MAX_SECTION_LEN + 1) as u32, too_much_for_lz4] {
            let mut bytes = sample();
            set_u32(&mut bytes, entry_field(2, 20), declared);
            fix_table_checksum(&mut bytes);
            let r = Reader::new(&bytes).unwrap();
            assert_eq!(
                r.raw(SectionKind::MESH_CACHE).unwrap_err().message,
                format!(
                    "The file is damaged: the mesh cache section declares a size of {declared} \
                     bytes, which can't be right"
                )
            );
            assert!(r.raw(SectionKind::MODEL).is_ok());
        }

        // A wrong but plausible size fails in the decompressor instead.
        for declared in [
            stored.uncompressed_len as u32 - 1,
            stored.uncompressed_len as u32 + 1,
        ] {
            let mut bytes = sample();
            set_u32(&mut bytes, entry_field(2, 20), declared);
            fix_table_checksum(&mut bytes);
            let r = Reader::new(&bytes).unwrap();
            assert_eq!(
                r.raw(SectionKind::MESH_CACHE).unwrap_err().message,
                "The file is damaged: the mesh cache section could not be decompressed"
            );
        }

        // A stored section must declare exactly its stored size.
        let mut w = Writer::new();
        w.raw_section(SectionKind::MODEL, 1, b"abc");
        let mut bytes = w.finish();
        set_u32(&mut bytes, entry_field(0, 20), 4);
        fix_table_checksum(&mut bytes);
        let r = Reader::new(&bytes).unwrap();
        assert!(r.raw(SectionKind::MODEL).is_err());
    }

    #[test]
    fn sections_outside_the_file() {
        let len = sample().len() as u64;
        for (field, value) in [
            (8, len + 1),
            (8, u64::MAX),
            (8, u64::MAX - 10),
            (8, 1 << 40),
        ] {
            let mut bytes = sample();
            set_u64(&mut bytes, entry_field(1, field), value);
            fix_table_checksum(&mut bytes);
            assert_eq!(
                open_error(&bytes),
                "The file is damaged: the model section lies outside the file (the file may be \
                 incomplete)"
            );
        }
        let mut bytes = sample();
        set_u32(&mut bytes, entry_field(1, 16), u32::MAX);
        fix_table_checksum(&mut bytes);
        assert!(open_error(&bytes).contains("the model section lies outside the file"));

        // Pointing into the header or the table.
        for offset in [0, 4, (HEADER_LEN + 3 * ENTRY_LEN - 1) as u64] {
            let mut bytes = sample();
            set_u64(&mut bytes, entry_field(0, 8), offset);
            fix_table_checksum(&mut bytes);
            assert_eq!(
                open_error(&bytes),
                "The file is damaged: the metadata section overlaps the list of sections"
            );
        }
    }

    #[test]
    fn overlapping_sections() {
        let original = sample();
        let model_offset = u64_at(&original, entry_field(1, 8)).unwrap();

        // The mesh cache starts inside the model.
        let mut bytes = original.clone();
        set_u64(&mut bytes, entry_field(2, 8), model_offset + 1);
        fix_table_checksum(&mut bytes);
        assert_eq!(
            open_error(&bytes),
            "The file is damaged: the model section and the mesh cache section overlap"
        );

        // Two sections share the same bytes exactly.
        let mut bytes = original.clone();
        let model_len = u32_at(&original, entry_field(1, 16)).unwrap();
        set_u64(&mut bytes, entry_field(2, 8), model_offset);
        set_u32(&mut bytes, entry_field(2, 16), model_len);
        fix_table_checksum(&mut bytes);
        assert!(open_error(&bytes).ends_with("overlap"));

        // The metadata grows into the model.
        let mut bytes = original;
        let meta_len = u32_at(&bytes, entry_field(0, 16)).unwrap();
        set_u32(&mut bytes, entry_field(0, 16), meta_len + 1);
        fix_table_checksum(&mut bytes);
        assert_eq!(
            open_error(&bytes),
            "The file is damaged: the metadata section and the model section overlap"
        );
    }

    #[test]
    fn duplicate_kinds() {
        let mut bytes = sample();
        set_u32(&mut bytes, entry_field(2, 0), SectionKind::MODEL.0);
        fix_table_checksum(&mut bytes);
        assert_eq!(
            open_error(&bytes),
            "The file is damaged: the model section appears more than once"
        );
    }

    #[test]
    fn unknown_flags_fail_only_that_section() {
        let mut bytes = sample();
        bytes[entry_field(1, 6)] |= 0x02;
        fix_table_checksum(&mut bytes);
        let r = Reader::new(&bytes).unwrap();
        assert!(
            r.raw(SectionKind::MODEL)
                .unwrap_err()
                .message
                .starts_with("The model section is stored in a way this version")
        );
        assert_eq!(r.read::<Meta>(SectionKind::METADATA).unwrap(), Some(meta()));
    }

    #[test]
    fn garbage_in_a_compressed_section() {
        let mut bytes = sample();
        let r = Reader::new(&bytes).unwrap();
        let info = r.info(SectionKind::MESH_CACHE).unwrap();
        let start = bytes.len() - info.stored_len as usize;
        for b in &mut bytes[start..] {
            *b = 0xFF;
        }
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(
            r.raw(SectionKind::MESH_CACHE).unwrap_err().message,
            "The file is damaged: the mesh cache section could not be decompressed"
        );
    }

    #[test]
    fn wrong_type_is_an_error_not_a_panic() {
        let bytes = sample();
        let r = Reader::new(&bytes).unwrap();
        let e = r.read::<Model>(SectionKind::METADATA).unwrap_err();
        assert!(
            e.message
                .starts_with("The metadata section could not be read; the file is damaged or")
        );
    }

    #[test]
    fn larger_header_and_entries_are_read() {
        // What a later, compatible version might write: 4 more header bytes and 8 more bytes
        // per entry.
        let payload = b"hello";
        let (header_len, entry_len) = (20usize, 36usize);
        let data_at = header_len + 2 * entry_len;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(header_len as u16).to_le_bytes());
        bytes.extend_from_slice(&(entry_len as u16).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&[0xAA; 4]);
        for (kind, offset) in [
            (SectionKind::MODEL, data_at),
            (SectionKind(50), data_at + 7),
        ] {
            bytes.extend_from_slice(&kind.0.to_le_bytes());
            bytes.extend_from_slice(&9u16.to_le_bytes());
            bytes.extend_from_slice(&0u16.to_le_bytes());
            bytes.extend_from_slice(&(offset as u64).to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&crc32(payload).to_le_bytes());
            bytes.extend_from_slice(&[0xBB; 8]);
        }
        // A gap between the sections and bytes behind them are ignored.
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&[0xCC; 2]);
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&[0xDD; 3]);
        fix_table_checksum(&mut bytes);

        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.schema_version(SectionKind::MODEL), Some(9));
        assert_eq!(r.raw(SectionKind::MODEL), Ok(Some(payload.to_vec())));
        assert_eq!(r.raw(SectionKind(50)), Ok(Some(payload.to_vec())));
    }

    #[test]
    fn a_full_table_of_tiny_sections_is_checked_quickly() {
        let mut w = Writer::new();
        w.sections = (0..MAX_SECTIONS as u32)
            .map(|i| PendingSection {
                kind: SectionKind(i),
                schema_version: 1,
                bytes: vec![i as u8],
            })
            .collect();
        let bytes = w.finish();
        let r = Reader::new(&bytes).unwrap();
        assert_eq!(r.sections().count(), MAX_SECTIONS);
        assert_eq!(r.raw(SectionKind(300)), Ok(Some(vec![44])));

        w.sections.push(PendingSection {
            kind: SectionKind(u32::MAX),
            schema_version: 1,
            bytes: vec![],
        });
        assert!(
            w.try_finish()
                .unwrap_err()
                .message
                .contains("too many sections")
        );
    }

    #[test]
    fn error_is_displayed_as_its_message() {
        let e = Reader::new(b"nope").unwrap_err();
        assert_eq!(e.to_string(), e.message);
        let _: &dyn std::error::Error = &e;
    }

    /// The exact bytes of a small file. If this fails, the format changed: files that users
    /// have saved must stay readable, so bump [`FORMAT_VERSION`] or keep reading the old
    /// layout, and never just update the expected bytes.
    #[test]
    fn golden_bytes() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Tiny {
            name: String,
            size: u32,
            on: bool,
        }
        let tiny = Tiny {
            name: "ab".into(),
            size: 300,
            on: true,
        };
        // Small sections are stored as they are, so these bytes don't depend on the
        // compressor.
        let mut w = Writer::new();
        w.raw_section(SectionKind::METADATA, 1, b"PeetCAD");
        w.section(SectionKind::MODEL, 2, &tiny).unwrap();
        let bytes = w.finish();

        #[rustfmt::skip]
        let expected: [u8; 85] = [
            // Header: magic, version 1, header 16, entry 28, 2 sections, table checksum.
            b'P', b'E', b'E', b'T', 1, 0, 16, 0, 28, 0, 2, 0, 0xF1, 0xCC, 0x7B, 0x55,
            // Metadata: kind 1, schema 1, stored, at 72, 7 bytes, 7 bytes, checksum.
            1, 0, 0, 0, 1, 0, 0, 0, 72, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 7, 0, 0, 0,
            0x82, 0xAF, 0x00, 0xB9,
            // Model: kind 2, schema 2, stored, at 79, 6 bytes, 6 bytes, checksum.
            2, 0, 0, 0, 2, 0, 0, 0, 79, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 6, 0, 0, 0,
            0xCD, 0x97, 0x24, 0x5C,
            // Metadata data.
            b'P', b'e', b'e', b't', b'C', b'A', b'D',
            // Model data (postcard): string length 2, "ab", varint 300, true.
            2, b'a', b'b', 0xAC, 0x02, 1,
        ];
        assert_eq!(bytes, expected, "the file format changed: {bytes:02X?}");

        let r = Reader::new(&expected).unwrap();
        assert_eq!(r.raw(SectionKind::METADATA), Ok(Some(b"PeetCAD".to_vec())));
        assert_eq!(r.read::<Tiny>(SectionKind::MODEL), Ok(Some(tiny)));
    }

    /// A fixed file with a compressed section, built by hand with an independent CRC-32
    /// (zlib). Only read, because a different version of the compressor may produce other
    /// (equally valid) bytes.
    #[test]
    fn golden_compressed_file_is_read() {
        #[rustfmt::skip]
        let file: [u8; 55] = [
            // Header: magic, version 1, header 16, entry 28, 1 section, table checksum.
            b'P', b'E', b'E', b'T', 1, 0, 16, 0, 28, 0, 1, 0, 0xC0, 0x39, 0x65, 0x7E,
            // Mesh cache: kind 4, schema 5, LZ4, at 44, 11 bytes stored, 64 bytes, checksum.
            4, 0, 0, 0, 5, 0, 1, 0, 44, 0, 0, 0, 0, 0, 0, 0, 11, 0, 0, 0, 64, 0, 0, 0,
            0xB8, 0x59, 0x3C, 0xD5,
            // LZ4 block: 1 literal (7), a match of 58 bytes at distance 1, 5 literals.
            0x1F, 7, 1, 0, 39, 0x50, 7, 7, 7, 7, 7,
        ];
        let r = Reader::new(&file).unwrap();
        let info = r.info(SectionKind::MESH_CACHE).unwrap();
        assert!(info.compressed);
        assert_eq!(info.schema_version, 5);
        assert_eq!(r.raw(SectionKind::MESH_CACHE), Ok(Some(vec![7u8; 64])));
    }

    /// Valid files of up to four sections with arbitrary contents.
    fn arb_file() -> impl Strategy<Value = Vec<u8>> {
        let section = (
            prop::sample::select(KINDS.to_vec()),
            any::<u16>(),
            prop_oneof![
                prop::collection::vec(any::<u8>(), 0..200),
                // Compressible.
                (any::<u8>(), 0usize..2000).prop_map(|(b, n)| vec![b; n]),
            ],
        );
        prop::collection::vec(section, 0..5).prop_map(|sections| {
            let mut w = Writer::new();
            for (kind, schema, bytes) in sections {
                w.raw_section(kind, schema, &bytes);
            }
            w.finish()
        })
    }

    #[derive(Debug, Clone)]
    enum Mutation {
        Flip(prop::sample::Index, u8),
        Truncate(prop::sample::Index),
        Insert(prop::sample::Index, Vec<u8>),
        Remove(prop::sample::Index, usize),
    }

    fn arb_mutation() -> impl Strategy<Value = Mutation> {
        prop_oneof![
            4 => (any::<prop::sample::Index>(), 1u8..=255).prop_map(|(i, x)| Mutation::Flip(i, x)),
            1 => any::<prop::sample::Index>().prop_map(Mutation::Truncate),
            1 => (any::<prop::sample::Index>(), prop::collection::vec(any::<u8>(), 1..8))
                .prop_map(|(i, b)| Mutation::Insert(i, b)),
            1 => (any::<prop::sample::Index>(), 1usize..8).prop_map(|(i, n)| Mutation::Remove(i, n)),
        ]
    }

    fn mutate(bytes: &mut Vec<u8>, mutation: &Mutation) {
        match mutation {
            Mutation::Flip(at, xor) => {
                if !bytes.is_empty() {
                    let at = at.index(bytes.len());
                    bytes[at] ^= xor;
                }
            }
            Mutation::Truncate(at) => bytes.truncate(at.index(bytes.len() + 1)),
            Mutation::Insert(at, extra) => {
                let at = at.index(bytes.len() + 1);
                bytes.splice(at..at, extra.iter().copied());
            }
            Mutation::Remove(at, n) => {
                let at = at.index(bytes.len() + 1);
                let end = (at + n).min(bytes.len());
                bytes.drain(at..end);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 2000,
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        #[test]
        fn fuzz_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
            exercise(&bytes);
        }

        /// Arbitrary bytes behind a valid magic and version, with a valid table checksum, so
        /// the table checks are reached.
        #[test]
        fn fuzz_arbitrary_table(
            count in 0u16..6,
            entry_extra in 0u16..3,
            body in prop::collection::vec(any::<u8>(), 0..300),
            small_fields in any::<bool>(),
        ) {
            let entry_len = ENTRY_LEN as u16 + entry_extra;
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&MAGIC);
            bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
            bytes.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes());
            bytes.extend_from_slice(&entry_len.to_le_bytes());
            bytes.extend_from_slice(&count.to_le_bytes());
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&body);
            if small_fields {
                // Random offsets and lengths are nearly always out of range. Keep their low
                // byte only, so that the entries often pass and the sections get decoded.
                for i in 0..usize::from(count) {
                    let at = HEADER_LEN + i * usize::from(entry_len);
                    for zero in [5..6, 7..8, 9..16, 17..20, 21..24] {
                        if let Some(field) = bytes.get_mut(at + zero.start..at + zero.end) {
                            field.fill(0);
                        }
                    }
                    // Stored or compressed, no unknown flags.
                    if let Some(flags) = bytes.get_mut(at + 6) {
                        *flags &= 1;
                    }
                }
            }
            fix_table_checksum(&mut bytes);
            exercise(&bytes);
        }

        #[test]
        fn fuzz_round_trip(
            sections in prop::collection::vec(
                (any::<u32>(), any::<u16>(), prop::collection::vec(any::<u8>(), 0..400)),
                0..6,
            )
        ) {
            let mut w = Writer::new();
            let mut expected: Vec<(u32, u16, Vec<u8>)> = Vec::new();
            for (kind, schema, bytes) in sections {
                w.raw_section(SectionKind(kind), schema, &bytes);
                match expected.iter_mut().find(|e| e.0 == kind) {
                    Some(e) => *e = (kind, schema, bytes),
                    None => expected.push((kind, schema, bytes)),
                }
            }
            let file = w.finish();
            let r = Reader::new(&file).unwrap();
            prop_assert_eq!(r.sections().count(), expected.len());
            for (kind, schema, bytes) in expected {
                prop_assert_eq!(r.schema_version(SectionKind(kind)), Some(schema));
                prop_assert_eq!(r.raw(SectionKind(kind)), Ok(Some(bytes)));
            }
        }

        #[test]
        fn fuzz_mutated_files(
            file in arb_file(),
            mutations in prop::collection::vec(arb_mutation(), 1..4),
            fix_checksum in any::<bool>(),
        ) {
            let mut bytes = file;
            for mutation in &mutations {
                mutate(&mut bytes, mutation);
            }
            // Without this, nearly every change to the table stops at its checksum.
            if fix_checksum {
                fix_table_checksum(&mut bytes);
            }
            exercise(&bytes);
        }

        #[test]
        fn fuzz_mutated_typed_file(
            mutations in prop::collection::vec(arb_mutation(), 1..4),
            fix_checksum in any::<bool>(),
        ) {
            let mut bytes = sample();
            for mutation in &mutations {
                mutate(&mut bytes, mutation);
            }
            if fix_checksum {
                fix_table_checksum(&mut bytes);
            }
            exercise(&bytes);
        }

        /// Sections whose checksum is right but whose contents are arbitrary: the
        /// deserializer sees hostile bytes.
        #[test]
        fn fuzz_deserialize(payload in prop::collection::vec(any::<u8>(), 0..200)) {
            let mut w = Writer::new();
            for kind in KINDS {
                w.raw_section(kind, 1, &payload);
            }
            exercise(&w.finish());
        }
    }
}
