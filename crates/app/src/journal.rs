//! The safety net.
//!
//! Before a window is moved off screen, a line is written here saying where it
//! was and where it is going. If ShaiWin exits normally it takes every window
//! down first and empties the file. If it crashes, the next start reads the
//! file and puts those windows back where they belong.
//!
//! The format is one tab-separated record per line. The title comes last
//! because it is the only free-form field.

use std::fs;
use std::path::PathBuf;

use shaiwin_platform_win::park::{Parked, Placement};
use shaiwin_platform_win::Rect;

use crate::settings;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub hwnd: isize,
    pub pid: u32,
    pub process_start: u64,
    pub title: String,
    pub exe: String,
    pub placement: Placement,
    pub parked: Parked,
    /// Card anchor on screen, so the line reopens on the right monitor.
    pub anchor: (i32, i32),
    /// The window is back on the desktop but still hooked: its card is hidden,
    /// and minimizing the window hangs it up again. Recorded so the hook
    /// survives ShaiWin restarting, which is what the user expects from
    /// "minimize it and it comes back".
    pub hooked_away: bool,
    /// How wide this card's window is drawn on the line. The user sets it by
    /// dragging the card's edge; it is recorded so a restart keeps the line the
    /// size they left it.
    pub zoom: f32,
    /// How tall this card's window is drawn on the line, as a multiple of its
    /// normal height. Set by dragging the card's bottom edge.
    pub zoom_y: f32,
    /// How far the card was dragged from its packed place, in logical pixels.
    /// Positive is right. Recorded so a card stays where the user put it.
    pub offset: f32,
    /// The window was dragged onto the line, so it opens in the middle of the
    /// screen instead of where it was. Recorded so a restart keeps that promise.
    pub centred: bool,
    /// The user's short label for the card. Empty when untagged. Mainly so a
    /// loaded line can be read at a glance; it never affects parking.
    pub tag: String,
}

fn file() -> PathBuf {
    settings::data_dir().join("parked.txt")
}

pub fn load() -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(file()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(entry) = parse(line) {
            out.push(entry);
        }
    }
    out
}

fn parse(line: &str) -> Option<Entry> {
    let mut f = line.splitn(18, '\t');
    let hwnd = f.next()?.parse().ok()?;
    let pid = f.next()?.parse().ok()?;
    let process_start = f.next()?.parse().ok()?;
    let title = unescape(f.next()?);
    let exe = f.next()?.to_string();
    let flags = f.next()?.parse().ok()?;
    let show_cmd = f.next()?.parse().ok()?;
    let normal = rect(f.next()?)?;
    let (min_x, min_y) = pair(f.next()?)?;
    let (max_x, max_y) = pair(f.next()?)?;
    let parked_rect = rect(f.next()?)?;
    let (anchor_x, anchor_y) = pair(f.next()?)?;
    // Records written before the hook was stored have no 13th field.
    let hooked_away = f.next() == Some("1");
    let zoom = f.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let offset = f.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    // Appended last so a record written before the height existed still has its
    // zoom and offset read from the same fields.
    let zoom_y = f.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    // Records written before the drag/hotkey distinction have no 17th field;
    // they open where they were, which is what every hang did back then.
    let centred = f.next() == Some("1");
    // The tag arrived last, so every record written before it still reads.
    let tag = f.next().map(unescape).unwrap_or_default();
    Some(Entry {
        hwnd,
        pid,
        process_start,
        title,
        exe,
        placement: Placement {
            flags,
            show_cmd,
            min_x,
            min_y,
            max_x,
            max_y,
            normal,
        },
        parked: Parked {
            placement: Placement {
                flags,
                show_cmd,
                min_x,
                min_y,
                max_x,
                max_y,
                normal,
            },
            parked: parked_rect,
            was_maximized: show_cmd == 3,
        },
        anchor: (anchor_x, anchor_y),
        hooked_away,
        zoom,
        zoom_y,
        offset,
        centred,
        tag,
    })
}

fn rect(s: &str) -> Option<Rect> {
    let mut parts = s.split(',');
    Some(Rect {
        left: parts.next()?.parse().ok()?,
        top: parts.next()?.parse().ok()?,
        right: parts.next()?.parse().ok()?,
        bottom: parts.next()?.parse().ok()?,
    })
}

fn pair(s: &str) -> Option<(i32, i32)> {
    let mut parts = s.split(',');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn escape(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn unescape(s: &str) -> String {
    s.to_string()
}

fn line(entry: &Entry) -> String {
    let p = &entry.placement;
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{},{},{},{}\t{},{}\t{},{}\t{},{},{},{}\t{},{}\t{}\t{:.3}\t{:.1}\t{:.3}\t{}\t{}\n",
        entry.hwnd,
        entry.pid,
        entry.process_start,
        escape(&entry.title),
        entry.exe,
        p.flags,
        p.show_cmd,
        p.normal.left,
        p.normal.top,
        p.normal.right,
        p.normal.bottom,
        p.min_x,
        p.min_y,
        p.max_x,
        p.max_y,
        entry.parked.parked.left,
        entry.parked.parked.top,
        entry.parked.parked.right,
        entry.parked.parked.bottom,
        entry.anchor.0,
        entry.anchor.1,
        u8::from(entry.hooked_away),
        entry.zoom,
        entry.offset,
        entry.zoom_y,
        u8::from(entry.centred),
        escape(&entry.tag),
    )
}

pub fn save(entries: &[Entry]) {
    let file = file();
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if entries.is_empty() {
        let _ = fs::remove_file(&file);
        return;
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str(&line(entry));
    }
    // Write beside the real file and swap it in, so a crash mid-write cannot
    // leave a half-written journal.
    let tmp = file.with_extension("tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, &file);
    }
}

pub fn clear() {
    let _ = fs::remove_file(file());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let entry = Entry {
            hwnd: 12345,
            pid: 999,
            process_start: 42,
            title: "A window with | a pipe".to_string(),
            exe: "chrome.exe".to_string(),
            placement: Placement {
                flags: 0,
                show_cmd: 3,
                min_x: -1,
                min_y: -1,
                max_x: -2,
                max_y: -2,
                normal: Rect {
                    left: 10,
                    top: 20,
                    right: 300,
                    bottom: 400,
                },
            },
            parked: Parked {
                placement: Placement {
                    flags: 0,
                    show_cmd: 3,
                    min_x: -1,
                    min_y: -1,
                    max_x: -2,
                    max_y: -2,
                    normal: Rect {
                        left: 10,
                        top: 20,
                        right: 300,
                        bottom: 400,
                    },
                },
                parked: Rect {
                    left: 2000,
                    top: 40,
                    right: 2500,
                    bottom: 800,
                },
                was_maximized: true,
            },
            anchor: (1920, 0),
            hooked_away: true,
            zoom: 1.35,
            zoom_y: 1.6,
            offset: -42.0,
            centred: true,
            tag: "work".to_string(),
        };
        let text = line(&entry);
        let back = parse(text.trim_end()).expect("parses");
        assert_eq!(back.hwnd, entry.hwnd);
        assert_eq!(back.title, entry.title);
        assert_eq!(back.placement.normal, entry.placement.normal);
        assert_eq!(back.parked.parked, entry.parked.parked);
        assert_eq!(back.anchor, entry.anchor);
        assert_eq!(back.hooked_away, entry.hooked_away);
        assert!(back.centred);
        assert_eq!(back.tag, entry.tag);
        assert!((back.zoom - entry.zoom).abs() < 0.001);
        assert!((back.zoom_y - entry.zoom_y).abs() < 0.001);
        assert!((back.offset - entry.offset).abs() < 0.1);
    }

    #[test]
    fn a_record_without_the_hook_field_still_reads() {
        // Records written before the hook was stored end at the anchor.
        let entry = Entry {
            hwnd: 1,
            pid: 2,
            process_start: 3,
            title: "t".to_string(),
            exe: "a.exe".to_string(),
            placement: Placement {
                flags: 0,
                show_cmd: 1,
                min_x: 0,
                min_y: 0,
                max_x: 0,
                max_y: 0,
                normal: Rect {
                    left: 0,
                    top: 0,
                    right: 10,
                    bottom: 10,
                },
            },
            parked: Parked {
                placement: Placement {
                    flags: 0,
                    show_cmd: 1,
                    min_x: 0,
                    min_y: 0,
                    max_x: 0,
                    max_y: 0,
                    normal: Rect {
                        left: 0,
                        top: 0,
                        right: 10,
                        bottom: 10,
                    },
                },
                parked: Rect {
                    left: 0,
                    top: 0,
                    right: 10,
                    bottom: 10,
                },
                was_maximized: false,
            },
            anchor: (0, 0),
            hooked_away: false,
            zoom: 1.0,
            zoom_y: 1.0,
            offset: 0.0,
            centred: false,
            tag: String::new(),
        };
        // An old record is today's line with the fields added since removed.
        let full = line(&entry).trim_end_matches('\n').to_string();
        let mut head = full.as_str();
        for _ in 0..6 {
            head = head.rsplit_once('\t').expect("more than one field").0;
        }
        let back = parse(head).expect("parses");
        assert!(!back.hooked_away);
        assert!(!back.centred);
        assert_eq!(back.hwnd, entry.hwnd);
        assert_eq!(back.anchor, entry.anchor);
        assert_eq!(back.zoom, 1.0);
        assert_eq!(back.zoom_y, 1.0);
        assert_eq!(back.offset, 0.0);
    }

    #[test]
    fn a_record_from_before_the_height_still_keeps_its_zoom_and_offset() {
        // The height and the centring flag are the last fields, so an old
        // record's zoom and offset are still read from where they were, and
        // only those last two default.
        let entry = Entry {
            hwnd: 7,
            pid: 8,
            process_start: 9,
            title: "t".to_string(),
            exe: "a.exe".to_string(),
            placement: Placement {
                flags: 0,
                show_cmd: 1,
                min_x: 0,
                min_y: 0,
                max_x: 0,
                max_y: 0,
                normal: Rect {
                    left: 0,
                    top: 0,
                    right: 10,
                    bottom: 10,
                },
            },
            parked: Parked {
                placement: Placement {
                    flags: 0,
                    show_cmd: 1,
                    min_x: 0,
                    min_y: 0,
                    max_x: 0,
                    max_y: 0,
                    normal: Rect {
                        left: 0,
                        top: 0,
                        right: 10,
                        bottom: 10,
                    },
                },
                parked: Rect {
                    left: 0,
                    top: 0,
                    right: 10,
                    bottom: 10,
                },
                was_maximized: false,
            },
            anchor: (0, 0),
            hooked_away: true,
            zoom: 1.45,
            zoom_y: 1.6,
            offset: -37.0,
            centred: true,
            tag: String::new(),
        };
        let full = line(&entry).trim_end_matches('\n').to_string();
        // Drop the tag, the centring flag and the height that came after it.
        let old = full.rsplit_once('\t').expect("more than one field").0;
        let old = old.rsplit_once('\t').expect("more than one field").0;
        let old = old.rsplit_once('\t').expect("more than one field").0;
        let back = parse(old).expect("parses");
        assert_eq!(back.zoom, 1.45);
        assert_eq!(back.offset, -37.0);
        assert_eq!(back.zoom_y, 1.0);
        assert!(!back.centred);
        assert!(back.hooked_away);
    }
}
