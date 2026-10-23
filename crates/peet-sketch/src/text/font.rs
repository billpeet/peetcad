//! Finding a font by name: the TrueType and OpenType fonts installed on the machine, and
//! one built in for when the font named is not there.
//!
//! The fonts installed are listed once per process, from each file's name table alone
//! (a few hundred bytes read per file); a font is read whole only when it is used.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The font used where the one named is not installed: Didact Gothic, a geometric sans
/// like SolidWorks' default Century Gothic, under the SIL Open Font License
/// (`fonts/DidactGothic-OFL.txt`).
pub const FALLBACK: &str = "Didact Gothic";
const FALLBACK_DATA: &[u8] = include_bytes!("../../fonts/DidactGothic-Regular.ttf");

/// A font file's bytes and which face in it (a collection holds several).
#[derive(Debug)]
pub struct FontData {
    pub bytes: Vec<u8>,
    pub index: u32,
    /// The face's full name, as its name table gives it.
    pub name: String,
}

impl FontData {
    pub fn face(&self) -> Option<skrifa::FontRef<'_>> {
        skrifa::FontRef::from_index(&self.bytes, self.index).ok()
    }
}

/// One face of an installed font file.
#[derive(Clone, Debug)]
struct Entry {
    path: PathBuf,
    index: u32,
    family: String,
    full: String,
    regular: bool,
}

fn installed() -> &'static [Entry] {
    static LIST: OnceLock<Vec<Entry>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut out = Vec::new();
        for dir in font_dirs() {
            scan(&dir, &mut out, 0);
        }
        out
    })
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        let windir = env("WINDIR")
            .or_else(|| env("SystemRoot"))
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        dirs.push(windir.join("Fonts"));
        if let Some(local) = env("LOCALAPPDATA") {
            dirs.push(local.join(r"Microsoft\Windows\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Library/Fonts".into());
        if let Some(home) = env("HOME") {
            dirs.push(home.join("Library/Fonts"));
        }
    } else {
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Some(home) = env("HOME") {
            dirs.push(home.join(".local/share/fonts"));
            dirs.push(home.join(".fonts"));
        }
    }
    dirs
}

fn scan(dir: &Path, out: &mut Vec<Entry>, depth: u32) {
    let Ok(list) = std::fs::read_dir(dir) else {
        return;
    };
    for item in list.flatten() {
        let path = item.path();
        if path.is_dir() {
            if depth < 4 {
                scan(&path, out, depth + 1);
            }
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
            let _ = names_of(&path, out);
        }
    }
}

/// Reads the name tables of a font file's faces, without reading the whole file.
fn names_of(path: &Path, out: &mut Vec<Entry>) -> std::io::Result<()> {
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head)?;
    let u32_at = |b: &[u8], i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let offsets: Vec<u32> = if &head[0..4] == b"ttcf" {
        let n = u32_at(&head, 8).min(64) as usize;
        let mut buf = vec![0u8; 4 * n];
        f.read_exact(&mut buf)?;
        (0..n).map(|i| u32_at(&buf, 4 * i)).collect()
    } else {
        vec![0]
    };
    for (index, offset) in offsets.into_iter().enumerate() {
        f.seek(SeekFrom::Start(u64::from(offset)))?;
        let mut dir = [0u8; 12];
        f.read_exact(&mut dir)?;
        let tables = u16::from_be_bytes([dir[4], dir[5]]).min(512) as usize;
        let mut records = vec![0u8; 16 * tables];
        f.read_exact(&mut records)?;
        let Some(name) = (0..tables).find(|i| &records[16 * i..16 * i + 4] == b"name") else {
            continue;
        };
        let at = u32_at(&records, 16 * name + 8);
        let len = u32_at(&records, 16 * name + 12).min(1 << 20) as usize;
        let mut data = vec![0u8; len];
        f.seek(SeekFrom::Start(u64::from(at)))?;
        f.read_exact(&mut data)?;
        use skrifa::raw::{FontData as Raw, FontRead, tables::name::Name, types::NameId};
        let Ok(table) = Name::read(Raw::new(&data)) else {
            continue;
        };
        // English (US) first, as Windows lists fonts.
        let get = |id: NameId| {
            let named = || {
                table
                    .name_record()
                    .iter()
                    .filter(move |r| r.name_id() == id && matches!(r.platform_id(), 0 | 3))
            };
            named()
                .filter(|r| r.language_id() == 0x0409)
                .chain(named())
                .find_map(|r| r.string(table.string_data()).ok().map(|s| s.to_string()))
        };
        let family = get(NameId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| get(NameId::FAMILY_NAME));
        let sub = get(NameId::TYPOGRAPHIC_SUBFAMILY_NAME).or_else(|| get(NameId::SUBFAMILY_NAME));
        let full = get(NameId::FULL_NAME);
        let (Some(family), Some(full)) = (family, full) else {
            continue;
        };
        let regular = sub.is_none_or(|s| {
            let s = s.to_ascii_lowercase();
            s == "regular" || s == "normal" || s == "book" || s == "roman"
        });
        out.push(Entry {
            path: path.to_owned(),
            index: index as u32,
            family,
            full,
            regular,
        });
    }
    Ok(())
}

fn same(a: &str, b: &str) -> bool {
    let key = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    key(a) == key(b)
}

/// The installed font with this name: a face's full name ("Century Gothic Bold") or a
/// family's ("Century Gothic", its regular face). `None` if it isn't installed.
pub fn installed_font(name: &str) -> Option<Arc<FontData>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<FontData>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().ok()?.get(name) {
        return found.clone();
    }
    let list = installed();
    let pick = list
        .iter()
        .find(|e| same(&e.full, name))
        .or_else(|| list.iter().find(|e| same(&e.family, name) && e.regular))
        .or_else(|| list.iter().find(|e| same(&e.family, name)));
    let found = pick.and_then(|e| {
        let bytes = std::fs::read(&e.path).ok()?;
        skrifa::FontRef::from_index(&bytes, e.index).ok()?;
        Some(Arc::new(FontData {
            bytes,
            index: e.index,
            name: e.full.clone(),
        }))
    });
    if let Ok(mut c) = cache.lock() {
        c.insert(name.to_owned(), found.clone());
    }
    found
}

/// The built-in font.
pub fn fallback() -> Arc<FontData> {
    static DATA: OnceLock<Arc<FontData>> = OnceLock::new();
    DATA.get_or_init(|| {
        Arc::new(FontData {
            bytes: FALLBACK_DATA.to_vec(),
            index: 0,
            name: FALLBACK.to_owned(),
        })
    })
    .clone()
}

/// The font named if it is installed (or is the built-in one), else the built-in one.
/// The flag says whether it is the one named.
pub fn font(name: &str) -> (Arc<FontData>, bool) {
    if same(name, FALLBACK) {
        return (fallback(), true);
    }
    match installed_font(name) {
        Some(f) => (f, true),
        None => (fallback(), false),
    }
}
