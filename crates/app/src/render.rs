//! Drawing the line.
//!
//! Each card is painted once into a cached sprite — shadow, glass, thumbnail,
//! footer, clothespin — and every frame only places that sprite with a
//! rotation and a scale. Swaying costs nothing. Text is rasterised once per
//! (string, size, colour) and cached too.
//!
//! A card that is in the air is drawn differently: its window's snapshot is
//! stretched along the flight rectangle, so the user sees the real window
//! shrinking into the line rather than a generic card appearing.

use std::collections::HashMap;

use shaiwin_core::theme::{tag_color, tag_ink};
use shaiwin_core::{LineLayout, Rgb};
use shaiwin_platform_win::Rect as WRect;
use tiny_skia::{
    Color, FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder,
    Pixmap, PixmapPaint, Point, Shader, SpreadMode, Stroke, Transform,
};

use crate::line::{Card, Line};

pub struct SpriteKey {
    pub thumb_version: u64,
    pub tag_version: u64,
    pub scale_milli: u32,
    pub zoom_milli: u32,
    pub zoom_y_milli: u32,
}

impl PartialEq for SpriteKey {
    fn eq(&self, other: &Self) -> bool {
        self.thumb_version == other.thumb_version
            && self.tag_version == other.tag_version
            && self.scale_milli == other.scale_milli
            && self.zoom_milli == other.zoom_milli
            && self.zoom_y_milli == other.zoom_y_milli
    }
}

pub struct Sprite {
    pub pix: Pixmap,
    /// The clothespin's top centre, which is where the card pivots.
    pub pivot: (f32, f32),
    /// The card's rectangle inside the sprite (x, y, width, height).
    pub card: (f32, f32, f32, f32),
}

/// What one frame needs besides the cards.
pub struct Ui<'a> {
    pub hover: Option<u64>,
    /// Vertical offset of the whole line, in device pixels (0 = down).
    pub y_off: f32,
    /// Shown when the line is empty.
    pub hint: Option<&'a str>,
    /// A dragged window is near enough for the line to answer: the rope goes
    /// warm and the empty-line hint steps aside.
    pub hot: bool,
    /// Whether this layer draws the rope. When the card layer has sunk behind
    /// the user's work, the rope lives in its own always-on-top window instead,
    /// so drawing it here too would double the line.
    pub rope: bool,
    /// The pointer is on the hovered card's close button.
    pub close_hot: bool,
}

pub struct Frame {
    pub hits: Vec<(u64, WRect)>,
    /// Close buttons drawn this frame, in canvas coordinates. At most one: the
    /// button belongs to the card under the pointer.
    pub closes: Vec<(u64, WRect)>,
}

pub struct Painter {
    text: Option<crate::text::Text>,
    labels: HashMap<String, Pixmap>,
    sprites: HashMap<u64, (SpriteKey, Sprite)>,
}

impl Painter {
    pub fn new() -> Painter {
        Painter {
            text: crate::text::Text::new(),
            labels: HashMap::new(),
            sprites: HashMap::new(),
        }
    }

    pub fn forget(&mut self, id: u64) {
        self.sprites.remove(&id);
    }

    /// Text rendered once and reused. The key is the text, the size and the colour.
    fn label(&mut self, text: &str, px: f32, rgb: (u8, u8, u8)) -> Option<&Pixmap> {
        let key = format!("{text}|{px:.2}|{rgb:?}");
        if !self.labels.contains_key(&key) {
            let (rgba, width, height) = self.text.as_mut()?.render(text, px, rgb)?;
            let mut pix = Pixmap::new(width, height)?;
            pix.data_mut().copy_from_slice(&rgba);
            self.labels.insert(key.clone(), pix);
        }
        self.labels.get(&key)
    }

    fn label_bold(&mut self, text: &str, px: f32, rgb: (u8, u8, u8)) -> Option<&Pixmap> {
        let key = format!("b|{text}|{px:.2}|{rgb:?}");
        if !self.labels.contains_key(&key) {
            let (rgba, width, height) = self.text.as_mut()?.render_bold(text, px, rgb)?;
            let mut pix = Pixmap::new(width, height)?;
            pix.data_mut().copy_from_slice(&rgba);
            self.labels.insert(key.clone(), pix);
        }
        self.labels.get(&key)
    }

    fn text_width(&mut self, text: &str, px: f32) -> f32 {
        self.text
            .as_mut()
            .and_then(|t| t.width(text, px))
            .unwrap_or(text.chars().count() as f32 * px * 0.5)
    }

    fn text_width_bold(&mut self, text: &str, px: f32) -> f32 {
        self.text
            .as_mut()
            .and_then(|t| t.width_bold(text, px))
            .unwrap_or(text.chars().count() as f32 * px * 0.58)
    }

    /// Shortens `text` with an ellipsis until it fits `max_w`.
    fn elide(&mut self, text: &str, px: f32, max_w: f32) -> String {
        if self.text_width(text, px) <= max_w {
            return text.to_string();
        }
        self.elide_with(text, px, max_w, false)
    }

    fn elide_bold(&mut self, text: &str, px: f32, max_w: f32) -> String {
        if self.text_width_bold(text, px) <= max_w {
            return text.to_string();
        }
        self.elide_with(text, px, max_w, true)
    }

    fn elide_with(&mut self, text: &str, px: f32, max_w: f32, bold: bool) -> String {
        let ellipsis = "…";
        let measure = |this: &mut Self, s: &str| {
            if bold {
                this.text_width_bold(s, px)
            } else {
                this.text_width(s, px)
            }
        };
        let ellipsis_w = measure(self, ellipsis);
        let mut out = String::new();
        let mut width = 0.0;
        for ch in text.chars() {
            let advance = measure(self, ch.encode_utf8(&mut [0u8; 4]));
            if width + advance > max_w - ellipsis_w {
                break;
            }
            width += advance;
            out.push(ch);
        }
        out.push('…');
        out
    }

    fn sprite(&mut self, card: &Card, scale: f32) -> Option<&Sprite> {
        let key = SpriteKey {
            thumb_version: card.thumb_version,
            tag_version: card.tag_version,
            scale_milli: (scale * 1000.0) as u32,
            zoom_milli: (card.zoom * 1000.0) as u32,
            zoom_y_milli: (card.zoom_y * 1000.0) as u32,
        };
        let stale = match self.sprites.get(&card.id) {
            Some((cached, _)) => cached != &key,
            None => true,
        };
        if stale {
            let sprite = self.build_sprite(card, scale)?;
            self.sprites.insert(card.id, (key, sprite));
        }
        self.sprites.get(&card.id).map(|(_, s)| s)
    }

    /// Paints one card at device scale `s`.
    fn build_sprite(&mut self, card: &Card, s: f32) -> Option<Sprite> {
        let card_w = card.width(s);
        let card_h = card.height(s);
        let m = 22.0 * s;
        let pin_h = (crate::line::PIN_TOP - 22.0) * s;
        let pin_w = 9.0 * s;
        let radius = 14.0 * s;
        let footer = 38.0 * s;
        let sw = (card_w + m * 2.0).ceil() as u32;
        let sh = (m + pin_h + card_h + m).ceil() as u32;
        let cx = sw as f32 / 2.0;
        let x0 = (sw as f32 - card_w) / 2.0;
        let y0 = m + pin_h;
        let mut pix = Pixmap::new(sw, sh)?;

        // Shadow under the card and a tighter one under the pin.
        let mut shadow = Pixmap::new(sw, sh)?;
        fill_rrect(
            &mut shadow,
            x0,
            y0 + 4.0 * s,
            card_w,
            card_h,
            radius,
            solid(0, 0, 0, 0.20),
        );
        box_blur(&mut shadow, (5.0 * s).round().max(1.0) as usize);
        let mut pin_shadow = Pixmap::new(sw, sh)?;
        fill_rrect(
            &mut pin_shadow,
            cx - pin_w / 2.0,
            m + 2.0 * s,
            pin_w,
            pin_h,
            3.5 * s,
            solid(0, 0, 0, 0.28),
        );
        box_blur(&mut pin_shadow, (2.0 * s).round().max(1.0) as usize);
        draw_layer(&mut pix, &shadow, 0, 0);
        draw_layer(&mut pix, &pin_shadow, 0, 0);

        // The card pane. Windows cannot blur what is behind a layered window,
        // so this is a clean translucent pane rather than frosted glass.
        fill_rrect(
            &mut pix,
            x0,
            y0,
            card_w,
            card_h,
            radius,
            solid(252, 252, 254, 0.93),
        );
        stroke_rrect(
            &mut pix,
            x0 + 0.5 * s,
            y0 + 0.5 * s,
            card_w - s,
            card_h - s,
            radius - 0.5 * s,
            0.75 * s,
            solid(255, 255, 255, 0.9),
        );

        // Thumbnail.
        let thumb_y = y0 + 5.0 * s;
        let thumb_h = card_h - footer - 10.0 * s;
        let thumb_x = x0 + 5.0 * s;
        let thumb_w = card_w - 10.0 * s;
        let inner_radius = (radius - 5.0 * s).max(2.0);
        match card.thumb.as_ref() {
            Some(thumb) => {
                let mut mask = Mask::new(sw, sh)?;
                if let Some(path) = rrect_path(thumb_x, thumb_y, thumb_w, thumb_h, inner_radius) {
                    mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
                }
                let k = (thumb_w / thumb.width() as f32).max(thumb_h / thumb.height() as f32);
                let dw = thumb.width() as f32 * k;
                let dh = thumb.height() as f32 * k;
                let transform = Transform::from_translate(
                    thumb_x + (thumb_w - dw) / 2.0,
                    thumb_y + (thumb_h - dh) / 2.0,
                )
                .pre_scale(k, k);
                let paint = PixmapPaint {
                    quality: FilterQuality::Bilinear,
                    ..Default::default()
                };
                pix.draw_pixmap(0, 0, thumb.as_ref(), &paint, transform, Some(&mask));
            }
            None => {
                // No picture yet: a tinted pane with the icon in the middle, so
                // the card still says which application it belongs to.
                fill_rrect(
                    &mut pix,
                    thumb_x,
                    thumb_y,
                    thumb_w,
                    thumb_h,
                    inner_radius,
                    accent_gradient(card.accent, thumb_y, thumb_y + thumb_h),
                );
                if let Some(icon) = card.icon.as_ref() {
                    let size = 40.0 * s;
                    let paint = PixmapPaint {
                        quality: FilterQuality::Bilinear,
                        ..Default::default()
                    };
                    let scale = size / icon.width() as f32;
                    let at = Transform::from_translate(
                        thumb_x + (thumb_w - size) / 2.0,
                        thumb_y + (thumb_h - size) / 2.0,
                    )
                    .pre_scale(scale, scale);
                    pix.draw_pixmap(0, 0, icon.as_ref(), &paint, at, None);
                }
            }
        }

        // A tag is the fastest way to find one window on a loaded line, so it
        // gets a full-width label across the top of the thumbnail. The colour
        // comes from the words, not from the app: two windows of the same
        // application can still be told apart at a glance.
        if !card.tag.is_empty() {
            let base = tag_color(&card.tag);
            let ink = tag_ink(base);
            // The label is how a window is found again on a full line, so it
            // is the largest thing on the card after the picture itself: 13 px
            // of semibold on a 26 px band, not a caption.
            let band_h = 26.0 * s;
            let band_x = thumb_x + 4.0 * s;
            let band_y = thumb_y + 4.0 * s;
            let band_w = thumb_w - 8.0 * s;
            fill_rrect(
                &mut pix,
                band_x,
                band_y + 1.5 * s,
                band_w,
                band_h,
                6.0 * s,
                solid(0, 0, 0, 0.24),
            );
            fill_rrect(
                &mut pix,
                band_x,
                band_y,
                band_w,
                band_h,
                6.0 * s,
                tag_gradient(base, band_y, band_y + band_h),
            );
            stroke_rrect(
                &mut pix,
                band_x + 0.5 * s,
                band_y + 0.5 * s,
                band_w - s,
                band_h - s,
                5.5 * s,
                0.75 * s,
                solid(255, 255, 255, 0.30),
            );
            // The punched hole is what makes it read as a luggage tag rather
            // than a toolbar laid over the picture.
            let hole = 3.5 * s;
            fill_rrect(
                &mut pix,
                band_x + 7.0 * s,
                band_y + (band_h - hole) / 2.0,
                hole,
                hole,
                hole / 2.0,
                solid(255, 255, 255, 0.82),
            );
            let text_x = band_x + 16.0 * s;
            let room = (band_x + band_w - text_x - 8.0 * s).max(20.0);
            let tag = self.elide_bold(&card.tag, 13.0 * s, room);
            if let Some(label) = self.label_bold(&tag, 13.0 * s, ink.as_tuple()) {
                pix.draw_pixmap(
                    text_x.round() as i32,
                    (band_y + (band_h - label.height() as f32) / 2.0).round() as i32,
                    label.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        }

        // Footer: application icon, window title, application name. Both lines
        // are measured first and centred as a block, so a long title or a
        // missing app name can never push text past the card's bottom edge.
        let footer_top = y0 + card_h - footer;
        let icon_size = 16.0 * s;
        let pad = 9.0 * s;
        if let Some(icon) = card.icon.as_ref() {
            let paint = PixmapPaint {
                quality: FilterQuality::Bilinear,
                ..Default::default()
            };
            let scale = icon_size / icon.width() as f32;
            let at = Transform::from_translate(x0 + pad, footer_top + (footer - icon_size) / 2.0)
                .pre_scale(scale, scale);
            pix.draw_pixmap(0, 0, icon.as_ref(), &paint, at, None);
        }
        let text_x = x0 + pad + icon_size + 6.0 * s;
        let text_w = (x0 + card_w - pad - text_x).max(10.0);
        let title = self.elide(&card.title, 11.5 * s, text_w);
        let app = self.elide(&card.app, 9.0 * s, text_w);
        let app_rgb = card.accent.darken(0.25).as_tuple();
        let title_h = self
            .label(&title, 11.5 * s, (34, 34, 42))
            .map_or(0.0, |p| p.height() as f32);
        let app_h = self
            .label(&app, 9.0 * s, app_rgb)
            .map_or(0.0, |p| p.height() as f32);
        let gap = 1.0 * s;
        let block = title_h + gap + app_h;
        let mut ty = footer_top + ((footer - block) / 2.0).max(1.0);
        if let Some(label) = self.label(&title, 11.5 * s, (34, 34, 42)) {
            pix.draw_pixmap(
                text_x as i32,
                ty as i32,
                label.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            ty += title_h + gap;
        }
        if let Some(label) = self.label(&app, 9.0 * s, app_rgb) {
            pix.draw_pixmap(
                text_x as i32,
                ty as i32,
                label.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }

        // The clothespin, tinted with the application's accent: two windows of
        // the same app get the same clip, which is half of "who belongs to whom".
        let pin_x = cx - pin_w / 2.0;
        fill_rrect(
            &mut pix,
            pin_x,
            m,
            pin_w,
            pin_h + 6.0 * s,
            3.5 * s,
            accent_gradient(card.accent, m, m + pin_h),
        );
        stroke_rrect(
            &mut pix,
            pin_x,
            m,
            pin_w,
            pin_h + 6.0 * s,
            3.5 * s,
            0.6 * s,
            solid(255, 255, 255, 0.55),
        );
        fill_rrect(
            &mut pix,
            cx - 2.5 * s,
            m + 8.0 * s,
            5.0 * s,
            1.4 * s,
            0.7 * s,
            solid(0, 0, 0, 0.28),
        );

        // Debug aid: SHAIWIN_DUMP_SPRITES=<dir> writes each card sprite once, at
        // 1:1, which is far easier to inspect than the whole translucent frame.
        if let Some(dir) = std::env::var_os("SHAIWIN_DUMP_SPRITES") {
            let path = std::path::PathBuf::from(dir).join(format!("sprite-{}.png", card.id));
            let _ = pix.save_png(&path);
        }

        Some(Sprite {
            pix,
            pivot: (cx, m),
            card: (x0, y0, card_w, card_h),
        })
    }

    /// Draws the rope, the cards and any hint into `canvas`, which the caller
    /// sizes and owns.
    ///
    /// Reusing one canvas matters on a weak machine: a fresh full-width pixmap
    /// per frame means allocating and first-touching megabytes sixty times a
    /// second, and that cost grows with the screen, not with the line.
    pub fn compose_into(
        &mut self,
        canvas: &mut Pixmap,
        line: &Line,
        layout: &LineLayout,
        ui: &Ui,
    ) -> Frame {
        canvas.fill(Color::TRANSPARENT);
        let s = layout.scale;
        if ui.rope {
            draw_rope(canvas, layout, ui.y_off, ui.hot);
        }
        // A full line shrinks every card by the same factor; the sprites are
        // built at the user's zoom, so this is just a draw-time scale.
        let fit = line.fit(layout);

        let mut hits = Vec::new();
        let mut closes = Vec::new();
        // Draw the card being dragged last, so it stays above its neighbours.
        let mut ordered: Vec<&Card> = line.cards.iter().filter(|c| !c.away).collect();
        if let Some(drag) = line.dragging {
            if let Some(pos) = ordered.iter().position(|c| c.id == drag) {
                let card = ordered.remove(pos);
                ordered.push(card);
            }
        }
        for card in ordered {
            // A leaving card has already had its window put back; its place on
            // the line is empty until the card is removed this same frame.
            if card.leaving {
                continue;
            }
            let sprite = match self.sprite(card, s) {
                Some(s) => s,
                None => continue,
            };

            let pivot_x = card.x.x;
            let pivot_y = layout.rope_y(pivot_x) + ui.y_off;
            let angle = card.sway.x * 0.9;
            // The hover pop and the packed fit are both draw-time scales.
            let k = card.scale * fit;
            let transform = card_transform(sprite, pivot_x, pivot_y, angle, k);
            canvas.draw_pixmap(
                0,
                0,
                sprite.pix.as_ref(),
                &PixmapPaint::default(),
                transform,
                None,
            );

            if ui.hover == Some(card.id) {
                draw_hover_ring(canvas, sprite, s, transform);
                // The card's own way off the line, shown only while the pointer
                // is on the card: the top-right corner is where the thumbnail's
                // own content is, and a button parked there for every card would
                // sit on top of it for a card the user is only looking at.
                if let Some(rect) = draw_close(canvas, sprite, s, transform, k, ui.close_hot) {
                    closes.push((card.id, rect));
                }
            }

            // Clickable from the first frame it hangs: a card you can see is a
            // card you can click. The rectangle starts at the pin, not at the
            // card's top edge: grabbing a card by its clothespin is what people
            // do, and that part used to be click-through.
            let (x0, y0, cw, ch) = sprite.card;
            let left = pivot_x + (x0 - sprite.pivot.0) * k;
            let top = pivot_y;
            hits.push((
                card.id,
                WRect {
                    left: left.round() as i32,
                    top: top.round() as i32,
                    right: (left + cw * k).round() as i32,
                    bottom: (pivot_y + (y0 - sprite.pivot.1) * k + ch * k).round() as i32,
                },
            ));
        }

        // While a window is being dragged over the line the rope and the empty
        // slot say "let go here" — a plate of text over the top of the screen
        // covered the very thing it was describing.
        if let Some(hint) = ui.hint.filter(|_| !ui.hot) {
            let center_x = layout.width / 2.0;
            let center_y = layout.rope_y(center_x) + ui.y_off + 36.0 * s;
            if let Some(label) = self.label(hint, 12.0 * s, (238, 241, 248)) {
                draw_hint(canvas, center_x, center_y, label, s, false);
            }
        }

        Frame { hits, closes }
    }

    /// Draws only the rope, for the always-on-top companion window that keeps
    /// it visible after the cards sink behind the user's work.
    pub fn compose_rope_into(
        &mut self,
        canvas: &mut Pixmap,
        layout: &LineLayout,
        y_off: f32,
        hot: bool,
    ) {
        canvas.fill(Color::TRANSPARENT);
        draw_rope(canvas, layout, y_off, hot);
    }

    /// The way into a non-resident line: a small pill at the top of the screen
    /// that the user can click. Hover only shows this, never the line itself.
    /// It is the only thing drawn, so the rest of the strip stays click-through.
    pub fn compose_peek_into(
        &mut self,
        canvas: &mut Pixmap,
        layout: &LineLayout,
        hot: bool,
        cards: usize,
    ) -> Frame {
        let s = layout.scale;
        canvas.fill(Color::TRANSPARENT);
        // The count is the reason a tucked line is not a lost line: after the
        // auto-collapse the pill is the only proof the cards are still hanging.
        let text = if cards == 0 {
            crate::i18n::t("peek_open").to_string()
        } else {
            format!("{} · {}", crate::i18n::t("peek_open"), cards)
        };
        let Some(label) = self.label(&text, 13.0 * s, (240, 243, 250)) else {
            return Frame {
                hits: Vec::new(),
                closes: Vec::new(),
            };
        };
        let cx = layout.width / 2.0;
        let cy = 44.0 * s;
        draw_hint(canvas, cx, cy, label, s, hot);
        let w = label.width() as f32 + 28.0 * s;
        let h = label.height() as f32 + 16.0 * s;
        let rect = WRect {
            left: (cx - w / 2.0).round() as i32,
            top: (cy - h / 2.0).round() as i32,
            right: (cx + w / 2.0).round() as i32,
            bottom: (cy + h / 2.0).round() as i32,
        };
        Frame {
            hits: vec![(crate::app::PEEK_ID, rect)],
            closes: Vec::new(),
        }
    }
}

fn card_transform(sprite: &Sprite, x: f32, y: f32, angle_deg: f32, scale: f32) -> Transform {
    Transform::from_translate(-sprite.pivot.0, -sprite.pivot.1)
        .post_rotate(angle_deg)
        .post_scale(scale, scale)
        .post_translate(x, y)
}

/// The close button's diameter and its inset from the card body's top-right
/// corner, in logical pixels.
const CLOSE_SIZE: f32 = 22.0;
const CLOSE_INSET: f32 = 9.0;

/// Where the close button sits on a card, in sprite coordinates: the disc's
/// left, top and diameter.
///
/// It never grows past a fifth of the card. A card can be zoomed down to well
/// under its own width, and a fixed-size disc on a small card would cover the
/// thumbnail it is meant to sit on top of.
fn close_button(sprite: &Sprite, s: f32) -> (f32, f32, f32) {
    let (x0, y0, cw, _) = sprite.card;
    let d = (CLOSE_SIZE * s).min(cw * 0.2);
    let inset = CLOSE_INSET * s;
    (x0 + cw - inset - d, y0 + inset, d)
}

/// Draws the close button and answers with the rectangle it occupies on the
/// canvas, which is what a click is matched against.
///
/// The cross is two strokes rather than a glyph: it stays crisp at every scale
/// and rotation without re-rasterising text, and the button is drawn per frame
/// only for the one card under the pointer.
fn draw_close(
    canvas: &mut Pixmap,
    sprite: &Sprite,
    s: f32,
    transform: Transform,
    k: f32,
    hot: bool,
) -> Option<WRect> {
    let (bx, by, d) = close_button(sprite, s);
    let r = d / 2.0;
    let cx = bx + r;
    let cy = by + r;
    let disc = rrect_path(bx, by, d, d, r)?;
    canvas.fill_path(
        &disc,
        &solid(12, 14, 18, if hot { 0.84 } else { 0.52 }),
        FillRule::Winding,
        transform,
        None,
    );
    stroke_path(
        canvas,
        &disc,
        solid(255, 255, 255, if hot { 0.92 } else { 0.55 }),
        1.1 * s,
        transform,
    );
    let arm = d * 0.23;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - arm, cy - arm);
    pb.line_to(cx + arm, cy + arm);
    pb.move_to(cx + arm, cy - arm);
    pb.line_to(cx - arm, cy + arm);
    if let Some(cross) = pb.finish() {
        stroke_path(
            canvas,
            &cross,
            solid(255, 255, 255, if hot { 1.0 } else { 0.84 }),
            1.6 * s,
            transform,
        );
    }
    // The target. The card sways by a fraction of a degree, so the disc's own
    // square is a faithful outline of where the button landed; what has to be
    // right is the position, which is why it goes through the transform.
    let mut c = Point::from_xy(cx, cy);
    transform.map_point(&mut c);
    let half = (r * k).max(5.0 * s);
    Some(WRect {
        left: (c.x - half).round() as i32,
        top: (c.y - half).round() as i32,
        right: (c.x + half).round() as i32,
        bottom: (c.y + half).round() as i32,
    })
}

fn draw_rope(canvas: &mut Pixmap, layout: &LineLayout, y_off: f32, hot: bool) {
    let s = layout.scale;
    let w = layout.width;
    // The rope and the pins must be on the same curve: the cards attach to the
    // rope exactly where `rope_y` puts them.
    let top = layout.rope_y(0.0) + y_off;
    let sag = layout.sag();
    let mut pb = PathBuilder::new();
    pb.move_to(-20.0 * s, top);
    pb.quad_to(w / 2.0, top + 2.0 * sag, w + 20.0 * s, top);
    let Some(path) = pb.finish() else { return };
    if hot {
        // The drag's answer: a warm glow around the rope where the window would
        // be clipped on. Warm, not white: a wide white band at the top of the
        // screen reads as a system frame, which is the one thing this must not
        // look like.
        stroke_path(
            canvas,
            &path,
            rope_paint(w, (255, 186, 84), 0.55),
            7.0 * s,
            Transform::identity(),
        );
    }
    stroke_path(
        canvas,
        &path,
        rope_paint(w, (0, 0, 0), 0.22),
        2.2 * s,
        Transform::from_translate(0.0, s),
    );
    let core = if hot {
        (246, 198, 120)
    } else {
        (132, 132, 140)
    };
    stroke_path(
        canvas,
        &path,
        rope_paint(w, core, 1.0),
        1.15 * s,
        Transform::identity(),
    );
    stroke_path(
        canvas,
        &path,
        rope_paint(w, (255, 255, 255), 0.5),
        0.4 * s,
        Transform::from_translate(0.0, -0.35 * s),
    );
}

fn rope_paint(width: f32, rgb: (u8, u8, u8), alpha: f32) -> Paint<'static> {
    let stops = vec![
        GradientStop::new(0.0, Color::from_rgba8(rgb.0, rgb.1, rgb.2, 0)),
        GradientStop::new(
            0.06,
            Color::from_rgba8(rgb.0, rgb.1, rgb.2, (alpha * 255.0) as u8),
        ),
        GradientStop::new(
            0.94,
            Color::from_rgba8(rgb.0, rgb.1, rgb.2, (alpha * 255.0) as u8),
        ),
        GradientStop::new(1.0, Color::from_rgba8(rgb.0, rgb.1, rgb.2, 0)),
    ];
    let shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(width, 0.0),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(Color::TRANSPARENT));
    Paint {
        shader,
        anti_alias: true,
        ..Default::default()
    }
}

fn tag_gradient(base: Rgb, y0: f32, y1: f32) -> Paint<'static> {
    let light = base.lighten(0.12);
    let dark = base.darken(0.14);
    let stops = vec![
        GradientStop::new(0.0, Color::from_rgba8(light.0, light.1, light.2, 255)),
        GradientStop::new(1.0, Color::from_rgba8(dark.0, dark.1, dark.2, 255)),
    ];
    let shader = LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(Color::TRANSPARENT));
    Paint {
        shader,
        anti_alias: true,
        ..Default::default()
    }
}

fn accent_gradient(accent: Rgb, y0: f32, y1: f32) -> Paint<'static> {
    let light = accent.lighten(0.28);
    let dark = accent.darken(0.18);
    let stops = vec![
        GradientStop::new(0.0, Color::from_rgba8(light.0, light.1, light.2, 255)),
        GradientStop::new(1.0, Color::from_rgba8(dark.0, dark.1, dark.2, 255)),
    ];
    let shader = LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(Color::TRANSPARENT));
    Paint {
        shader,
        anti_alias: true,
        ..Default::default()
    }
}

fn draw_hover_ring(canvas: &mut Pixmap, sprite: &Sprite, s: f32, transform: Transform) {
    let (x0, y0, cw, ch) = sprite.card;
    if let Some(path) = rrect_path(
        x0 - 1.5 * s,
        y0 - 1.5 * s,
        cw + 3.0 * s,
        ch + 3.0 * s,
        15.0 * s,
    ) {
        stroke_path(
            canvas,
            &path,
            solid(255, 255, 255, 0.85),
            1.6 * s,
            transform,
        );
    }
}

/// The pill the line shows when it has something to say: the "let go" prompt
/// while a window is over the band, and the "show the line" peek at the top of
/// the screen.
///
/// Edge's hover strip is the reference: a see-through tint, not a white plate
/// with a hard edge. Windows will not blur what sits behind a layered window, so
/// the tint is plain transparency, and legibility comes from a dark wash under
/// light text with one hairline of light. That pair reads on a white page and on
/// a black one, which an opaque white pill on a light wallpaper does not.
fn draw_hint(canvas: &mut Pixmap, cx: f32, cy: f32, label: &Pixmap, s: f32, hot: bool) {
    let pad_x = 15.0 * s;
    let pad_y = 8.0 * s;
    let w = label.width() as f32 + pad_x * 2.0;
    let h = label.height() as f32 + pad_y * 2.0;
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;
    let r = h / 2.0;

    let wash = if hot { 0.78 } else { 0.62 };
    let stops = vec![
        GradientStop::new(0.0, Color::from_rgba8(40, 45, 58, (wash * 255.0) as u8)),
        GradientStop::new(
            1.0,
            Color::from_rgba8(18, 21, 29, (wash * 0.8 * 255.0) as u8),
        ),
    ];
    let shader = LinearGradient::new(
        Point::from_xy(x, y),
        Point::from_xy(x, y + h),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let (Some(path), Some(shader)) = (rrect_path(x, y, w, h, r), shader) {
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        canvas.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    // One light line along the top, where the pill meets the screen edge; the
    // other sides are defined by the tint alone. When the pointer is on it the
    // whole rim lights up, which is the only solid outline it ever gets.
    if !hot {
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y + 0.5 * s);
        pb.line_to(x + w - r, y + 0.5 * s);
        if let Some(path) = pb.finish() {
            stroke_path(
                canvas,
                &path,
                solid(255, 255, 255, 0.26),
                1.0 * s,
                Transform::identity(),
            );
        }
    }
    if let Some(path) = rrect_path(x + 0.5 * s, y + 0.5 * s, w - s, h - s, r - 0.5 * s) {
        let rim = if hot {
            solid(0xEF, 0x9C, 0x3B, 0.95)
        } else {
            solid(255, 255, 255, 0.10)
        };
        stroke_path(
            canvas,
            &path,
            rim,
            if hot { 1.4 * s } else { 1.0 * s },
            Transform::identity(),
        );
    }

    let paint = PixmapPaint::default();
    canvas.draw_pixmap(
        (cx - label.width() as f32 / 2.0).round() as i32,
        (cy - label.height() as f32 / 2.0).round() as i32,
        label.as_ref(),
        &paint,
        Transform::identity(),
        None,
    );
}

// MARK: Shapes and paints

fn solid(r: u8, g: u8, b: u8, a: f32) -> Paint<'static> {
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    paint.set_color(Color::from_rgba8(
        r,
        g,
        b,
        (a.clamp(0.0, 1.0) * 255.0).round() as u8,
    ));
    paint
}

fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.clamp(0.0, w.min(h) / 2.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_rrect(pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, paint: Paint<'static>) {
    if let Some(path) = rrect_path(x, y, w, h, r) {
        pix.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_rrect(
    pix: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    width: f32,
    paint: Paint<'static>,
) {
    if let Some(path) = rrect_path(x, y, w, h, r) {
        stroke_path(pix, &path, paint, width, Transform::identity());
    }
}

fn stroke_path(
    canvas: &mut Pixmap,
    path: &Path,
    paint: Paint<'static>,
    width: f32,
    transform: Transform,
) {
    let stroke = Stroke {
        width,
        ..Default::default()
    };
    canvas.stroke_path(path, &paint, &stroke, transform, None);
}

fn draw_layer(dst: &mut Pixmap, layer: &Pixmap, x: i32, y: i32) {
    dst.draw_pixmap(
        x,
        y,
        layer.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// Three passes of a box blur approximate a Gaussian, which is what a shadow
/// is. Works straight on the premultiplied RGBA bytes.
fn box_blur(pix: &mut Pixmap, radius: usize) {
    if radius == 0 {
        return;
    }
    let (w, h) = (pix.width() as usize, pix.height() as usize);
    let mut tmp = vec![0u8; w * h * 4];
    let data = pix.data_mut();
    for _ in 0..2 {
        blur_axis(data, &mut tmp, w, h, radius, true);
        blur_axis(&tmp, data, w, h, radius, false);
    }
}

fn blur_axis(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let index = |line: usize, p: usize| -> usize {
        if horizontal {
            (line * w + p) * 4
        } else {
            (p * w + line) * 4
        }
    };
    let window = (2 * r + 1) as u32;
    for line in 0..lines {
        for c in 0..4 {
            let at = |p: isize| -> u32 {
                if p < 0 || p >= len as isize {
                    0
                } else {
                    src[index(line, p as usize) + c] as u32
                }
            };
            let mut sum: u32 = (0..=r as isize).map(at).sum();
            for p in 0..len {
                dst[index(line, p) + c] = (sum / window) as u8;
                sum += at((p + r + 1) as isize);
                sum -= at(p as isize - r as isize);
            }
        }
    }
}
