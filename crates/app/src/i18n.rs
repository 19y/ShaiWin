//! The strings the app shows, and the language pack files that may replace them.
//!
//! Two languages ship with the program: English and Simplified Chinese. A third
//! is a file. `%LOCALAPPDATA%\ShaiWin\languages\<tag>.txt` holds `key=value`
//! lines, `#` starts a comment, and a key beginning with `_` describes the pack
//! instead of translating it (`_name` is what the language calls itself, `_font`
//! the face it needs). A pack only has to carry the lines it wants to change:
//! anything missing falls back to the language the program ships with, and then
//! to English, so a twenty-line file is a working translation.
//!
//! The shipped languages are written into that folder on every start - never
//! over a file the user has touched - so there is always a complete, editable
//! copy of every string lying around to begin a new language from.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use windows::core::PCWSTR;
use windows::Win32::Globalization::GetUserDefaultUILanguage;

/// Not a language: show whatever the system speaks. The only value of
/// [`Settings::lang`](crate::settings::Settings::lang) that is not a tag.
pub const AUTO: &str = "auto";
/// The language every fallback ends in, and the one the table is written in.
pub const EN: &str = "en";
/// Simplified Chinese, the language the program was written for.
pub const ZH: &str = "zh-CN";

const FACE_CJK: &str = "Microsoft YaHei UI";
const FACE_LATIN: &str = "Segoe UI";
/// Tags that name a language the program has strings for, in the order the
/// settings row shows them.
const SHIPPED: [&str; 2] = [EN, ZH];

pub fn languages_dir() -> PathBuf {
    crate::settings::data_dir().join("languages")
}

/// A tag becomes a file name, and it also comes out of a settings file somebody
/// edited by hand, so it is checked rather than trusted: nothing that could
/// climb out of the languages folder, and nothing too long to be a tag.
pub fn is_safe_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 40
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What the stored or typed tag means once it has been looked at: `auto`, a tag
/// that could be a file name, or `auto` again.
pub fn lang_tag(tag: &str) -> String {
    let tag = tag.trim();
    if tag == AUTO || is_safe_tag(tag) {
        tag.to_string()
    } else {
        AUTO.to_string()
    }
}

/// The language the system speaks. Everything that is not Chinese is answered
/// with English, which is the language the table itself is written in.
pub fn system_tag() -> String {
    let lang = unsafe { GetUserDefaultUILanguage() };
    // The primary language id is the low ten bits; 0x04 is Chinese.
    if (lang & 0x3FF) == 0x04 {
        ZH.to_string()
    } else {
        EN.to_string()
    }
}

/// `auto` is what it says; any other tag is the answer.
pub fn resolve(tag: &str) -> String {
    if tag == AUTO {
        system_tag()
    } else {
        tag.to_string()
    }
}

struct Catalog {
    strings: HashMap<&'static str, &'static str>,
    /// The face this language is drawn with, as a null-terminated wide string
    /// that lives as long as the process. Decided from the language itself -
    /// see [`needs_cjk`] - so a pack never has to name a font to get one.
    face: &'static [u16],
}

/// A language written in Chinese, Japanese or Korean glyphs needs a face that
/// carries them. Decided from the tag and from the strings the pack actually
/// holds, because a partial Japanese pack would otherwise ask for Segoe and
/// render its own five lines as empty boxes.
fn needs_cjk(tag: &str, strings: &HashMap<&'static str, &'static str>) -> bool {
    let prefix = tag.split(['-', '_']).next().unwrap_or("");
    let spoken_cjk = matches!(prefix, "zh" | "ja" | "ko");
    spoken_cjk
        || strings
            .values()
            .any(|value| value.chars().any(|c| matches!(c, '\u{2E80}'..='\u{9FFF}')))
}

impl Catalog {
    /// The strings in force for `tag`: English first because the table is written
    /// in it and every key exists there, then the shipped strings for this
    /// language, then the user's file for this language.
    fn load(tag: &str) -> Catalog {
        let mut strings: HashMap<&'static str, &'static str> =
            BUILTIN.iter().map(|(key, en, _)| (*key, *en)).collect();
        if tag == ZH {
            for &(key, _, zh) in BUILTIN {
                strings.insert(key, zh);
            }
        }
        let mut face: Option<&'static str> = None;
        if let Some(text) = read_pack(tag) {
            for (key, value) in parse_pack(&text) {
                match key {
                    "_name" => {}
                    "_font" => face = Some(intern(value.trim())),
                    _ => {
                        strings.insert(intern(key), intern(value));
                    }
                }
            }
        }
        let name = match face {
            Some(named) if !named.is_empty() => named.to_string(),
            _ if needs_cjk(tag, &strings) => FACE_CJK.to_string(),
            _ => FACE_LATIN.to_string(),
        };
        Catalog {
            strings,
            face: wide_null(&name),
        }
    }

    fn get(&self, key: &str) -> &'static str {
        // Every key in the table is already in the map, so this only answers a
        // key nobody ever declared - which is the bug it should be loud about.
        match self.strings.get(key) {
            Some(found) => found,
            None => builtin(EN, key).unwrap_or_else(|| intern(key)),
        }
    }
}

static CATALOG: OnceLock<RwLock<Catalog>> = OnceLock::new();

fn catalog() -> &'static RwLock<Catalog> {
    CATALOG.get_or_init(|| RwLock::new(Catalog::load(&system_tag())))
}

/// Switch to `tag` (`auto` follows the system) and reload the packs. Called at
/// startup with the stored setting and again when the settings window saves.
pub fn activate(tag: &str) {
    let loaded = Catalog::load(&lang_tag(&resolve(&lang_tag(tag))));
    let mut guard = match catalog().write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = loaded;
}

fn with_catalog<R>(f: impl FnOnce(&Catalog) -> R) -> R {
    // A poisoned lock means a panic happened while the catalog was being
    // replaced, which a release build cannot reach at all (`panic = "abort"`).
    // In a test build the language being one frame stale beats crashing.
    let guard = match catalog().read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&guard)
}

/// Looks up a key. An unknown key comes back as itself, which makes a missing
/// translation obvious without crashing.
pub fn t(key: &str) -> &'static str {
    with_catalog(|c| c.get(key))
}

/// A string with `{placeholder}` holes filled in.
pub fn t_fmt(key: &str, args: &[(&str, &str)]) -> String {
    fill(t(key), args)
}

/// The `{base}_one` / `{base}_other` pair English needs and Chinese does not. A
/// language with more than two forms asks for both keys and gets whichever it
/// defines; `_other` is the one that must exist.
pub fn t_count(base: &str, count: usize, args: &[(&str, &str)]) -> String {
    let key = if count == 1 {
        format!("{base}_one")
    } else {
        format!("{base}_other")
    };
    let found = with_catalog(|c| c.strings.contains_key(key.as_str()));
    let chosen = if found { key } else { format!("{base}_other") };
    let text = t(&chosen);
    let mut extra = args.to_vec();
    let count_text = count.to_string();
    extra.push(("{count}", count_text.as_str()));
    fill(text, &extra)
}

fn fill(text: &str, args: &[(impl AsRef<str>, impl AsRef<str>)]) -> String {
    let mut out = text.to_string();
    for (hole, value) in args {
        out = out.replace(hole.as_ref(), value.as_ref());
    }
    out
}

/// The face the interface is drawn with for the language in force.
pub fn ui_face() -> PCWSTR {
    with_catalog(|c| PCWSTR::from_raw(c.face.as_ptr()))
}

/// A language and what it calls itself: the name is never translated, because
/// "日本語" on a button is how you find the Japanese line when you cannot read
/// the interface.
pub struct Language {
    pub tag: String,
    pub name: String,
}

/// Every language the settings row can offer: `auto`, then the shipped ones,
/// then each file found in the languages folder. A file named after a shipped
/// language is that language with edits, not a second choice.
pub fn languages() -> Vec<Language> {
    let mut found = SHIPPED
        .iter()
        .map(|tag| Language {
            tag: tag.to_string(),
            name: native_name(tag),
        })
        .collect::<Vec<_>>();
    if let Ok(entries) = fs::read_dir(languages_dir()) {
        let mut extra = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("txt") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let tag = stem.to_string();
            // `auto` is the setting that follows the system, never a file of its
            // own: a folder that happens to hold `auto.txt` would otherwise offer
            // the row two buttons that mean the same thing.
            if !is_safe_tag(&tag) || tag == AUTO || SHIPPED.contains(&tag.as_str()) {
                continue;
            }
            let name = match read_pack(&tag).and_then(|text| pack_name(&text)) {
                Some(named) => named,
                None => tag.clone(),
            };
            extra.push(Language { tag, name });
        }
        extra.sort_by(|a, b| a.tag.cmp(&b.tag));
        found.extend(extra);
    }
    found
}

/// What a shipped language calls itself. Never translated: "日本語" on a button
/// is how you find the Japanese line in an interface you cannot read.
fn native_name(tag: &str) -> String {
    match tag {
        EN => "English".to_string(),
        ZH => "简体中文".to_string(),
        other => other.to_string(),
    }
}

fn read_pack(tag: &str) -> Option<String> {
    if !is_safe_tag(tag) {
        return None;
    }
    let path = languages_dir().join(format!("{tag}.txt"));
    match fs::read_to_string(&path) {
        // An editor that does not write UTF-8 - Notepad's "Unicode, little-endian"
        // or an ANSI save - is the one way a pack can be in the folder and still
        // not appear, so the refusal says which file and why.
        Err(why) if path.exists() => {
            crate::log!("language pack {} cannot be read as UTF-8: {why}", tag);
            None
        }
        Err(_) => None,
        Ok(text) if text.trim().is_empty() => None,
        Ok(text) => Some(text),
    }
}

/// The `_name` a pack gives itself, if it has one.
fn pack_name(text: &str) -> Option<String> {
    parse_pack(text)
        .into_iter()
        .find(|(key, _)| *key == "_name")
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

/// `key=value` lines, `#` comments, blank lines, and an empty value that means
/// "show nothing here" rather than "use the fallback".
fn parse_pack(text: &str) -> Vec<(&str, &str)> {
    // Notepad puts a byte-order mark in front of every UTF-8 file it saves, and
    // a pack is meant to be edited in it.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.starts_with('#') {
            continue;
        }
        out.push((key, value.trim()));
    }
    out
}

/// Write out the shipped languages so the folder holds something to copy and
/// translate. An existing file is never rewritten: it is the user's.
pub fn export_templates() {
    let dir = languages_dir();
    let mut fresh = false;
    for tag in SHIPPED {
        let path = dir.join(format!("{tag}.txt"));
        if path.exists() {
            continue;
        }
        if !fresh {
            let _ = fs::create_dir_all(&dir);
            fresh = true;
        }
        let _ = fs::write(&path, template(tag));
    }
}

/// A complete pack for a shipped language, with the holes explained in the
/// language being exported.
fn template(tag: &str) -> String {
    let mut text = format!("# ShaiWin language pack: {tag}\n# {usage}\n\n_name = {name}\n\n",
        usage = match tag {
            ZH => "一行一个 key = 文本。# 开头是注释。留空的行不算修改这一句，写 key = 才是把这句变成没有。\n# 只改你想改的行；没写的自动用程序自带的文本，最后兜底是英文。\n# _name 是这个语言对自己的称呼，_font 可以指定别的字体。",
            _ => "One key = text per line. # starts a comment. A line you leave out keeps\n# the string the program ships with, and English answers last.\n# _name is what this language calls itself; _font can name another face.",
        },
        name = native_name(tag),
    );
    let mut group = "";
    for (key, en, zh) in BUILTIN {
        let here = group_of(key, en);
        if here != group {
            group = here;
            if !here.is_empty() {
                text.push_str(&format!("\n# {here}\n"));
            }
        }
        let value = if tag == ZH { zh } else { en };
        text.push_str(&format!("{key} = {value}\n"));
    }
    text
}

/// The exporter reads the table's own naming to break the file into the sections
/// a person translating expects, so the grouping cannot drift from the strings.
/// The counts section is decided by the sentence rather than the key, because a
/// `_one` suffix is not proof that a number goes in the hole.
fn group_of(key: &str, en: &str) -> &'static str {
    if key.starts_with("rej_") {
        "why a window is refused"
    } else if key.starts_with("settings_") {
        "settings window"
    } else if key.starts_with("tag_") {
        "window tags"
    } else if key.starts_with("lang") {
        "language"
    } else if en.contains("{count}") {
        "counts"
    } else {
        ""
    }
}

/// A string that outlives the catalog it came from, so `t()` can still hand back
/// `&'static str` and the call sites stay as they were. Packs are read once per
/// language change; leaking their contents is the whole trade.
fn intern(text: &str) -> &'static str {
    Box::leak(text.to_string().into_boxed_str())
}

fn wide_null(text: &str) -> &'static [u16] {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    Box::leak(wide.into_boxed_slice())
}

macro_rules! table {
    ($($key:literal => $en:literal | $zh:literal),* $(,)?) => {
        /// Every string the program ships, as `(key, English, Simplified Chinese)`
        /// in the order the exporter writes them out.
        const BUILTIN: &[(&str, &str, &str)] = &[$(($key, $en, $zh),)*];
    };
}

table! {
    "app_name" => "ShaiWin" | "晒窗",
    "hang_window" => "Hang the window I am using" | "挂起当前窗口",
    "take_down_all" => "Take everything down" | "全部取下",
    "take_down" => "Take down" | "取下",
    "restore" => "Take it down and show it" | "取下并显示",
    "unhook" => "Stop managing this window" | "取消挂起（以后不再自动回来）",
    "show_line" => "Show the line" | "展开晾衣架",
    "hide_line" => "Hide the line" | "收起晾衣架",
    "open_at_login" => "Start with Windows" | "开机时启动",
    "quit" => "Quit ShaiWin" | "退出晒窗",
    "settings" => "Settings…" | "设置…",
    "excluded" => "Apps that cannot be hung" | "不允许挂起的应用",
    // The two words a card menu answers with. They are not a count pair: the menu
    // says what happened to the one card under the pointer.
    "hung_one" => "Hung up" | "已挂起",
    "taken_one" => "Taken down" | "已取下",
    "hook_hint" => "It is still hooked: minimize the window and it will come back to the line. Right-click the card to stop managing it." | "窗口仍然挂在晾衣架上：把它最小化就会自动回来；右键卡片可以“取消挂起”。",
    "raise_hint" => "Hover over the rope, or slide along it, and the cards float back up" | "鼠标悬浮或滑过绳子，卡片就会浮上来",
    "dont_remind" => "Don't remind me" | "不再提示",
    "peek_open" => "Show the line" | "展开晾衣架",
    "hotkey_failed" => "Some shortcuts are already in use" | "有些快捷键被占用了",
    "hotkey_failed_body" => "Taken: {list}. Change them under Settings in the tray menu." | "已被占用：{list}。可以在托盘菜单的「设置」里换一个组合。",
    "already_hung" => "That window is already on the line." | "这个窗口已经挂在晾衣架上。",
    "cannot_hang" => "Cannot hang that window" | "无法挂起这个窗口",
    "closed_itself" => "The window closed itself, so its card came down." | "窗口已关闭，卡片已自动取下。",
    "recovered" => "Restored windows from your last session." | "已恢复上次挂起的窗口。",
    "restore_failed" => "The window is gone." | "窗口已经不在了。",
    "rej_gone" => "That window has already closed." | "这个窗口已经关闭了。",
    "rej_ours" => "ShaiWin cannot hang its own windows." | "晒窗不能挂起自己的窗口。",
    "rej_hidden" => "That window is hidden." | "这个窗口是隐藏的。",
    "rej_notitle" => "That window has no title to show on a card." | "这个窗口没有标题，做不出卡片。",
    "rej_tool" => "Tool windows and palettes cannot be hung." | "工具窗口和浮动面板不能挂起。",
    "rej_cloaked" => "That window is on another desktop or is not visible." | "这个窗口在其他虚拟桌面，或当前不可见。",
    "rej_minimized" => "Restore the window before hanging it up." | "请先把窗口还原，再挂起。",
    "rej_elevated" => "That window runs as administrator, and Windows blocks a normal app from moving it. Restart ShaiWin with “Run as administrator” to hang it." | "这个窗口以管理员权限运行，普通权限的晒窗无法移动它。用“以管理员身份运行”重新打开晒窗就能挂起它。",
    "rej_small" => "That window is too small to make a useful card." | "这个窗口太小了，做不出有用的卡片。",
    "rej_excluded" => "That app is on your exclusion list." | "这个应用在排除列表里。",
    "howto_title" => "ShaiWin is running" | "晒窗已在后台运行",
    "howto_body" => "A line hangs at the top of your screen: drag a window into it, or use the shortcut. Click a card and its window comes back while the card stays on the rope. Right-click takes it down. Hover the rope to raise cards your work covers." | "晾衣架就挂在屏幕上方。把窗口拖进去，或按设置里的挂起快捷键。点一下卡片，窗口回到桌面并置顶，卡片仍挂在绳上，随时可以再点；右键卡片才能把它取下。卡片会被你正在用的窗口盖住，鼠标滑过绳子就浮上来。左右拖动卡片可以换位置，拖两侧改宽窄，拖底边或底角改高矮。",
    "tray_hint" => "Click the tray icon for more." | "右键托盘图标查看更多。",
    "elevated_tip" => "ShaiWin is running as administrator. Do that only when you need to hang administrator windows; running normally is safer." | "晒窗正以管理员权限运行。只有在需要挂起管理员窗口时才这样做，平时以普通权限运行更安全。",
    "settings_title" => "ShaiWin settings" | "晒窗设置",
    "settings_group_keys" => "Shortcuts" | "快捷键",
    "settings_group_line" => "The line" | "晾衣架",
    "settings_hang" => "Hang the current window" | "挂起当前窗口",
    "settings_toggle" => "Show / hide the line" | "展开 / 收起晾衣架",
    "settings_key_unset" => "Not set" | "未设置",
    "settings_key_capture" => "Press a shortcut" | "按下组合键",
    "settings_sag_note" => "Pull the rope, nudge it with the arrow keys, double-click for the shipped curve." | "按住绳子往下拉，或用方向键微调；双击回自动弧度。",
    "settings_auto" => "auto" | "自动",
    "settings_always" => "Keep the line on screen" | "晾衣架一直显示",
    "settings_always_note" => "Off: it opens from the top edge and tucks itself away when you are done." | "关掉后只在需要时展开，用完自己收起。",
    "settings_drag" => "Drag to the top to hang" | "拖到屏幕顶端就挂上",
    "settings_drag_note" => "Off: only the shortcut hangs a window." | "关掉后，只能靠快捷键挂起窗口。",
    "settings_stay" => "Cards stay on the rope" | "点开后卡片仍挂在绳上",
    "settings_stay_note" => "Off: clicking a card takes it down, and minimizing puts it back." | "关掉后，点卡片就是把窗口取下；再最小化才会挂回去。",
    "settings_save" => "Save" | "保存",
    "settings_cancel" => "Cancel" | "取消",
    "settings_no_mods" => "Give the shortcut at least one modifier (Ctrl, Alt, Shift or Win)." | "快捷键至少要带一个 Ctrl、Alt、Shift 或 Win。",
    "settings_hotkey_failed" => "That shortcut could not be registered. Another program may be using it." | "这个快捷键注册失败，可能被其他程序占用了。",
    "settings_lang" => "Interface language" | "界面语言",
    "settings_lang_note" => "Want another language? Open \"Language files…\" and drop in a <tag>.txt." | "想要别的语言？点「语言文件…」，放一个 <标签>.txt 进去。",
    "lang_auto" => "System" | "跟随系统",
    "lang_folder" => "Language files…" | "语言文件…",
    "tag_menu" => "Set tag…" | "设置标签…",
    "tag_current" => "Tag: {tag}" | "标签：{tag}",
    "tag_clear" => "Remove tag" | "清除标签",
    "tag_title" => "Window tag" | "窗口标签",
    "tag_placeholder" => "A name you will recognise" | "起个一眼能认出的名字",
    "tag_ok" => "Save" | "保存",
    "tag_cancel" => "Cancel" | "取消",
    "untitled_window" => "Window" | "窗口",
    "empty_hint" => "Drag a window up here, or press {hotkey}" | "把窗口拖到这里，或按 {hotkey}",
    "tray_tip_one" => "{app} · {count} window hung up" | "{app} · 已挂起 {count} 个窗口",
    "tray_tip_other" => "{app} · {count} windows hung up" | "{app} · 已挂起 {count} 个窗口",
}

/// The empty line's prompt uses the user's own hang shortcut.
pub fn empty_hint(hotkey: &str) -> String {
    t_fmt("empty_hint", &[("{hotkey}", hotkey)])
}

/// Formats a count into a short label for the tray tooltip.
pub fn tray_tip(count: usize) -> String {
    let app = t("app_name");
    t_count("tray_tip", count, &[("{app}", app)])
}

/// Built-in strings for `tag`, and `None` for a language the program has nothing
/// for. Used by the look-up and by the tests that check the table against itself.
pub fn builtin(tag: &str, key: &str) -> Option<&'static str> {
    if tag != EN && tag != ZH {
        return None;
    }
    BUILTIN
        .iter()
        .find(|row| row.0 == key)
        .map(|row| if tag == ZH { row.2 } else { row.1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two halves of the table are the same document. A key with no English
    /// is a key that shows a raw snake_case name, and a key with no Chinese is
    /// the same bug in the language the program was written for.
    #[test]
    fn every_string_exists_in_both_languages() {
        for (key, en, zh) in BUILTIN {
            assert!(!en.is_empty(), "{key} has no English");
            assert!(!zh.is_empty(), "{key} has no Chinese");
        }
        let mut keys: Vec<&str> = BUILTIN.iter().map(|(key, _, _)| *key).collect();
        let before = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(before, keys.len(), "a key is declared twice");
    }

    /// A translator who loses a hole gets a sentence with no number in it and no
    /// way to find out why, so the holes have to match line for line.
    #[test]
    fn the_holes_match_across_languages() {
        for &(key, en, zh) in BUILTIN {
            let (mut en, mut zh) = (holes(en), holes(zh));
            en.sort();
            zh.sort();
            assert_eq!(en, zh, "{key}");
        }
    }

    fn holes(text: &str) -> Vec<String> {
        text.split('{')
            .skip(1)
            .map(|part| part.split('}').next().unwrap_or(part).to_string())
            .collect()
    }

    /// `languages/ru.txt` is the proof that a language can be added without
    /// touching the program, so it is checked the way the program reads it: every
    /// line present, every hole kept, every balloon still fitting its balloon.
    /// A user's own pack is nobody's business; this one is the example.
    #[test]
    fn the_example_pack_covers_the_table() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../languages/ru.txt");
        let text = fs::read_to_string(path).expect("the Russian example pack is in the repo");
        let entries = parse_pack(&text);
        let value = |key: &str| entries.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        assert_eq!(pack_name(&text).as_deref(), Some("Русский"));
        assert_eq!(
            entries.iter().filter(|(k, _)| !k.starts_with('_')).count(),
            BUILTIN.len(),
            "the example pack lost or gained a line"
        );
        for &(key, en, zh) in BUILTIN {
            let Some(ru) = value(key) else {
                panic!("the example pack has no {key}");
            };
            let (mut want, mut have) = (holes(en), holes(ru));
            want.extend(holes(zh));
            want.sort();
            want.dedup();
            have.sort();
            have.dedup();
            assert_eq!(want, have, "{key} changed the holes");
            assert!(!ru.trim().is_empty(), "{key} is empty");
            if BALLOON_KEYS.contains(&key) {
                assert!(
                    ru.chars().count() <= crate::tray::BALLOON_MAX,
                    "{key} is {} characters, and the balloon cuts it at {}",
                    ru.chars().count(),
                    crate::tray::BALLOON_MAX
                );
            }
        }
    }

    #[test]
    fn a_pack_is_lines_of_key_value() {
        let text = "# comment\r\nquit = Exit now\r\n\r\nsettings_save =\r\n_name = Français\r\n_font = Segoe UI\r\n";
        let entries = parse_pack(text);
        assert_eq!(
            entries,
            vec![
                ("quit", "Exit now"),
                ("settings_save", ""),
                ("_name", "Français"),
                ("_font", "Segoe UI"),
            ]
        );
        assert_eq!(pack_name(text).as_deref(), Some("Français"));
        // The same file saved by Notepad, which writes a byte-order mark first.
        assert_eq!(
            pack_name(&format!("\u{FEFF}{text}")).as_deref(),
            Some("Français")
        );
    }

    /// A tag is a file name and it comes from a hand-edited settings file, so the
    /// one thing it must never be is a path.
    #[test]
    fn a_tag_is_a_file_name_and_nothing_else() {
        assert!(is_safe_tag("zh-CN"));
        assert!(is_safe_tag("pt_BR"));
        assert!(!is_safe_tag("../settings"));
        assert!(!is_safe_tag("a/b"));
        assert!(!is_safe_tag("a\\b"));
        assert!(!is_safe_tag(""));
        assert!(!is_safe_tag(&"x".repeat(41)));
        assert_eq!(lang_tag("  ja "), "ja");
        assert_eq!(lang_tag(".."), AUTO);
        assert_eq!(lang_tag("anything else"), AUTO);
    }

    #[test]
    fn the_pack_survives_a_round_trip() {
        for tag in SHIPPED {
            let pack = template(tag);
            let entries = parse_pack(&pack);
            let named = entries
                .iter()
                .filter(|(key, _)| !key.starts_with('_'))
                .count();
            assert_eq!(named, BUILTIN.len(), "{tag} lost or gained a line");
            for (key, _, _) in BUILTIN {
                let value = entries
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| *v)
                    .unwrap_or_default();
                assert_eq!(value, builtin(tag, key).unwrap_or_default(), "{key}");
            }
        }
    }

    /// The strings that end up in a tray balloon, and therefore in an array
    /// Windows cuts off without asking anybody.
    const BALLOON_KEYS: &[&str] = &[
        "howto_body",
        "hook_hint",
        "elevated_tip",
        "hotkey_failed_body",
    ];

    /// The tray balloon holds 255 characters and then cuts, which is how the
    /// English first-run lesson used to end in the middle of a word.
    #[test]
    fn the_longest_balloons_fit_the_balloons() {
        for key in BALLOON_KEYS {
            for (built, text) in [(EN, builtin(EN, key)), (ZH, builtin(ZH, key))] {
                let text = text.expect("the key is in the table");
                assert!(
                    text.chars().count() <= crate::tray::BALLOON_MAX,
                    "{key} in {built} is {} characters",
                    text.chars().count()
                );
            }
        }
    }

    #[test]
    fn an_unknown_language_answers_in_english() {
        assert_eq!(builtin("fr", "quit"), None);
        assert_eq!(builtin(EN, "quit"), Some("Quit ShaiWin"));
        assert_eq!(builtin(ZH, "quit"), Some("退出晒窗"));
    }

    #[test]
    fn only_the_languages_we_have_are_cjk() {
        let empty = HashMap::new();
        assert!(needs_cjk(ZH, &empty));
        assert!(needs_cjk("ja", &empty));
        assert!(!needs_cjk(EN, &empty));
        // A half-translated Japanese pack still needs the face, and a French
        // pack that quotes a Japanese title gets one without asking.
        let mut quoted = HashMap::new();
        quoted.insert("quit", "閉じる");
        assert!(needs_cjk("ja-JP", &quoted));
        assert!(needs_cjk("fr", &quoted));
        assert!(!needs_cjk("fr", &empty));
    }

    #[test]
    fn holes_are_filled() {
        assert_eq!(
            fill("press {hotkey} now", &[("{hotkey}", "Ctrl+2")]),
            "press Ctrl+2 now"
        );
        assert_eq!(
            fill("nothing here", &[("{hotkey}", "Ctrl+2")]),
            "nothing here"
        );
    }
}
