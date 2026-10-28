//! The line: which cards hang on it, where they sit, and how they move.
//!
//! The physics is a damped spring per value, stepped in fixed substeps by
//! `shaiwin-core`. A card that is in the air is driven by an `ArcFlight`
//! instead, so hanging a window and taking it down are the same code path
//! played in opposite directions.

use std::time::{Duration, Instant};

use shaiwin_core::layout::EDGE_PAD;
use shaiwin_core::{ArcFlight, LineLayout, Rgb, Spring, anim::lerp_rect};
use shaiwin_platform_win::Rect as WRect;
use shaiwin_platform_win::park::{Parked, Placement};
use tiny_skia::Pixmap;

/// Where a card's rectangle sits relative to the hanging point, in logical px.
pub const CARD_W: f32 = 168.0;
pub const CARD_H: f32 = 118.0;
/// How much of the clothespin overlaps the card.
pub const PIN_OVERLAP: f32 = 12.0;
const GUST_MIN: f32 = 9.0;
const GUST_MAX: f32 = 18.0;

/// How long the snapshot takes to travel. Kept short on purpose: the flight is
/// a pointer to where the window went, not a show.
const HANG_FLIGHT: f32 = 0.30;
const TAKE_FLIGHT: f32 = 0.22;
/// The card slides to its slot this quickly after it lands or is released.
const SLOT_RESPONSE: f32 = 0.32;
const SLOT_DAMPING: f32 = 0.9;
/// How much bigger the card under the pointer gets.
const HOVER_SCALE: f32 = 1.045;
/// The clothespin swing. A gust has to stop: an under-damped sway keeps ringing
/// for seconds, and a ringing card keeps the whole strip compositing at 60 Hz.
/// These numbers let a 2.6 degree swing die out inside two seconds.
const SWAY_RESPONSE: f32 = 1.0;
const SWAY_DAMPING: f32 = 0.62;

/// A sway spring hanging at `angle`, ready to be retargeted by a gust.
fn tilt(angle: f32) -> Spring {
    Spring::responsive(angle, SWAY_RESPONSE, SWAY_DAMPING)
}

/// How far a card may be stretched or squeezed sideways. Wide enough to read a
/// title, narrow enough that the line holds a dozen windows.
pub const ZOOM_MIN: f32 = 0.55;
pub const ZOOM_MAX: f32 = 1.8;
/// The same, for the card's height. The top edge stays on the pin; the card
/// grows downwards, so a taller card never hides its own clothespin.
pub const ZOOM_Y_MIN: f32 = 0.6;
pub const ZOOM_Y_MAX: f32 = 1.8;
/// How close to the side of a card the pointer has to be before it is resizing
/// it rather than clicking it, in logical pixels.
pub const EDGE_REACH: f32 = 9.0;
/// The corners resize both ways at once, and a corner is a smaller target than
/// an edge: it gets a square zone of its own so a diagonal grab does not have to
/// be pixel-perfect.
pub const CORNER_REACH: f32 = 16.0;
/// Distance from the pin's top to the top of the card body, in logical pixels.
/// The renderer and the app's panel bounds both need it.
pub const PIN_TOP: f32 = 44.0;

/// Which side of a card is being grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Bottom,
    BottomLeft,
    BottomRight,
}

impl Edge {
    /// Whether dragging this edge changes the card's width.
    pub fn resizes_width(self) -> bool {
        matches!(self, Edge::Left | Edge::Right | Edge::BottomLeft | Edge::BottomRight)
    }

    /// Whether dragging this edge changes the card's height.
    pub fn resizes_height(self) -> bool {
        matches!(self, Edge::Bottom | Edge::BottomLeft | Edge::BottomRight)
    }
}

/// The side of `rect` `point` is within reach of, if any. Outside the card, or
/// on its face, this is `None` and the press is a click or a drag.
pub fn edge_at(rect: &WRect, point: (i32, i32), s: f32) -> Option<Edge> {
    let inside = point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom;
    if !inside {
        return None;
    }
    let reach = (EDGE_REACH * s) as i32;
    let corner = (CORNER_REACH * s) as i32;
    let from_bottom = rect.bottom - 1 - point.1;
    let from_left = point.0 - rect.left;
    let from_right = rect.right - 1 - point.0;
    // Corners first: near the bottom either side, a diagonal drag should win
    // over the plain bottom edge, otherwise the corner zone would be pure
    // bottom and a diagonal grab could never start.
    if from_bottom < corner {
        if from_left < corner {
            return Some(Edge::BottomLeft);
        }
        if from_right < corner {
            return Some(Edge::BottomRight);
        }
    }
    if from_bottom < reach {
        return Some(Edge::Bottom);
    }
    if from_left < reach {
        Some(Edge::Left)
    } else if from_right < reach {
        Some(Edge::Right)
    } else {
        None
    }
}

/// The zoom a card ends up with when its edge is dragged to `x`. The centre is
/// the one captured when the drag started: the pin hangs from it, so the card
/// grows outwards from where it hangs instead of sliding along the rope.
pub fn zoom_from_edge(x: f32, centre: f32, s: f32) -> f32 {
    let width = (x - centre).abs() * 2.0;
    (width / (CARD_W * s.max(0.01))).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// The vertical zoom when the bottom edge is dragged to `y`. `top` is where the
/// card's body starts and `unit` is how many device pixels one zoom unit is;
/// both are captured when the drag starts, so the top edge never wanders.
pub fn zoom_y_from_bottom(y: f32, top: f32, unit: f32) -> f32 {
    ((y - top) / unit.max(1.0)).clamp(ZOOM_Y_MIN, ZOOM_Y_MAX)
}


/// How close a dropped card may come to its neighbour, in logical pixels.
const PACK_GAP: f32 = 8.0;

/// The placements of every hanging card, in line order. The line packs by each
/// card's width, then every card keeps the sideways offset the user dragged it
/// to. A left-to-right pass keeps neighbours from overlapping, and the whole
/// line slides back if an offset pushed it past the edge.
fn placed(layout: &LineLayout, widths: &[f32], offsets: &[f32]) -> Vec<f32> {
    let n = widths.len();
    if n == 0 {
        return Vec::new();
    }
    let s = layout.scale;
    let fit = layout.pack_fit(widths);
    let pad = EDGE_PAD * s;
    let gap = PACK_GAP * s;
    let base = layout.pack_x(widths);
    let mut xs: Vec<f32> = base
        .iter()
        .zip(offsets)
        .map(|(x, offset)| x + offset * s)
        .collect();
    for (x, w) in xs.iter_mut().zip(widths) {
        let half = w * fit / 2.0;
        *x = x.clamp(pad + half, layout.width - pad - half);
    }
    for i in 1..n {
        let min = xs[i - 1] + (widths[i - 1] + widths[i]) * fit / 2.0 + gap;
        if xs[i] < min {
            xs[i] = min;
        }
    }
    let overflow = xs[n - 1] + widths[n - 1] * fit / 2.0 - (layout.width - pad);
    if overflow > 0.0 {
        let room = (xs[0] - widths[0] * fit / 2.0 - pad).max(0.0);
        let shift = overflow.min(room);
        for x in &mut xs {
            *x -= shift;
        }
    }
    for (x, w) in xs.iter_mut().zip(widths) {
        let half = w * fit / 2.0;
        *x = x.clamp(pad + half, layout.width - pad - half);
    }
    xs
}

pub struct Card {
    pub id: u64,
    pub hwnd: isize,
    pub pid: u32,
    pub process_start: u64,
    pub title: String,
    pub app: String,
    pub exe: String,
    pub accent: Rgb,
    pub placement: Placement,
    pub parked: Parked,
    /// Where the window was on screen when it was hung up.
    pub window_rect: WRect,
    pub thumb: Option<Pixmap>,
    /// Bumped whenever `thumb` changes, so the sprite cache knows to rebuild.
    pub thumb_version: u64,
    pub icon: Option<Pixmap>,
    /// How wide this card is, relative to a normal one. The user sets it by
    /// dragging the card's side edge; a squeezed line is the reason it exists.
    pub zoom: f32,
    /// How tall this card is, relative to a normal one. Set by dragging the
    /// bottom edge; the top stays on the pin.
    pub zoom_y: f32,
    /// How far the card was dragged from its packed place, in logical pixels.
    /// The line still packs and reorders; this is what lets a card stay exactly
    /// where the user dropped it instead of snapping back to its slot.
    pub offset: f32,

    pub x: Spring,
    /// Sway, in degrees.
    pub sway: Spring,
    /// Hover/press feedback, eased rather than sprung.
    pub scale: f32,
    /// What `scale` is easing towards. Rest is this, not 1.0: a card the pointer
    /// is resting on is enlarged, and it is still at rest there.
    pub scale_goal: f32,

    /// In the air: from a window-sized rectangle to its slot, or the reverse.
    pub flight: Option<ArcFlight>,
    /// Screen rectangles the flight travels between.
    pub ghost_from: WRect,
    pub ghost_to: WRect,
    /// Bank angle at the start and end of the flight, in degrees.
    pub ghost_angle: (f32, f32),
    /// Where the line should reopen on the next launch, if this card is the
    /// reason the line moved to another monitor.
    pub anchor: (i32, i32),
    /// True once the window has been put back and the card is on its way out.
    pub leaving: bool,
    /// The window has been put back on the desktop but is still hooked: the
    /// card is hidden, and minimizing the window will hang it up again.
    pub away: bool,
    /// When the window last came down. Used to give the user a moment before a
    /// minimize counts as "put it back".
    pub away_at: Option<Instant>,
    /// The take-down was a request to stop managing this window; remove the
    /// card instead of keeping the hook.
    pub unhook: bool,
}

impl Card {
    pub fn flying(&self) -> bool {
        self.flight.is_some()
    }

    /// The card's width in device pixels. The strip packs by width; height is
    /// the user's business and does not re-pack the line.
    pub fn width(&self, s: f32) -> f32 {
        CARD_W * s * self.zoom
    }

    /// The card's height in device pixels, pin excluded.
    pub fn height(&self, s: f32) -> f32 {
        CARD_H * s * self.zoom_y
    }

    /// The card's rectangle on screen at rest, given its slot. `fit` is the
    /// factor the packed line had to shrink every card by to make them fit.
    pub fn rest_rect(&self, slot_x: f32, rope_y: f32, s: f32, fit: f32) -> WRect {
        let w = self.width(s) * fit;
        let h = self.height(s) * fit;
        let top = rope_y + PIN_OVERLAP * s;
        WRect {
            left: (slot_x - w / 2.0).round() as i32,
            top: top.round() as i32,
            right: (slot_x + w / 2.0).round() as i32,
            bottom: (top + h).round() as i32,
        }
    }
}

pub struct Line {
    pub cards: Vec<Card>,
    /// The card the pointer is dragging along the line, if any.
    pub dragging: Option<u64>,
    next_id: u64,
    rng: u64,
    next_gust: Instant,
    gust_back: Option<Instant>,
}

impl Line {
    pub fn new() -> Line {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let mut line = Line {
            cards: Vec::new(),
            dragging: None,
            next_id: 1,
            rng: seed | 1,
            next_gust: Instant::now(),
            gust_back: None,
        };
        line.next_gust = Instant::now() + Duration::from_secs_f32(line.random(7.0, 16.0));
        line
    }

    fn random(&mut self, lo: f32, hi: f32) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let unit = (self.rng >> 11) as f32 / (1u64 << 53) as f32;
        lo + (hi - lo) * unit
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn live_count(&self) -> usize {
        self.cards.iter().filter(|c| !c.leaving && !c.away).count()
    }

    /// The card's place among the cards actually hanging, if it is one of them.
    pub fn live_position(&self, id: u64) -> Option<usize> {
        self.cards.iter().filter(|c| !c.leaving && !c.away).position(|c| c.id == id)
    }

    /// Cards whose windows are still hooked, including the ones currently
    /// shown on the desktop.
    pub fn hooked_count(&self) -> usize {
        self.cards.iter().filter(|c| !c.leaving).count()
    }

    pub fn contains_hwnd(&self, hwnd: isize) -> bool {
        self.cards.iter().any(|c| c.hwnd == hwnd && !c.leaving)
    }

    pub fn get(&self, id: u64) -> Option<&Card> {
        self.cards.iter().find(|c| c.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Card> {
        self.cards.iter_mut().find(|c| c.id == id)
    }

    pub fn remove(&mut self, id: u64) -> Option<Card> {
        let index = self.cards.iter().position(|c| c.id == id)?;
        Some(self.cards.remove(index))
    }

    fn live_offsets(&self) -> Vec<f32> {
        self.cards.iter().filter(|c| !c.away && !c.leaving).map(|c| c.offset).collect()
    }

    fn live_widths(&self, layout: &LineLayout) -> Vec<f32> {
        self.cards
            .iter()
            .filter(|c| !c.away && !c.leaving)
            .map(|c| CARD_W * layout.scale * c.zoom)
            .collect()
    }

    /// How much the packed line had to shrink every card so they all fit. The
    /// renderer scales by this too, or a full line would draw its cards at full
    /// size and bury one under another.
    pub fn fit(&self, layout: &LineLayout) -> f32 {
        layout.pack_fit(&self.live_widths(layout))
    }

    /// Where each hanging card sits, in line order.
    pub fn targets(&self, layout: &LineLayout) -> Vec<f32> {
        placed(layout, &self.live_widths(layout), &self.live_offsets())
    }

    /// Resizes one card. Returns false when nothing changed, so the caller does
    /// not redraw or rewrite the journal for a drag that went nowhere.
    pub fn set_zoom(&mut self, id: u64, zoom: f32) -> bool {
        let Some(card) = self.get_mut(id) else { return false };
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        if (card.zoom - zoom).abs() < 0.001 {
            return false;
        }
        card.zoom = zoom;
        true
    }

    /// Resizes one card's height, see [`set_zoom`](Self::set_zoom).
    pub fn set_zoom_y(&mut self, id: u64, zoom_y: f32) -> bool {
        let Some(card) = self.get_mut(id) else { return false };
        let zoom_y = zoom_y.clamp(ZOOM_Y_MIN, ZOOM_Y_MAX);
        if (card.zoom_y - zoom_y).abs() < 0.001 {
            return false;
        }
        card.zoom_y = zoom_y;
        true
    }

    /// Where a dropped card goes in the order. `x` is the pointer, and every
    /// card whose centre is left of it belongs before the dropped one.
    pub fn drop_index(&self, layout: &LineLayout, id: u64, x: f32) -> usize {
        let mut widths = Vec::new();
        let mut offsets = Vec::new();
        for card in self.cards.iter().filter(|c| !c.away && !c.leaving && c.id != id) {
            widths.push(CARD_W * layout.scale * card.zoom);
            offsets.push(card.offset);
        }
        if widths.is_empty() {
            return 0;
        }
        placed(layout, &widths, &offsets).iter().filter(|cx| **cx < x).count()
    }

    /// Leaves a dragged card where it was released. The order has already been
    /// decided; this only records how far from its packed place it sits. The
    /// clamp keeps it on screen and leaves a gap to its neighbours, which is
    /// what stops two cards from covering each other.
    pub fn place_at(&mut self, id: u64, layout: &LineLayout, x: f32) -> bool {
        let cards: Vec<&Card> = self.cards.iter().filter(|c| !c.away && !c.leaving).collect();
        let Some(index) = cards.iter().position(|c| c.id == id) else { return false };
        let widths: Vec<f32> = cards.iter().map(|c| CARD_W * layout.scale * c.zoom).collect();
        let offsets: Vec<f32> = cards.iter().map(|c| if c.id == id { 0.0 } else { c.offset }).collect();
        let fit = layout.pack_fit(&widths);
        let base = layout.pack_x(&widths);
        let others = placed(layout, &widths, &offsets);
        let s = layout.scale;
        let half = widths[index] * fit / 2.0;
        let gap = PACK_GAP * s;
        let pad = EDGE_PAD * s;
        let lo = if index > 0 { others[index - 1] + half + widths[index - 1] * fit / 2.0 + gap } else { pad + half };
        let hi = if index + 1 < cards.len() {
            others[index + 1] - half - widths[index + 1] * fit / 2.0 - gap
        } else {
            layout.width - pad - half
        };
        let want = x.clamp(pad + half, layout.width - pad - half);
        let target = if lo <= hi { want.clamp(lo, hi) } else { base[index] };
        let Some(card) = self.get_mut(id) else { return false };
        card.offset = (target - base[index]) / s.max(0.01);
        card.x.snap(target);
        true
    }

    /// Adds a card that has just been hung up and starts its flight in.
    pub fn hang(&mut self, card: Card, layout: &LineLayout, reduced_motion: bool) -> u64 {
        let id = card.id;
        let mut card = card;
        card.away = false;
        card.away_at = None;
        card.unhook = false;
        card.flight = None;
        self.cards.push(card);
        let index = self.cards.iter().position(|c| c.id == id).expect("just pushed");
        let position = self.cards[..index].iter().filter(|c| !c.away && !c.leaving).count();
        let fit = self.fit(layout);
        let slot = self.targets(layout)[position];
        let sway = self.random(-1.6, 1.6);
        let card = self.get_mut(id).expect("just pushed");
        card.x = Spring::responsive(slot, SLOT_RESPONSE, SLOT_DAMPING);
        card.sway = tilt(sway);
        let rope = layout.rope_y(slot);
        card.ghost_to = card.rest_rect(slot, rope, layout.scale, fit);
        if reduced_motion {
            card.x.snap(slot);
        } else {
            let lift = (90.0 * layout.scale).max(40.0);
            card.flight = Some(ArcFlight::new(
                (
                    (card.ghost_from.left + card.ghost_from.right) as f32 / 2.0,
                    (card.ghost_from.top + card.ghost_from.bottom) as f32 / 2.0,
                ),
                (
                    (card.ghost_to.left + card.ghost_to.right) as f32 / 2.0,
                    (card.ghost_to.top + card.ghost_to.bottom) as f32 / 2.0,
                ),
                lift,
                HANG_FLIGHT,
            ));
        }
        id
    }

    /// Re-creates a card from the journal. `card.away` says whether the window is
    /// on the desktop with its card hidden (still hooked) or actually parked; the
    /// caller reads it out of the record.
    pub fn recover(&mut self, mut card: Card, layout: &LineLayout) -> u64 {
        let id = card.id;
        card.x = Spring::new(0.0, 0.0, 0.0);
        card.sway = tilt(0.0);
        card.flight = None;
        card.away_at = None;
        card.unhook = false;
        self.cards.push(card);
        let index = self.cards.iter().position(|c| c.id == id).expect("just pushed");
        let position = self.cards[..index].iter().filter(|c| !c.away && !c.leaving).count();
        // A card whose window was on the desktop is not part of the order until
        // it is hung up again, so it has no slot to take: `targets` does not
        // contain it. Reading a slot for it anyway is an out-of-bounds index.
        let slot = self.targets(layout).get(position).copied().unwrap_or(0.0);
        let card = self.get_mut(id).expect("just pushed");
        card.x = Spring::responsive(slot, SLOT_RESPONSE, SLOT_DAMPING);
        card.x.snap(slot);
        id
    }

    /// Puts a hooked window back on the line after it was minimized. The card
    /// is simply there on the next frame: the user asked for no fade, and a
    /// card that is still travelling cannot be clicked or dragged.
    pub fn rehang(&mut self, id: u64, layout: &LineLayout) -> bool {
        if !self.get(id).is_some_and(|c| c.away) {
            return false;
        }
        let sway0 = self.random(-1.6, 1.6);
        {
            let Some(card) = self.get_mut(id) else { return false };
            card.away = false;
            card.away_at = None;
            card.unhook = false;
            card.leaving = false;
            card.flight = None;
            card.sway = tilt(sway0);
            // A fresh snapshot arrives shortly; until then the old thumbnail
            // is better than a blank pane.
            card.thumb_version += 1;
        }
        let Some(index) = self.cards.iter().position(|c| c.id == id) else { return false };
        let position = self.cards[..index].iter().filter(|c| !c.away && !c.leaving).count();
        let slot = self.targets(layout)[position];
        let fit = self.fit(layout);
        if let Some(card) = self.get_mut(id) {
            card.x = Spring::responsive(slot, SLOT_RESPONSE, SLOT_DAMPING);
            card.x.snap(slot);
            card.ghost_to = card.rest_rect(slot, layout.rope_y(slot), layout.scale, fit);
        }
        true
    }

    /// Takes the card off the line but keeps the hook: the window is on the
    /// desktop, and minimizing it will bring the card back.
    pub fn detach(&mut self, id: u64, now: Instant) {
        if let Some(card) = self.get_mut(id) {
            card.away = true;
            card.away_at = Some(now);
            card.leaving = false;
            card.flight = None;
        }
    }

    /// Moves an on-line card to a new position in the line. `target` is an
    /// index among the cards that are actually hanging.
    pub fn reorder(&mut self, id: u64, target: usize) {
        let mut on_line: Vec<Card> = Vec::new();
        let mut others: Vec<Card> = Vec::new();
        for card in std::mem::take(&mut self.cards) {
            if card.away || card.leaving {
                others.push(card);
            } else {
                on_line.push(card);
            }
        }
        if let Some(from) = on_line.iter().position(|c| c.id == id) {
            let card = on_line.remove(from);
            let to = target.min(on_line.len());
            on_line.insert(to, card);
        }
        others.extend(on_line);
        self.cards = others;
    }

    /// Sends a card back to its window. The window is only restored once the
    /// flight has finished, so the user never sees it jump. With `no_flight`
    /// there is no animation to wait for: the caller restores immediately.
    pub fn take_down(&mut self, id: u64, layout: &LineLayout, no_flight: bool) -> bool {
        let Some(index) = self.cards.iter().position(|c| c.id == id) else { return false };
        if self.cards[index].leaving || self.cards[index].away {
            return false;
        }
        let slot_x = {
            let position = self.cards[..index].iter().filter(|c| !c.leaving && !c.away).count();
            self.targets(layout)[position]
        };
        let fit = self.fit(layout);
        let card = &mut self.cards[index];
        card.leaving = true;
        card.flight = None;
        let rope = layout.rope_y(slot_x);
        card.ghost_from = card.rest_rect(slot_x, rope, layout.scale, fit);
        card.ghost_to = card.window_rect;
        card.ghost_angle = (card.sway.x * 0.5, 0.0);
        if no_flight {
            return true;
        }
        let lift = (60.0 * layout.scale).max(30.0);
        card.flight = Some(ArcFlight::new(
            (
                (card.ghost_from.left + card.ghost_from.right) as f32 / 2.0,
                (card.ghost_from.top + card.ghost_from.bottom) as f32 / 2.0,
            ),
            (
                (card.ghost_to.left + card.ghost_to.right) as f32 / 2.0,
                (card.ghost_to.top + card.ghost_to.bottom) as f32 / 2.0,
            ),
            lift,
            TAKE_FLIGHT,
        ));
        true
    }

    /// Puts a leaving card back on the line. Used when the window refuses to
    /// come back: losing the card would leave the window stranded off screen.
    pub fn cancel_take_down(&mut self, id: u64, _layout: &LineLayout) {
        let Some(card) = self.get_mut(id) else { return };
        card.leaving = false;
        card.flight = None;
    }

    /// Advances every animation. Returns the ids of cards that have finished
    /// flying back and are ready for their window to be restored.
    pub fn step(&mut self, dt: f32, now: Instant, layout: &LineLayout, hover: Option<u64>) -> Vec<u64> {
        if self.gust_back.is_some_and(|t| now >= t) {
            self.gust_back = None;
            for card in &mut self.cards {
                if !card.flying() {
                    card.sway.retarget(0.0);
                }
            }
        }
        let slots = self.targets(layout);
        let mut slot = 0usize;
        let mut finished = Vec::new();
        let mut keep = Vec::with_capacity(self.cards.len());
        for mut card in std::mem::take(&mut self.cards) {
            if card.away {
                keep.push(card);
                continue;
            }
            if card.leaving {
                let done = match card.flight.as_mut() {
                    Some(flight) => !flight.step(dt),
                    None => true,
                };
                if done {
                    // Keep the card until the app has restored the window (or
                    // put the card back if it refused). Dropping it here would
                    // leave the window parked with nothing on the line.
                    finished.push(card.id);
                    keep.push(card);
                    continue;
                }
                card.sway.step(dt);
                keep.push(card);
                continue;
            }
            let target_x = slots[slot.min(slots.len().saturating_sub(1))];
            slot += 1;
            // While the pointer drags a card, it follows the pointer instead of
            // its slot. Dropping it reorders the line.
            if self.dragging != Some(card.id) {
                card.x.retarget(target_x);
            }
            card.x.step(dt);
            if card.flight.is_some() {
                let landed = match card.flight.as_mut() {
                    Some(flight) => !flight.step(dt),
                    None => false,
                };
                if landed {
                    // The flight is over: put the card exactly on its slot, so
                    // it does not slide after the snapshot has already landed.
                    card.flight = None;
                    card.x.settle();
                }
            }
            card.sway.step(dt);
            card.scale_goal = if hover == Some(card.id) { HOVER_SCALE } else { 1.0 };
            card.scale = shaiwin_core::spring::approach(card.scale, card.scale_goal, dt, 16.0);
            if (card.scale - card.scale_goal).abs() < 0.0005 {
                card.scale = card.scale_goal;
            }
            keep.push(card);
        }
        self.cards = keep;
        finished
    }

    /// True while anything is in the air, which keeps the frame timer running.
    /// Everything here must be able to come back to rest, or this stays true for
    /// the whole session and the app redraws at 60Hz forever.
    pub fn animating(&self) -> bool {
        self.cards.iter().any(|c| {
            !c.away
                && (c.flying()
                    || !c.x.at_rest()
                    || !c.sway.at_rest()
                    || (c.scale - c.scale_goal).abs() > 0.001)
        })
    }

    /// The rectangle a card's ghost covers right now, in screen coordinates.
    pub fn ghost_rect(card: &Card) -> Option<WRect> {
        let flight = card.flight.as_ref()?;
        let (x, y) = flight.position();
        let t = flight.raw();
        let r = lerp_rect(
            (
                card.ghost_from.left as f32,
                card.ghost_from.top as f32,
                card.ghost_from.width() as f32,
                card.ghost_from.height() as f32,
            ),
            (card.ghost_to.left as f32, card.ghost_to.top as f32, card.ghost_to.width() as f32, card.ghost_to.height() as f32),
            t,
        );
        // The flight drives the centre; keep the rectangle centred on it.
        Some(WRect {
            left: (x - r.2 / 2.0).round() as i32,
            top: (y - r.3 / 2.0).round() as i32,
            right: (x + r.2 / 2.0).round() as i32,
            bottom: (y + r.3 / 2.0).round() as i32,
        })
    }

    /// Maybe starts a gust. Returns true when it did, so the caller knows a
    /// redraw is needed.
    pub fn gauge(&mut self, now: Instant) -> bool {
        if self.live_count() == 0 || now < self.next_gust {
            return false;
        }
        let angles: Vec<f32> = (0..self.cards.len()).map(|_| self.random(1.2, 2.6)).collect();
        for (card, angle) in self.cards.iter_mut().zip(angles) {
            if !card.leaving && !card.away && !card.flying() {
                card.sway.retarget(angle);
            }
        }
        self.gust_back = Some(now + Duration::from_millis(280));
        self.next_gust = now + Duration::from_secs_f32(self.random(GUST_MIN, GUST_MAX));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: i32, t: i32, r: i32, b: i32) -> WRect {
        WRect { left: l, top: t, right: r, bottom: b }
    }

    fn card(id: u64) -> Card {
        let placement = Placement {
            flags: 0,
            show_cmd: 1,
            min_x: 0,
            min_y: 0,
            max_x: 0,
            max_y: 0,
            normal: rect(100, 100, 600, 500),
        };
        Card {
            id,
            hwnd: id as isize,
            pid: 1,
            process_start: 1,
            title: format!("window {id}"),
            app: "test".to_string(),
            exe: "test.exe".to_string(),
            accent: Rgb::new(200, 120, 60),
            placement,
            parked: Parked { placement, parked: rect(3000, 0, 3500, 400), was_maximized: false },
            window_rect: rect(100, 100, 600, 500),
            thumb: None,
            thumb_version: 0,
            icon: None,
            zoom: 1.0,
            zoom_y: 1.0,
            offset: 0.0,
            x: Spring::new(0.0, 0.0, 0.0),
            sway: Spring::new(0.0, 0.0, 0.0),
            scale: 1.0,
            scale_goal: 1.0,
            flight: None,
            ghost_from: rect(0, 0, 10, 10),
            ghost_to: rect(0, 0, 10, 10),
            ghost_angle: (0.0, 0.0),
            anchor: (0, 0),
            leaving: false,
            away: false,
            away_at: None,
            unhook: false,
        }
    }

    fn hung(line: &mut Line, ids: &[u64], layout: &LineLayout) {
        for id in ids {
            line.hang(card(*id), layout, true);
        }
    }

    fn live_ids(line: &Line) -> Vec<u64> {
        line.cards.iter().filter(|c| !c.away && !c.leaving).map(|c| c.id).collect()
    }

    fn layout() -> LineLayout {
        LineLayout::with_sag(2560.0, 1.0, 12.0)
    }

    #[test]
    fn a_card_can_be_moved_along_the_line() {
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2, 3], &l);
        assert_eq!(live_ids(&line), vec![1, 2, 3]);

        line.reorder(1, 2);
        assert_eq!(live_ids(&line), vec![2, 3, 1]);

        line.reorder(1, 0);
        assert_eq!(live_ids(&line), vec![1, 2, 3]);

        // Past the end means the last slot, not a panic.
        line.reorder(1, 99);
        assert_eq!(live_ids(&line), vec![2, 3, 1]);
    }

    #[test]
    fn a_window_back_on_the_desktop_is_not_part_of_the_order() {
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2, 3], &l);
        line.detach(2, Instant::now());
        assert_eq!(line.live_count(), 2);

        // Slot 1 of the two that hang is still the end of the line, and the
        // hidden card stays hidden rather than being pulled back into it.
        line.reorder(1, 1);
        assert_eq!(live_ids(&line), vec![3, 1]);
        assert!(line.get(2).unwrap().away);
        assert_eq!(line.cards.len(), 3);
    }

    #[test]
    fn detaching_keeps_the_hook_and_rehanging_clears_it() {
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2], &l);

        let now = Instant::now();
        line.detach(1, now);
        assert!(line.get(1).unwrap().away);
        assert_eq!(line.hooked_count(), 2);
        assert_eq!(live_ids(&line), vec![2]);

        // A card that is not on the line cannot be taken down again.
        assert!(!line.take_down(1, &l, true));

        // Minimizing the window puts it back; the hook is what makes this possible.
        assert!(line.rehang(1, &l));
        assert!(!line.get(1).unwrap().away);
        assert_eq!(line.live_count(), 2);
    }

    #[test]
    fn a_hung_line_is_at_rest_and_a_gust_stops_ringing() {
        // What the user felt as 卡: the decorative swing rang for seconds at a
        // time, and every frame of it costs a full-strip composite. A line that is
        // merely hanging there must cost nothing.
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2, 3], &l);
        run(&mut line, &l, 1.0);
        assert!(!line.animating(), "a line that is only hanging has something moving");

        // A gust, the way `gauge` throws one: every card takes a new angle.
        for card in line.cards.iter_mut() {
            card.sway.retarget(2.4);
        }
        assert!(line.animating(), "a gust did not start anything moving");
        run(&mut line, &l, 2.5);
        assert!(!line.animating(), "the swing outlasted 2.5 s");
    }

    /// Where each hanging card belongs, in the order `step` walks the line.
    fn expected_slots(line: &Line, l: &LineLayout) -> Vec<(u64, f32)> {
        let targets = line.targets(l);
        let mut out = Vec::new();
        let mut index = 0;
        for card in &line.cards {
            if card.away || card.leaving {
                continue;
            }
            out.push((card.id, targets[index]));
            index += 1;
        }
        out
    }

    fn run(line: &mut Line, l: &LineLayout, seconds: f32) {
        let now = Instant::now();
        for _ in 0..(seconds * 60.0) as u32 {
            line.step(1.0 / 60.0, now, l, None);
        }
    }

    #[test]
    fn a_card_under_the_pointer_comes_to_rest_at_its_hover_size() {
        // The hover pop is eased towards a size, and that size is the resting
        // state. Measuring rest against 1.0 kept the frame timer running for as
        // long as the pointer rested on the line, which is when the user is
        // looking at it.
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2], &l);
        let now = Instant::now();
        for _ in 0..180 {
            line.step(1.0 / 60.0, now, &l, Some(1));
        }
        let hovered = line.get(1).unwrap();
        assert!(hovered.scale > 1.02, "the card under the pointer did not enlarge");
        assert_eq!(hovered.scale, hovered.scale_goal);
        assert!(!line.animating(), "a hovered card kept the line animating");

        // And it eases back down when the pointer leaves.
        run(&mut line, &l, 2.0);
        assert_eq!(line.get(1).unwrap().scale, 1.0);
        assert!(!line.animating());
    }

    #[test]
    fn a_rehung_card_is_on_its_own_slot_at_once() {
        // No flight, no fade: the card is where it belongs on the first frame,
        // which is also what makes it clickable straight away.
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2, 3], &l);
        line.detach(2, Instant::now());

        assert!(line.rehang(2, &l));
        let card = line.get(2).unwrap();
        assert!(!card.flying());
        let slot = expected_slots(&line, &l).into_iter().find(|(id, _)| *id == 2).unwrap().1;
        assert!((card.x.x - slot).abs() < 0.5, "card 2 sits at {}, belongs at {slot}", card.x.x);

        run(&mut line, &l, 3.0);
        assert!(!line.animating(), "the line never came to rest");
        for (id, slot) in expected_slots(&line, &l) {
            let x = line.get(id).unwrap().x.x;
            assert!((x - slot).abs() < 0.5, "card {id} sits at {x}, belongs at {slot}");
        }
    }

    #[test]
    fn a_dragged_card_stays_where_it_was_dropped() {
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2], &l);
        run(&mut line, &l, 2.0);
        let second = line.get(2).unwrap().x.x;

        // The real drop: the pointer decides the order first, then the card keeps
        // the spot where it was released. Card 1 goes past card 2 without
        // springing back onto its packed slot.
        let wanted = second + 40.0;
        let target = line.drop_index(&l, 1, wanted);
        assert_eq!(target, 1, "the pointer is past card 2");
        line.reorder(1, target);
        assert!(line.place_at(1, &l, wanted));
        run(&mut line, &l, 2.0);

        let x = line.get(1).unwrap().x.x;
        assert!((x - wanted).abs() < 1.0, "card 1 snapped back to {x}, wanted {wanted}");
        assert!(line.get(2).unwrap().x.x < second, "card 2 did not close the gap card 1 left");
    }

    #[test]
    fn a_drop_cannot_push_a_card_over_its_neighbour() {
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1, 2], &l);
        let second = line.get(2).unwrap().x.x;

        // Dropping the first card right on top of the second must stop at the
        // gap, not cover it: a covered card cannot be clicked.
        assert!(line.place_at(1, &l, second));
        run(&mut line, &l, 2.0);
        let first = line.get(1).unwrap().x.x;
        assert!(first < second, "card 1 ended up at {first}, card 2 at {second}");
        assert!(second - first > CARD_W * l.scale * 0.9, "the two cards overlap");
    }

    #[test]
    fn cards_recovered_into_a_longer_line_slide_to_their_slots() {
        let l = layout();
        let mut line = Line::new();
        // Each record is recovered while the line is shorter than it will end up,
        // so every card has to be retargeted. A spring that cannot move leaves
        // them stacked on top of each other.
        for id in 1..=4 {
            line.recover(card(id), &l);
        }
        run(&mut line, &l, 3.0);
        assert!(!line.animating());
        let mut last = -1.0;
        for (id, slot) in expected_slots(&line, &l) {
            let x = line.get(id).unwrap().x.x;
            assert!((x - slot).abs() < 0.5, "card {id} sits at {x}, belongs at {slot}");
            assert!(x > last, "cards must not share a slot");
            last = x;
        }
    }

    #[test]
    fn a_record_hooked_on_the_desktop_recovers_before_the_line_does() {
        // The journal is written in line order, so the first record after a
        // restart can be a window that was on the desktop. It has no slot, and
        // asking for one is what killed the app on startup.
        let l = layout();
        let mut line = Line::new();
        let mut away = card(1);
        away.away = true;
        line.recover(away, &l);
        line.recover(card(2), &l);
        run(&mut line, &l, 2.0);
        assert!(!line.animating());
        let slots = expected_slots(&line, &l);
        assert_eq!(slots.len(), 1, "only the card on the line has a slot");
        assert!((line.get(2).unwrap().x.x - slots[0].1).abs() < 0.5);
    }

    #[test]
    fn a_hang_lands_without_leaving_anything_moving() {
        let l = layout();
        let mut line = Line::new();
        let mut first = card(1);
        first.ghost_from = rect(400, 300, 1400, 900);
        assert!(line.hang(first, &l, false) > 0);
        assert!(line.get(1).unwrap().flying());

        run(&mut line, &l, 2.0);
        let card1 = line.get(1).unwrap();
        assert!(!card1.flying());
        assert!(card1.x.at_rest());
        assert!(!line.animating());
    }

    #[test]
    fn a_card_resizes_downwards_without_moving_its_top() {
        let l = layout();
        let mut line = Line::new();
        hung(&mut line, &[1], &l);
        let s = l.scale;
        let before = line.get(1).unwrap().rest_rect(500.0, 40.0, s, 1.0);
        assert!(line.set_zoom_y(1, 1.6));
        let after = line.get(1).unwrap().rest_rect(500.0, 40.0, s, 1.0);
        assert_eq!(before.top, after.top, "the big card moved its top edge");
        assert_eq!(before.left, after.left, "resizing height changed the width");
        let grown = (after.bottom - after.top) as f32;
        assert!((grown / (CARD_H * s) - 1.6).abs() < 0.02, "height zoom = {}", grown / (CARD_H * s));

        // The middle of the bottom edge resizes height, the sides resize width,
        // and the bottom corners resize both at once.
        assert_eq!(edge_at(&after, (after.left + 60, after.bottom - 3), s), Some(Edge::Bottom));
        assert_eq!(edge_at(&after, (after.left + 4, after.bottom - 4), s), Some(Edge::BottomLeft));
        assert_eq!(edge_at(&after, (after.right - 5, after.bottom - 2), s), Some(Edge::BottomRight));
        assert_eq!(edge_at(&after, (after.left + 3, after.top + 60), s), Some(Edge::Left));
        assert_eq!(edge_at(&after, (after.left + 60, after.top + 60), s), None);
        // A corner is a small target; the diagonal zone is wider than the edge
        // strip so a grab that is a few pixels off still resizes both ways.
        assert_eq!(edge_at(&after, (after.left + 12, after.bottom - 12), s), Some(Edge::BottomLeft));
    }

    #[test]
    fn each_edge_says_which_way_it_resizes() {
        assert!(Edge::Left.resizes_width() && !Edge::Left.resizes_height());
        assert!(Edge::Right.resizes_width() && !Edge::Right.resizes_height());
        assert!(!Edge::Bottom.resizes_width() && Edge::Bottom.resizes_height());
        assert!(Edge::BottomLeft.resizes_width() && Edge::BottomLeft.resizes_height());
        assert!(Edge::BottomRight.resizes_width() && Edge::BottomRight.resizes_height());
    }

    #[test]
    fn the_height_zoom_follows_the_bottom_edge_and_stops_at_its_limits() {
        // The grab point is the card's bottom; the top is pinned.
        let unit = CARD_H;
        assert!((zoom_y_from_bottom(40.0 + CARD_H * 1.5, 40.0, unit) - 1.5).abs() < 0.001);
        assert_eq!(zoom_y_from_bottom(0.0, 40.0, unit), ZOOM_Y_MIN);
        assert_eq!(zoom_y_from_bottom(4000.0, 40.0, unit), ZOOM_Y_MAX);
    }
}
