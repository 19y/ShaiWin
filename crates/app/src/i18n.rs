//! The handful of strings the app shows. Two languages for now (Simplified
//! Chinese and English), chosen from the Windows display language.

use std::sync::OnceLock;

use windows::Win32::Globalization::GetUserDefaultUILanguage;

pub fn is_chinese() -> bool {
    static CHINESE: OnceLock<bool> = OnceLock::new();
    *CHINESE.get_or_init(|| {
        let lang = unsafe { GetUserDefaultUILanguage() };
        // The primary language id lives in the low 10 bits. 0x04 is Chinese.
        (lang & 0x3FF) == 0x04
    })
}

/// Looks up a key. Unknown keys come back as themselves, which makes a missing
/// translation obvious without crashing.
pub fn t(key: &str) -> &'static str {
    if is_chinese() {
        zh(key)
    } else {
        en(key)
    }
}

macro_rules! table {
    ($($key:literal => $en:literal | $zh:literal),* $(,)?) => {
        pub fn en(key: &str) -> &'static str {
            match key { $($key => $en,)* _ => leak(key) }
        }
        pub fn zh(key: &str) -> &'static str {
            match key { $($key => $zh,)* _ => en(key) }
        }
    };
}

fn leak(key: &str) -> &'static str {
    // Missing keys are a developer error, not user input; leaking one short
    // string at most once per process is harmless.
    Box::leak(key.to_string().into_boxed_str())
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
    "hung_one" => "Hung up" | "已挂起",
    "taken_one" => "Taken down" | "已取下",
    "hook_hint" => "It is still hooked: minimize the window and it will come back to the line. Right-click the card to stop managing it." | "窗口仍然挂在晾衣架上：把它最小化就会自动回来；右键卡片可以“取消挂起”。",
    "peek_open" => "Show the line" | "展开晾衣架",
    "hotkey_failed" => "Some shortcuts are already in use" | "有些快捷键被占用了",
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
    "howto_body" => "The line hangs along the top of the screen. Drag a window into it, or press the hang shortcut, to hang it up. Click a card to bring the window back; minimize it and it returns to the line by itself. Drag a card sideways to move it, its side edges to change its width, and its bottom edge or bottom corners to change its height." | "晾衣架就挂在屏幕上方。把窗口拖进去，或按设置里的挂起快捷键。点击卡片让窗口回到桌面；最小化后它会自己回到晾衣架上。左右拖动卡片可以换位置，拖两侧改宽窄，拖底边或底角改高矮。",
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
    "settings_save" => "Save" | "保存",
    "settings_cancel" => "Cancel" | "取消",
    "settings_no_mods" => "Give the shortcut at least one modifier (Ctrl, Alt, Shift or Win)." | "快捷键至少要带一个 Ctrl、Alt、Shift 或 Win。",
    "settings_hotkey_failed" => "That shortcut could not be registered. Another program may be using it." | "这个快捷键注册失败，可能被其他程序占用了。",
    "tag_menu" => "Set tag…" | "设置标签…",
    "tag_current" => "Tag: " | "标签：",
    "tag_clear" => "Remove tag" | "清除标签",
    "tag_title" => "Window tag" | "窗口标签",
    "tag_placeholder" => "A name you will recognise" | "起个一眼能认出的名字",
    "tag_ok" => "Save" | "保存",
    "tag_cancel" => "Cancel" | "取消",
}

/// The empty line's prompt uses the user's own hang shortcut.
pub fn empty_hint(hotkey: &str) -> String {
    if is_chinese() {
        format!("把窗口拖到这里，或按 {hotkey}")
    } else {
        format!("Drag a window up here, or press {hotkey}")
    }
}

/// Formats a count into a short label for the tray tooltip.
pub fn tray_tip(count: usize) -> String {
    if is_chinese() {
        format!("晒窗 · 已挂起 {count} 个窗口")
    } else if count == 1 {
        "ShaiWin · 1 window hung up".to_string()
    } else {
        format!("ShaiWin · {count} windows hung up")
    }
}
