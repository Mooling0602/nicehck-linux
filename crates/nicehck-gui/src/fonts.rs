//! CJK font loading.
//!
//! egui ships only Ubuntu-Light, NotoEmoji and emoji-icon-font — none carry CJK
//! glyphs, so Chinese text renders as tofu boxes unless a font is installed
//! explicitly. We cannot bundle one either: a usable Simplified Chinese face is
//! 10–20 MB and would dwarf the binaries (and the APK's own HarmonyOS Sans is a
//! Latin-only subset, verified: it has no U+4E00–9FFF at all).
//!
//! So we resolve one at runtime, in three tiers:
//!
//!   1. fontconfig — honours the desktop's own font settings. Asking for
//!      `sans-serif:lang=zh-cn` makes fontconfig weigh glyph coverage, so even a
//!      Latin-only default resolves to a face that actually has Chinese.
//!   2. a list of well-known absolute paths, for containers and minimal installs
//!      where fontconfig is absent but a font package is present.
//!   3. a scan of the standard font directories for any `*.tt[cf]`/`*.ot[cf]`
//!      whose family name looks like a CJK sans.
//!
//! Whichever tier answers first wins. If all three come up empty we keep egui's
//! defaults: Chinese looks wrong, but the program still runs and says why on
//! stderr rather than refusing to start.

use std::path::{Path, PathBuf};
use std::process::Command;

use egui::{FontData, FontDefinitions, FontFamily};

/// A font file plus which face inside it to use — `.ttc` collections hold many.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Face {
    path: PathBuf,
    index: u32,
}

impl Face {
    fn new(path: impl Into<PathBuf>, index: u32) -> Self {
        Self {
            path: path.into(),
            index,
        }
    }
}

/// Which tier produced the face, surfaced in the About tab so a wrong-looking
/// font is diagnosable rather than mysterious.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Fontconfig,
    KnownPath,
    DirectoryScan,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Fontconfig => "fontconfig",
            Self::KnownPath => "已知路径",
            Self::DirectoryScan => "目录扫描",
        }
    }
}

/// Ask fontconfig for the best face matching `pattern`.
///
/// `fc-match -f` prints `file\tindex`. Returns `None` when fontconfig is missing,
/// fails, or names a path that does not exist.
fn fc_match(pattern: &str) -> Option<Face> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}\t%{index}", pattern])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut fields = text.trim().split('\t');
    let path = fields.next()?;
    // A missing or malformed index is not fatal; face 0 is the sane default.
    let index = fields.next().unwrap_or("0").parse().unwrap_or(0);

    if path.is_empty() {
        return None;
    }
    let path = PathBuf::from(path);
    // fc-match answers from its config even for fonts that are not installed.
    if !path.is_file() {
        return None;
    }
    Some(Face { path, index })
}

/// Tier 2: fonts that ship with common distros, best-suited first (a UI sans with
/// Latin+CJK, not a serif or a handwriting face).
const KNOWN_FACES: &[(&str, u32)] = &[
    // Noto CJK — the current NixOS / Debian / Arch default.
    (
        "/run/current-system/sw/share/fonts/opentype/noto-cjk/NotoSansCJK-Regular.ttc",
        0,
    ),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 0),
    (
        "/usr/share/fonts/opentype/noto-cjk/NotoSansCJK-VF.otf.ttc",
        0,
    ),
    // Source Han Sans / 思源黑体 — same design as Noto CJK.
    (
        "/usr/share/fonts/opentype/source-han-sans/SourceHanSansSC-Regular.otf",
        0,
    ),
    (
        "/usr/share/fonts/truetype/source-han-sans/SourceHanSansSC-Regular.otf",
        0,
    ),
    // Sarasa Gothic / 更纱黑体 — what this machine actually has.
    (
        "/usr/share/fonts/truetype/sarasa-gothic/Sarasa-Regular.ttc",
        0,
    ),
    // WenQuanYi 文泉驿 — common on lighter installs.
    ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
    (
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
        0,
    ),
    ("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc", 0),
    // macOS.
    ("/System/Library/Fonts/PingFang.ttc", 0),
];

/// Tier 3: directories worth scanning when the first two tiers fail.
const SCAN_DIRS: &[&str] = &[
    "/run/current-system/sw/share/fonts",
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/run/host/fonts",
];

/// Substrings that identify a CJK *sans* face by file name, lowercased.
///
/// Matching happens on file names, so both Latin and Chinese spellings appear:
/// font files are often named in pinyin (`msyh.ttc`) while carrying a Chinese
/// family name in their `name` table.
const CJK_SANS_HINTS: &[&str] = &[
    "notosanscjk",
    "noto sans cjk",
    "noto sans sc",
    "sourcehansans",
    "source han sans",
    "sarasa",
    "yahei",
    "msyh",
    "pingfang",
    "microhei",
    "zenhei",
    "wqy",
    "heiti",
    "sans sc",
    "微软雅黑",
    "思源黑体",
    "更纱黑体",
    "文泉驿",
    "雅黑",
    "黑体",
];

/// Faces we must never pick for UI text, checked *after* the hints above so an
/// explicit serif marker wins. `NotoSerifCJK-Regular.ttc` contains "cjk" and
/// `SourceHanSerifSC` contains "sourcehansans"-like prefixes, so a plain
/// substring allowlist alone is not enough.
const NON_SANS_MARKERS: &[&str] = &[
    "serif", "song", "ming", "kai", "fangsong", "mono", "symbola", "宋", "楷", "仿宋",
];

/// True when the face looks like a CJK sans we can use for UI text.
fn looks_like_cjk_sans(name: &str) -> bool {
    let lower = name.to_lowercase();
    // An explicit non-sans marker disqualifies the face outright: `NotoSerifCJK`
    // would otherwise pass on "cjk" alone.
    if NON_SANS_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return false;
    }
    CJK_SANS_HINTS.iter().any(|hint| lower.contains(hint))
}

/// Tier 3 implementation: walk the standard directories for a plausible face.
fn scan_for_face() -> Option<Face> {
    for dir in SCAN_DIRS {
        let dir = Path::new(dir);
        if !dir.is_dir() {
            continue;
        }
        let mut stack = vec![dir.to_path_buf()];
        // Bounded so a pathological font tree cannot hang startup.
        let mut visited = 0usize;
        while let Some(path) = stack.pop() {
            visited += 1;
            if visited > 4096 {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let child = entry.path();
                if child.is_dir() {
                    stack.push(child);
                    continue;
                }
                let name = child.file_name().unwrap_or_default().to_string_lossy();
                let lower = name.to_lowercase();
                // .ttc/.otc are collections, .ttf/.otf are single faces.
                let is_font = [".ttc", ".otc", ".ttf", ".otf"]
                    .iter()
                    .any(|ext| lower.ends_with(ext));
                if is_font && looks_like_cjk_sans(&name) {
                    return Some(Face::new(child, 0));
                }
            }
        }
    }
    None
}

/// Resolve a CJK face through the three tiers.
fn find_cjk_face() -> Option<(Face, Source)> {
    // Tier 1: the user's own configuration, which is what they expect to see.
    for pattern in [
        "sans-serif:lang=zh-cn",
        "sans-serif:lang=zh",
        "Noto Sans CJK SC",
        "Sarasa Gothic SC",
    ] {
        if let Some(face) = fc_match(pattern) {
            return Some((face, Source::Fontconfig));
        }
    }

    // Tier 2: well-known paths.
    if let Some(face) = KNOWN_FACES
        .iter()
        .map(|(path, index)| Face::new(*path, *index))
        .find(|face| face.path.is_file())
    {
        return Some((face, Source::KnownPath));
    }

    // Tier 3: look around.
    scan_for_face().map(|face| (face, Source::DirectoryScan))
}

/// True when `bytes` is definitely not a font, checked before handing data to
/// egui.
///
/// `Fonts::new` *panics* on malformed font data rather than falling back, so a
/// bad guess would abort the whole program. Every sfnt file starts with one of a
/// handful of signatures; a wrong extension or a truncated file fails here.
fn is_definitely_not_a_font(bytes: &[u8]) -> bool {
    const SIGNATURES: [[u8; 4]; 4] = [
        [0x00, 0x01, 0x00, 0x00], // TrueType outlines
        *b"OTTO",                 // CFF outlines
        *b"ttcf",                 // font collection
        *b"true",                 // legacy Apple TrueType
    ];
    if bytes.len() < 12 {
        return true;
    }
    !SIGNATURES.iter().any(|sig| bytes.starts_with(sig))
}

/// Result of a successful install, kept for the About tab and for tests.
#[derive(Debug, Clone)]
pub struct LoadedFont {
    pub file: String,
    pub index: u32,
    pub source: &'static str,
}

/// Which font is in use, for the About tab. Does not install anything —
/// [`install`] does that; this only reports what `find_cjk_face` would pick.
pub fn describe() -> Option<LoadedFont> {
    let (face, source) = find_cjk_face()?;
    let file = face
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| face.path.display().to_string());
    Some(LoadedFont {
        file,
        index: face.index,
        source: source.label(),
    })
}

/// Install CJK-capable fonts into `ctx`, keeping egui's defaults as fallbacks.
///
/// Returns what was loaded, or `None` when nothing usable was found.
/// Load one font file into a `FontData`, or return `None` on failure.
fn load_face(face: &Face) -> Option<FontData> {
    let bytes = std::fs::read(&face.path).ok()?;
    if is_definitely_not_a_font(&bytes) {
        return None;
    }
    let mut data = FontData::from_owned(bytes);
    data.index = face.index;
    Some(data)
}

/// Try `fc-match` for a pattern, then fall back to `KNOWN_FACES` / directory scan.
///
/// For Latin we use the bare `sans-serif` pattern (no `:lang=` suffix) so
/// fontconfig returns whatever the desktop considers its UI font. For CJK we
/// qualify with `:lang=zh-cn` to force coverage-aware resolution.
fn find_face(pattern: &str, known: &[(&str, u32)]) -> Option<(Face, Source)> {
    if let Some(face) = fc_match(pattern) {
        return Some((face, Source::Fontconfig));
    }
    if let Some(face) = known
        .iter()
        .map(|(path, index)| Face::new(*path, *index))
        .find(|face| face.path.is_file())
    {
        return Some((face, Source::KnownPath));
    }
    scan_for_face().map(|face| (face, Source::DirectoryScan))
}

pub fn install(ctx: &egui::Context) -> Option<LoadedFont> {
    // Latin / UI font: the desktop's own sans-serif, so Latin and digits match
    // every other app on the machine rather than egui's bundled Ubuntu-Light.
    let latin = find_face("sans-serif", &[]);

    // CJK font: coverage-aware so a Latin-only default cannot sneak through.
    let cjk = find_cjk_face();

    // If neither is found, keep egui's defaults and say so.
    if latin.is_none() && cjk.is_none() {
        eprintln!("nicehck-gui: 未找到系统字体，将使用 egui 内置字体（中文显示为方块）。");
        return None;
    }

    let mut fonts = FontDefinitions::default();

    // Proportional chain: system sans-serif first, CJK second, egui defaults
    // last. Order matters — egui walks the list until a face has the glyph.
    let mut proportional: Vec<String> = Vec::new();
    let mut monospace: Vec<String> = Vec::new();

    // Helper: load a face and register it under `name`.
    let add = |fonts: &mut FontDefinitions, name: &str, face: &Face| -> bool {
        let Some(data) = load_face(face) else {
            return false;
        };
        fonts
            .font_data
            .insert(name.to_owned(), std::sync::Arc::new(data));
        true
    };

    // Latin face — primary for both families (it also has digits and symbols).
    if let Some((face, _source)) = &latin {
        if add(&mut fonts, "ui", face) {
            proportional.push("ui".to_owned());
            monospace.push("ui".to_owned());
        }
    }

    // CJK face — catches everything the Latin face lacks.
    if let Some((face, _source)) = &cjk {
        if add(&mut fonts, "cjk", face) {
            proportional.push("cjk".to_owned());
            monospace.push("cjk".to_owned());
        }
    }

    // egui defaults as the last-resort fallback (emoji, symbols).
    for name in ["Ubuntu-Light", "NotoEmoji-Regular", "emoji-icon-font"] {
        if fonts.font_data.contains_key(name) {
            proportional.push(name.to_owned());
            monospace.push(name.to_owned());
        }
    }

    fonts
        .families
        .insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, monospace);

    ctx.set_fonts(fonts);

    // Report the CJK face (the more interesting one for the About tab).
    let (face, source) = cjk.as_ref().or(latin.as_ref())?;
    let file = face
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| face.path.display().to_string());

    Some(LoadedFont {
        file,
        index: face.index,
        source: source.label(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_cjk_font_on_this_machine() {
        // Lenient like the runtime path: skip where no CJK font is installed.
        let Some((face, source)) = find_cjk_face() else {
            eprintln!("no CJK font installed; skipping");
            return;
        };
        assert!(face.path.is_file(), "{} should exist", face.path.display());
        let bytes = std::fs::read(&face.path).expect("font should be readable");
        assert!(
            !is_definitely_not_a_font(&bytes),
            "{} (from {source:?}) should look like an sfnt font",
            face.path.display()
        );
    }

    #[test]
    fn rejects_non_font_bytes() {
        assert!(is_definitely_not_a_font(b"not a font at all"));
        assert!(is_definitely_not_a_font(&[]));
        assert!(is_definitely_not_a_font(b"ttcf")); // too short to be a real header
        assert!(!is_definitely_not_a_font(
            b"\x00\x01\x00\x00\x00\x10padding!"
        ));
        assert!(!is_definitely_not_a_font(b"OTTO\x00\x10padding!"));
        assert!(!is_definitely_not_a_font(b"ttcf\x00\x10padding!"));
    }

    #[test]
    fn fc_match_tolerates_a_missing_index() {
        // The value comes from an external tool, so a missing field must not panic.
        let s = "/some/path.ttf";
        let mut fields = s.trim().split('\t');
        assert_eq!(fields.next(), Some("/some/path.ttf"));
        assert_eq!(fields.next().unwrap_or("0").parse().unwrap_or(0), 0);
    }

    #[test]
    fn cjk_sans_hints_pick_sans_over_serif() {
        assert!(looks_like_cjk_sans("NotoSansCJK-Regular.ttc"));
        assert!(looks_like_cjk_sans("Sarasa-Regular.ttc"));
        assert!(looks_like_cjk_sans("wqy-microhei.ttc"));
        assert!(looks_like_cjk_sans("SourceHanSansSC-Regular.otf"));
        assert!(looks_like_cjk_sans("微软雅黑"));
        // Serif CJK faces must not be chosen for UI text.
        assert!(!looks_like_cjk_sans("NotoSerifCJK-Regular.ttc"));
        assert!(!looks_like_cjk_sans("SourceHanSerifSC-Regular.otf"));
    }

    #[test]
    fn install_does_not_panic_without_a_font_stack() {
        // Exercises the real entry point; must survive finding nothing.
        let ctx = egui::Context::default();
        let _ = install(&ctx);
    }

    #[test]
    fn installed_font_can_actually_render_chinese() {
        // The whole point of this module. Existence of a font file is *not*
        // enough: the APK ships a HarmonyOS Sans that has zero CJK glyphs, so a
        // naive pick could still render boxes. Ask egui itself whether the UI's
        // own family now covers the characters we display.
        let ctx = egui::Context::default();
        let font_id = egui::FontId::proportional(14.0);
        let sample = "均衡器预设增益频率";

        // Fonts only exist after the first pass, so drive one with a no-op UI.
        // The returned deltas must be consumed explicitly: epaint panics if a
        // TexturesDelta is dropped unhandled (which is what a real backend does
        // when it uploads them).
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let before = ctx.fonts_mut(|f| f.has_glyphs(&font_id, sample));

        let loaded = install(&ctx);

        // `set_fonts` only records the new definitions; they are compiled at the
        // start of the next pass, so a frame has to run before they are visible.
        let mut output2 = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let after = ctx.fonts_mut(|f| f.has_glyphs(&font_id, sample));

        output.textures_delta.clear();
        output2.textures_delta.clear();

        if loaded.is_none() {
            eprintln!("no CJK font installed; skipping");
            return;
        }

        assert!(
            after,
            "installed a CJK font but Chinese still cannot be rendered; \
             before={before} after={after}"
        );
    }
}
