//! Gauge and material tables: the sheet stock a shop has, with the bend radius and bend
//! model that go with each thickness.
//!
//! A [`MaterialLibrary`] is a list of [`GaugeTable`]s, one per material (or per supplier,
//! or per brake), and each table is a list of [`GaugeEntry`]s: a gauge name ("16 ga",
//! "1.5 mm"), its thickness, the default inner radius and the bend model (K-factor, bend
//! allowance or bend deduction). Picking an entry fills in a body's [`SheetSettings`]
//! ([`GaugeEntry::settings`]).
//!
//! **Sharing.** Tables are meant to be edited and passed around, so besides `serde` (for
//! the app's settings) they read and write a plain CSV file that opens in a spreadsheet:
//!
//! ```text
//! material,gauge,thickness_mm,radius_mm,k_factor,bend_allowance_mm,bend_deduction_mm,notes
//! Mild steel,16 ga,1.519,1.9,0.43,,,MSG 0.0598 in
//! ```
//!
//! One row per entry. Exactly one of the three bend model columns is filled. Rows with
//! the same material make one table, in the order they first appear. The columns are found
//! by their header names, so they can come in any order and unknown columns are ignored;
//! `radius_mm` may be left empty (the radius is then the thickness). An optional `table`
//! column names the table when it isn't just the material (two tables of one material,
//! say from two suppliers); it is written only when needed. Fields with commas, quotes or
//! line breaks are quoted the usual CSV way; a file saved by a spreadsheet with
//! semicolons as separators is read too. [`MaterialLibrary::from_csv`] reports every
//! problem with its line number and never panics.
//!
//! **Built-in tables** ([`MaterialLibrary::builtin`]): mild steel, galvanised steel,
//! stainless steel and aluminium. They are a starting point: every shop should check the
//! radius and K-factor against a test bend on its own brake and tooling.
//!
//! - Thicknesses: mild steel is the Manufacturers' Standard Gauge for sheet steel (based
//!   on 41.82 lb per square foot per inch of thickness), galvanised steel the galvanised
//!   sheet gauge (MSG plus the zinc coating), stainless steel the US Standard gauge
//!   fractions it is usually sold to (11 ga = 1/8 in, 16 ga = 1/16 in), as tabulated for
//!   example in the Wikipedia article "Sheet metal". Aluminium is listed in common metric
//!   thicknesses (0.5 to 6 mm), as it is mostly sold that way outside the US.
//! - Radii: air bending makes an inner radius that is a fraction of the V-die opening,
//!   and the usual die opening is 8 × t. The fractions are from the "20 percent rule"
//!   (S. Benson, *The Fabricator*): about 15.6 % of the die opening for mild steel, 21 %
//!   for 304 stainless and 14 % for 5052-H32 aluminium. That gives `R ≈ 1.25·t` for
//!   steel, `1.7·t` for stainless and `1.1·t` for aluminium, rounded to 0.1 mm.
//! - K-factors: the widely published air-bending K-factor chart (radius between t and
//!   3·t): 0.40 for soft materials (aluminium), 0.43 for medium (mild and galvanised
//!   steel) and 0.45 for hard ones (stainless steel).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::settings::{BendModel, SheetSettings};

/// One stock thickness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GaugeEntry {
    /// The name the shop uses: "16 ga", "1.5 mm".
    pub gauge: String,
    /// Thickness in mm.
    pub thickness: f64,
    /// Default inner bend radius in mm.
    pub radius: f64,
    pub model: BendModel,
    pub notes: String,
}

impl GaugeEntry {
    /// `base` with this entry's thickness, radius and bend model (reliefs are kept).
    pub fn settings(&self, base: SheetSettings) -> SheetSettings {
        SheetSettings {
            thickness: self.thickness,
            radius: self.radius,
            model: self.model,
            ..base
        }
    }

    /// Checks the values, with a message for the user.
    pub fn check(&self) -> Result<(), String> {
        if self.gauge.trim().is_empty() {
            return Err("The gauge needs a name.".to_owned());
        }
        SheetSettings {
            thickness: self.thickness,
            radius: self.radius,
            model: self.model,
            ..SheetSettings::default()
        }
        .check()
    }
}

/// The stock of one material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GaugeTable {
    pub name: String,
    pub material: String,
    pub entries: Vec<GaugeEntry>,
}

impl GaugeTable {
    /// The entry called `gauge` (ignoring case and spaces: "16GA" finds "16 ga").
    pub fn find(&self, gauge: &str) -> Option<&GaugeEntry> {
        let key = simplify(gauge);
        self.entries.iter().find(|e| simplify(&e.gauge) == key)
    }

    /// The entry whose thickness is nearest `thickness`.
    pub fn nearest(&self, thickness: f64) -> Option<&GaugeEntry> {
        self.entries.iter().min_by(|a, b| {
            (a.thickness - thickness)
                .abs()
                .total_cmp(&(b.thickness - thickness).abs())
        })
    }
}

/// Every table the user has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialLibrary {
    pub tables: Vec<GaugeTable>,
}

impl Default for MaterialLibrary {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Lower case without spaces, for matching names typed by hand.
fn simplify(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A table from `(gauge, thickness)` rows, with the radius as `ratio · t` (to 0.1 mm).
fn table(material: &str, rows: &[(&str, f64, &str)], ratio: f64, k: f64) -> GaugeTable {
    GaugeTable {
        name: material.to_owned(),
        material: material.to_owned(),
        entries: rows
            .iter()
            .map(|&(gauge, thickness, notes)| GaugeEntry {
                gauge: gauge.to_owned(),
                thickness,
                radius: ((ratio * thickness * 10.0).round() / 10.0).max(0.1),
                model: BendModel::KFactor(k),
                notes: notes.to_owned(),
            })
            .collect(),
    }
}

impl MaterialLibrary {
    /// The built-in tables (sources in the module docs).
    pub fn builtin() -> Self {
        let mild = [
            ("10 ga", 3.416, "MSG 0.1345 in"),
            ("11 ga", 3.038, "MSG 0.1196 in"),
            ("12 ga", 2.657, "MSG 0.1046 in"),
            ("13 ga", 2.278, "MSG 0.0897 in"),
            ("14 ga", 1.897, "MSG 0.0747 in"),
            ("15 ga", 1.709, "MSG 0.0673 in"),
            ("16 ga", 1.519, "MSG 0.0598 in"),
            ("17 ga", 1.367, "MSG 0.0538 in"),
            ("18 ga", 1.214, "MSG 0.0478 in"),
            ("19 ga", 1.062, "MSG 0.0418 in"),
            ("20 ga", 0.912, "MSG 0.0359 in"),
            ("21 ga", 0.836, "MSG 0.0329 in"),
            ("22 ga", 0.759, "MSG 0.0299 in"),
            ("23 ga", 0.683, "MSG 0.0269 in"),
            ("24 ga", 0.607, "MSG 0.0239 in"),
        ];
        let galvanised = [
            ("10 ga", 3.51, "0.1382 in"),
            ("12 ga", 2.753, "0.1084 in"),
            ("14 ga", 1.994, "0.0785 in"),
            ("16 ga", 1.613, "0.0635 in"),
            ("18 ga", 1.311, "0.0516 in"),
            ("20 ga", 1.006, "0.0396 in"),
            ("22 ga", 0.853, "0.0336 in"),
            ("24 ga", 0.701, "0.0276 in"),
        ];
        let stainless = [
            ("10 ga", 3.571, "9/64 in"),
            ("11 ga", 3.175, "1/8 in"),
            ("12 ga", 2.778, "7/64 in"),
            ("14 ga", 1.984, "5/64 in"),
            ("16 ga", 1.588, "1/16 in"),
            ("18 ga", 1.27, "1/20 in"),
            ("20 ga", 0.953, "3/80 in"),
            ("22 ga", 0.794, "1/32 in"),
            ("24 ga", 0.635, "1/40 in"),
        ];
        let aluminium = [
            ("0.5 mm", 0.5, ""),
            ("0.8 mm", 0.8, ""),
            ("1 mm", 1.0, ""),
            ("1.2 mm", 1.2, ""),
            ("1.5 mm", 1.5, ""),
            ("2 mm", 2.0, ""),
            ("2.5 mm", 2.5, ""),
            ("3 mm", 3.0, ""),
            ("4 mm", 4.0, ""),
            ("5 mm", 5.0, ""),
            ("6 mm", 6.0, ""),
        ];
        Self {
            tables: vec![
                table("Mild steel", &mild, 1.25, 0.43),
                table("Galvanised steel", &galvanised, 1.25, 0.43),
                table("Stainless steel 304", &stainless, 1.7, 0.45),
                table("Aluminium 5052-H32", &aluminium, 1.1, 0.40),
            ],
        }
    }

    /// The table of `material` (ignoring case and spaces); by table name if no table has
    /// that material.
    pub fn table(&self, material: &str) -> Option<&GaugeTable> {
        let key = simplify(material);
        self.tables
            .iter()
            .find(|t| simplify(&t.material) == key)
            .or_else(|| self.tables.iter().find(|t| simplify(&t.name) == key))
    }

    /// The entry for `material` and `gauge` (both ignoring case and spaces).
    pub fn find(&self, material: &str, gauge: &str) -> Option<&GaugeEntry> {
        let key = simplify(material);
        self.tables
            .iter()
            .filter(|t| simplify(&t.material) == key || simplify(&t.name) == key)
            .find_map(|t| t.find(gauge))
    }

    /// The entry of `material` whose thickness is nearest `thickness`.
    pub fn nearest(&self, material: &str, thickness: f64) -> Option<&GaugeEntry> {
        let key = simplify(material);
        self.tables
            .iter()
            .filter(|t| simplify(&t.material) == key || simplify(&t.name) == key)
            .filter_map(|t| t.nearest(thickness))
            .min_by(|a, b| {
                (a.thickness - thickness)
                    .abs()
                    .total_cmp(&(b.thickness - thickness).abs())
            })
    }

    /// The library as CSV (see the module docs).
    pub fn to_csv(&self) -> String {
        let named = self.tables.iter().any(|t| t.name != t.material);
        let mut s = HEADER.join(",");
        if named {
            s.push_str(",table");
        }
        s.push('\n');
        for t in &self.tables {
            for e in &t.entries {
                let (k, ba, bd) = match e.model {
                    BendModel::KFactor(v) => (v.to_string(), String::new(), String::new()),
                    BendModel::Allowance(v) => (String::new(), v.to_string(), String::new()),
                    BendModel::Deduction(v) => (String::new(), String::new(), v.to_string()),
                };
                let mut fields = vec![
                    quote(&t.material),
                    quote(&e.gauge),
                    e.thickness.to_string(),
                    e.radius.to_string(),
                    k,
                    ba,
                    bd,
                    quote(&e.notes),
                ];
                if named {
                    fields.push(quote(&t.name));
                }
                s.push_str(&fields.join(","));
                s.push('\n');
            }
        }
        s
    }

    /// Reads a library from CSV (see the module docs). Every problem is reported, with
    /// its line number.
    pub fn from_csv(text: &str) -> Result<Self, Vec<CsvError>> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let first = text.lines().next().unwrap_or("");
        let sep = if !first.contains(',') && first.contains(';') {
            ';'
        } else {
            ','
        };
        let (records, mut errors) = split_records(text, sep);
        let mut records = records
            .into_iter()
            .filter(|(_, f)| f.iter().any(|x| !x.trim().is_empty()));
        let Some((header_line, header)) = records.next() else {
            return Err(vec![CsvError {
                line: 1,
                message: "The file is empty: it needs a header line and one line per gauge."
                    .to_owned(),
            }]);
        };
        let names: Vec<String> = header.iter().map(|h| h.trim().to_lowercase()).collect();
        let col = |name: &str| names.iter().position(|n| n == name);
        let mut missing = Vec::new();
        for required in ["material", "gauge", "thickness_mm"] {
            if col(required).is_none() {
                missing.push(required);
            }
        }
        if !missing.is_empty() {
            errors.push(CsvError {
                line: header_line,
                message: format!(
                    "The header has no {} column. The first line must name the columns: {}.",
                    missing.join(", "),
                    HEADER.join(",")
                ),
            });
            errors.sort_by_key(|e| e.line);
            return Err(errors);
        }
        let cols = Columns {
            material: col("material"),
            gauge: col("gauge"),
            thickness: col("thickness_mm"),
            radius: col("radius_mm"),
            k: col("k_factor"),
            ba: col("bend_allowance_mm"),
            bd: col("bend_deduction_mm"),
            notes: col("notes"),
            table: col("table"),
        };
        let mut lib = Self { tables: Vec::new() };
        for (line, fields) in records {
            match row(&cols, &fields) {
                Ok((name, material, entry)) => {
                    let found = lib
                        .tables
                        .iter_mut()
                        .find(|t| t.name == name && t.material == material);
                    match found {
                        Some(t) => t.entries.push(entry),
                        None => lib.tables.push(GaugeTable {
                            name,
                            material,
                            entries: vec![entry],
                        }),
                    }
                }
                Err(message) => errors.push(CsvError { line, message }),
            }
        }
        if errors.is_empty() {
            Ok(lib)
        } else {
            errors.sort_by_key(|e| e.line);
            Err(errors)
        }
    }
}

/// The CSV columns, in the order they are written.
pub const HEADER: [&str; 8] = [
    "material",
    "gauge",
    "thickness_mm",
    "radius_mm",
    "k_factor",
    "bend_allowance_mm",
    "bend_deduction_mm",
    "notes",
];

/// A problem in a CSV file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvError {
    /// 1-based line number in the file.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for CsvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for CsvError {}

struct Columns {
    material: Option<usize>,
    gauge: Option<usize>,
    thickness: Option<usize>,
    radius: Option<usize>,
    k: Option<usize>,
    ba: Option<usize>,
    bd: Option<usize>,
    notes: Option<usize>,
    table: Option<usize>,
}

/// One data row: `(table name, material, entry)`, or a message.
fn row(cols: &Columns, fields: &[String]) -> Result<(String, String, GaugeEntry), String> {
    let get = |c: Option<usize>| {
        c.and_then(|i| fields.get(i))
            .map(|s| s.trim())
            .unwrap_or("")
    };
    let number = |c: Option<usize>, what: &str| -> Result<Option<f64>, String> {
        let s = get(c);
        if s.is_empty() {
            return Ok(None);
        }
        match s.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(Some(v)),
            _ => Err(format!("The {what} \"{s}\" is not a number.")),
        }
    };
    let material = get(cols.material);
    if material.is_empty() {
        return Err("The material is empty.".to_owned());
    }
    let gauge = get(cols.gauge);
    if gauge.is_empty() {
        return Err("The gauge is empty.".to_owned());
    }
    let thickness =
        number(cols.thickness, "thickness")?.ok_or("The thickness is empty: give it in mm.")?;
    let radius = number(cols.radius, "radius")?.unwrap_or(thickness);
    let models = [
        number(cols.k, "K-factor")?.map(BendModel::KFactor),
        number(cols.ba, "bend allowance")?.map(BendModel::Allowance),
        number(cols.bd, "bend deduction")?.map(BendModel::Deduction),
    ];
    let mut given = models.iter().flatten();
    let model = match (given.next(), given.next()) {
        (Some(m), None) => *m,
        (None, _) => {
            return Err(
                "No bend model: fill in one of k_factor, bend_allowance_mm or bend_deduction_mm."
                    .to_owned(),
            );
        }
        (Some(_), Some(_)) => {
            return Err(
                "More than one bend model: fill in only one of k_factor, bend_allowance_mm and bend_deduction_mm."
                    .to_owned(),
            );
        }
    };
    let entry = GaugeEntry {
        gauge: gauge.to_owned(),
        thickness,
        radius,
        model,
        notes: get(cols.notes).to_owned(),
    };
    entry.check()?;
    let table = get(cols.table);
    let name = if table.is_empty() { material } else { table };
    Ok((name.to_owned(), material.to_owned(), entry))
}

/// A field quoted if it has to be.
fn quote(s: &str) -> String {
    let needs = s.contains([',', ';', '"', '\n', '\r']) || s.trim() != s;
    if needs {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// Splits CSV text into records of fields, each with the line it starts on. Quoted fields
/// may hold separators, doubled quotes and line breaks.
fn split_records(text: &str, sep: char) -> (Vec<(usize, Vec<String>)>, Vec<CsvError>) {
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut line = 1;
    let mut start = 1;
    let mut quoted = false;
    let mut quote_line = 1;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.trim().is_empty() => {
                field.clear();
                quoted = true;
                quote_line = line;
            }
            c if c == sep => fields.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                fields.push(std::mem::take(&mut field));
                records.push((start, std::mem::take(&mut fields)));
                line += 1;
                start = line;
            }
            _ => field.push(c),
        }
    }
    if quoted {
        errors.push(CsvError {
            line: quote_line,
            message: "A quoted field is never closed: add the closing \".".to_owned(),
        });
    }
    if !field.is_empty() || !fields.is_empty() {
        fields.push(field);
        records.push((start, fields));
    }
    (records, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_round_trips_through_csv() {
        let lib = MaterialLibrary::builtin();
        assert!(lib.tables.len() >= 4);
        for t in &lib.tables {
            for e in &t.entries {
                e.check().unwrap();
            }
        }
        let csv = lib.to_csv();
        assert!(csv.starts_with(&HEADER.join(",")), "{csv}");
        let back = MaterialLibrary::from_csv(&csv).unwrap();
        assert_eq!(back, lib);
        assert_eq!(back.to_csv(), csv);
    }

    #[test]
    fn named_tables_quotes_and_other_models_round_trip() {
        let lib = MaterialLibrary {
            tables: vec![
                GaugeTable {
                    name: "Supplier A, \"brake 2\"".to_owned(),
                    material: "Mild steel".to_owned(),
                    entries: vec![GaugeEntry {
                        gauge: "2 mm".to_owned(),
                        thickness: 2.0,
                        radius: 2.5,
                        model: BendModel::Allowance(4.1),
                        notes: "tested 2026\nsecond line".to_owned(),
                    }],
                },
                GaugeTable {
                    name: "Mild steel".to_owned(),
                    material: "Mild steel".to_owned(),
                    entries: vec![GaugeEntry {
                        gauge: "3 mm".to_owned(),
                        thickness: 3.0,
                        radius: 3.0,
                        model: BendModel::Deduction(5.25),
                        notes: String::new(),
                    }],
                },
            ],
        };
        let csv = lib.to_csv();
        assert!(csv.lines().next().unwrap().ends_with(",table"));
        assert_eq!(MaterialLibrary::from_csv(&csv).unwrap(), lib);
    }

    #[test]
    fn reads_spreadsheet_files() {
        // Columns in another order, an unknown column, no radius, a BOM, Windows line ends,
        // semicolons and blank rows.
        let text = "\u{feff}Gauge;Material;Thickness_mm;K_factor;Supplier\r\n16 ga;Steel;1.5;0.44;ACME\r\n;;;;\r\n";
        let lib = MaterialLibrary::from_csv(text).unwrap();
        let e = lib.find("steel", "16GA").unwrap();
        assert_eq!(e.thickness, 1.5);
        assert_eq!(e.radius, 1.5);
        assert_eq!(e.model, BendModel::KFactor(0.44));
    }

    #[test]
    fn errors_have_line_numbers() {
        let text = "material,gauge,thickness_mm,radius_mm,k_factor,bend_allowance_mm,bend_deduction_mm,notes\n\
            Steel,16 ga,1.5,1.5,0.44,,,\n\
            Steel,18 ga,thin,1.2,0.44,,,\n\
            Steel,20 ga,0.9,0.9,,,,\n\
            Steel,22 ga,0.75,0.75,0.44,1.2,,\n\
            Steel,24 ga,0.6,0.6,1.5,,,\n\
            ,26 ga,0.45,0.45,0.44,,,\n\
            Steel,28 ga,0.38,0.38,0.44,,,\"open\n";
        let errs = MaterialLibrary::from_csv(text).unwrap_err();
        let lines: Vec<usize> = errs.iter().map(|e| e.line).collect();
        assert_eq!(lines, [3, 4, 5, 6, 7, 8], "{errs:#?}");
        assert!(errs[0].to_string().starts_with("Line 3: "), "{}", errs[0]);
        assert!(errs[0].message.contains("\"thin\""));
        assert!(errs[1].message.contains("No bend model"));
        assert!(errs[2].message.contains("More than one"));
        assert!(errs[3].message.contains("K-factor"));
        assert!(errs[4].message.contains("material"));
        assert!(errs[5].message.contains("never closed"));
        // A missing column.
        let errs = MaterialLibrary::from_csv("material,thickness_mm\nSteel,1\n").unwrap_err();
        assert_eq!(errs[0].line, 1);
        assert!(errs[0].message.contains("gauge"));
        // Nothing at all.
        assert_eq!(MaterialLibrary::from_csv("").unwrap_err()[0].line, 1);
        // Garbage never panics.
        for s in ["\"", ",,,\n\"\"\"", "a\nb\n\"c", "\u{feff}", ";;\n;;"] {
            let _ = MaterialLibrary::from_csv(s);
        }
    }

    #[test]
    fn lookup() {
        let lib = MaterialLibrary::builtin();
        let e = lib.find("mild steel", "16 ga").unwrap();
        assert_eq!(e.thickness, 1.519);
        assert_eq!(e.radius, 1.9);
        assert_eq!(e.model, BendModel::KFactor(0.43));
        assert!(lib.find("mild steel", "99 ga").is_none());
        assert!(lib.find("unobtainium", "16 ga").is_none());
        assert_eq!(lib.nearest("Mild Steel", 1.5).unwrap().gauge, "16 ga");
        assert_eq!(
            lib.nearest("Aluminium 5052-H32", 1.9).unwrap().gauge,
            "2 mm"
        );
        assert_eq!(lib.table("stainless steel 304").unwrap().entries.len(), 9);
        let base = SheetSettings {
            relief_ratio: 0.8,
            ..SheetSettings::default()
        };
        let s = lib
            .find("stainless steel 304", "11 ga")
            .unwrap()
            .settings(base);
        assert_eq!(s.thickness, 3.175);
        assert_eq!(s.radius, 5.4);
        assert_eq!(s.model, BendModel::KFactor(0.45));
        assert_eq!(s.relief_ratio, 0.8);
        assert!(s.check().is_ok());
    }
}
