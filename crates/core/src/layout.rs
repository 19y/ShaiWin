//! Where the line hangs and where each card sits on it.
//!
//! All values are device pixels for one monitor. The design sizes are in
//! logical pixels and get multiplied by [`LineLayout::scale`], which is the
//! monitor's DPI factor. The Windows shell passes `scale` from
//! `GetDpiForMonitor`; a Swift shell would pass `backingScaleFactor`.

/// Cached layout for one monitor at one DPI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineLayout {
    /// Monitor width in device pixels.
    pub width: f32,
    /// DPI factor (1.0 at 96 DPI, 1.5 at 144 DPI, 2.0 at 192 DPI).
    pub scale: f32,
    /// How far below the top edge the rope hangs, in logical pixels.
    pub top_inset: f32,
    /// How far the middle of the rope hangs below its ends, in device pixels.
    pub sag: f32,
    /// Height of the strip, in logical pixels. It grows with the sag so the
    /// lowest card still fits.
    panel_h: f32,
}

/// Design sizes, in logical pixels.
pub const PANEL_H: f32 = 212.0;
pub const ROPE_TOP: f32 = 26.0;
pub const CARD_W: f32 = 168.0;
pub const CARD_H: f32 = 118.0;
pub const SPACING: f32 = 206.0;
pub const SPACING_MIN: f32 = 128.0;
pub const EDGE_PAD: f32 = 56.0;
/// A window whose top edge is this close to the top of the screen belongs to
/// Windows' own maximize strip, not to the line: the system clamps a window
/// pushed to the top edge to exactly the edge, and takes it full height on
/// release. This is only the margin that absorbs that clamp - the honest
/// evidence is [`window_flush`] seeing the real rectangle. In logical pixels.
pub const MAXIMIZE_YIELD: f32 = 2.0;

/// The sag the line ships with, in device pixels: a fixed 26 logical pixels at
/// the middle, easing off on a narrow screen so a laptop does not get a
/// parabola. Tried against a deeper curve and this is the one that reads right.
pub fn classic_sag(width: f32, scale: f32) -> f32 {
    (26.0 * scale.max(0.5)).min(width.max(1.0) * 0.016)
}

/// The deepest the slider can make the rope. At 16% of a 1920 px screen the
/// middle of the line is 307 px below the edge, which still leaves the strip
/// short enough that the taskbar stays reachable.
pub const SAG_PERCENT_MAX: f32 = 16.0;
/// How far below the rope the renderer hangs the card body - the clothespin.
/// Measured off a dumped frame: the bottom of a card is `rope_y + CARD_TOP_GAP +
/// CARD_H`. `CARD_BOTTOM_GAP` is the strip's breathing room under the cards.
const CARD_TOP_GAP: f32 = 22.0;
const CARD_BOTTOM_GAP: f32 = 18.0;

/// How far above and below the drawn rope a pointer still counts as being on
/// it, in logical pixels. The stroke is a couple of pixels wide, and aiming at
/// the drawing itself makes the pointer feel like a needle. This is the grab
/// band: a 28 logical px target, which is comfortable without being a wall.
/// It stays `CARD_TOP_GAP - ROPE_REACH` (8 px at 100%) clear of the card tops,
/// so no widening can ever make the band answer for a card's place on the
/// strip - that place belongs to the user's windows and must not summon the
/// cards back.
pub const ROPE_REACH: f32 = 14.0;

impl LineLayout {
    /// The shipped geometry: the classic sag.
    pub fn new(width: f32, scale: f32) -> Self {
        let width = width.max(1.0);
        let scale = scale.max(0.5);
        let sag = classic_sag(width, scale);
        Self::built(width, scale, sag)
    }

    /// Builds the layout with a user-chosen sag. `sag_percent` is the sag at
    /// the middle as a percentage of the monitor width (`0.0` is a straight
    /// rope). The strip grows taller as the sag grows, so a deep curve never
    /// pushes the lowest card outside the overlay.
    pub fn with_sag(width: f32, scale: f32, sag_percent: f32) -> Self {
        let width = width.max(1.0);
        let scale = scale.max(0.5);
        let sag = width * (sag_percent.clamp(0.0, SAG_PERCENT_MAX) / 100.0);
        Self::built(width, scale, sag)
    }

    fn built(width: f32, scale: f32, sag: f32) -> Self {
        let panel_h =
            (ROPE_TOP + CARD_TOP_GAP + CARD_H + CARD_BOTTOM_GAP + sag / scale).max(PANEL_H);
        LineLayout {
            width,
            scale,
            top_inset: ROPE_TOP,
            sag,
            panel_h,
        }
    }

    /// How far below the top edge a dragged window may be released and still
    /// hang: the lowest point of the rope, plus the clothespin.
    ///
    /// The light has to come on when the window's top edge has actually met the
    /// rope, and nowhere else. Measured on the user's 2560 px @100% screen with
    /// the rope dialed to 1.5%: its deepest point sits at y=64, but a band
    /// ending at the bottom of the cards (`204`) lit up and hung releases at
    /// `top=171/187/199/203` - windows whose top edge was still 135 px under the
    /// rope, which he felt as the line grabbing windows he was only moving.
    /// The cards are what hang off the rope, not the thing to aim at.
    ///
    /// This costs the opposite case, and it is taken on purpose: a release level
    /// with a card body is cold again. The rope is the aim, and the `sag`
    /// setting moves the aim with it - a deeper curve reaches further down.
    /// Kept at the deepest point of the curve so a window anywhere along the
    /// rope gets the same band, and the clothespin is the tolerance that makes
    /// "met the rope" not mean "landed on the pixel".
    pub fn hang_zone(&self) -> f32 {
        self.rope_y(self.width / 2.0) + CARD_TOP_GAP * self.scale
    }

    /// The bottom edge of the cards, in device pixels below the top edge.
    ///
    /// The floor of the line's own strip. The cards hang from the deepest point
    /// of the rope - its middle - so this is the lowest a card reaches; below it
    /// is the user's desktop, which the line never claims. The strip the overlay
    /// window occupies is taller (`panel_height`), because it keeps breathing
    /// room under the cards, and that room is not part of the line.
    pub fn card_bottom(&self) -> f32 {
        self.hang_zone() + CARD_H * self.scale
    }

    /// Whether the window being dragged has its own top edge on the line. Where
    /// the pointer is says nothing about that: a window still in the middle of
    /// the screen has never met the rope, and answering for it hangs windows the
    /// user only nudged.
    pub fn window_on_line(&self, window_top: i32, screen_top: i32) -> bool {
        !self.window_flush(window_top, screen_top) && window_top <= screen_top + self.hang_depth()
    }

    /// Whether the window is flush with the top of the screen. That is Windows'
    /// own maximize gesture, and the line has to stand aside for it.
    pub fn window_flush(&self, window_top: i32, screen_top: i32) -> bool {
        window_top <= screen_top + (MAXIMIZE_YIELD * self.scale).round() as i32
    }

    /// The depth of the drop band below the top edge, in device pixels.
    pub fn hang_depth(&self) -> i32 {
        self.hang_zone().round() as i32
    }

    /// The height of the strip the overlay window needs, in device pixels.
    pub fn panel_height(&self) -> f32 {
        self.panel_h * self.scale
    }

    /// How far the rope sags in the middle, in device pixels. Proportional to
    /// the screen so it looks the same on a laptop and an ultrawide.
    pub fn sag(&self) -> f32 {
        self.sag
    }

    /// The rope's grab band above and below the stroke, in device pixels.
    pub fn rope_reach(&self) -> f32 {
        ROPE_REACH * self.scale
    }

    /// Screen x of the rope at `x`.
    pub fn rope_y(&self, x: f32) -> f32 {
        if self.width <= 0.0 {
            return self.top_inset * self.scale;
        }
        let f = (x / self.width).clamp(0.0, 1.0);
        self.top_inset * self.scale + 4.0 * self.sag() * f * (1.0 - f)
    }

    /// Distance between neighbours. Cards pack tighter as the line fills up,
    /// but never so tight that the thumbnails stop being readable.
    pub fn spacing_for(&self, count: usize) -> f32 {
        if count <= 1 {
            return SPACING * self.scale;
        }
        let usable = (self.width - 2.0 * EDGE_PAD * self.scale).max(1.0);
        let ideal = SPACING * self.scale;
        let min = SPACING_MIN * self.scale;
        (usable / (count - 1) as f32).min(ideal).max(min)
    }

    /// Centre x of the card at `index` when `count` cards are hanging.
    pub fn slot_x(&self, index: usize, count: usize) -> f32 {
        if count == 0 {
            return self.width / 2.0;
        }
        let spacing = self.spacing_for(count);
        let total = spacing * (count.saturating_sub(1)) as f32;
        let start = (self.width - total) / 2.0;
        start + spacing * index as f32
    }

    /// Centre x for cards that are each their own width.
    ///
    /// `widths` are device pixels, in the order they hang. The line closes its
    /// gaps first and only then shrinks every card by the same factor, so a
    /// user who made one card small really does get more room and the line
    /// never spills off the screen.
    pub fn pack_x(&self, widths: &[f32]) -> Vec<f32> {
        let n = widths.len();
        if n == 0 {
            return Vec::new();
        }
        if n == 1 {
            return vec![self.width / 2.0];
        }
        let fit = self.pack_fit(widths);
        let gap = self.pack_gap(widths);
        let total = widths.iter().sum::<f32>() * fit + gap * (n - 1) as f32;
        let mut x = (self.width - total) / 2.0 + widths[0] * fit / 2.0;
        let mut out = Vec::with_capacity(n);
        out.push(x);
        for pair in widths.windows(2) {
            x += pair[0] * fit / 2.0 + gap + pair[1] * fit / 2.0;
            out.push(x);
        }
        out
    }

    /// The gap [`pack_x`] hangs the cards at.
    fn pack_gap(&self, widths: &[f32]) -> f32 {
        let n = widths.len();
        if n < 2 {
            return 0.0;
        }
        let usable = (self.width - 2.0 * EDGE_PAD * self.scale).max(1.0);
        let sum: f32 = widths.iter().sum();
        let gap_max = ((SPACING - CARD_W) * self.scale).max(1.0);
        let gap_min = (8.0 * self.scale).max(1.0);
        if sum + gap_max * (n - 1) as f32 <= usable {
            return gap_max;
        }
        if sum + gap_min * (n - 1) as f32 <= usable {
            return ((usable - sum) / (n - 1) as f32).clamp(gap_min, gap_max);
        }
        gap_min
    }

    /// The factor [`pack_x`] had to apply to every card's width to make them
    /// fit, which is what the caller needs to draw the cards at the size they
    /// were actually packed at.
    pub fn pack_fit(&self, widths: &[f32]) -> f32 {
        let n = widths.len();
        if n < 2 {
            return 1.0;
        }
        let usable = (self.width - 2.0 * EDGE_PAD * self.scale).max(1.0);
        let sum: f32 = widths.iter().sum();
        let gap_min = (8.0 * self.scale).max(1.0);
        if sum + gap_min * (n - 1) as f32 <= usable {
            return 1.0;
        }
        ((usable - gap_min * (n - 1) as f32) / sum.max(1.0)).clamp(0.35, 1.0)
    }

    /// Where a card with `count` cards already on the line starts its flight:
    /// just left of the leftmost card, so the line grows outwards.
    pub fn entering_slot_x(&self, count: usize) -> f32 {
        self.slot_x(0, count + 1)
    }

    /// How many cards fit without going below the minimum spacing.
    pub fn max_items(&self) -> usize {
        let usable = (self.width - 2.0 * EDGE_PAD * self.scale).max(1.0);
        let min = SPACING_MIN * self.scale;
        ((usable / min).floor() as usize + 1).clamp(1, 24)
    }

    /// Index whose slot contains `x`, for drop-on-the-line targeting.
    pub fn index_at(&self, x: f32, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        let spacing = self.spacing_for(count);
        let start = self.slot_x(0, count);
        let raw = ((x - start) / spacing).round();
        raw.clamp(0.0, (count - 1) as f32) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_cards_are_centred_and_never_cross() {
        let l = LineLayout::new(1920.0, 1.0);
        let widths = vec![CARD_W * l.scale; 3];
        let x = l.pack_x(&widths);
        assert_eq!(x.len(), 3);
        assert!((x[1] - 960.0).abs() < 0.01, "middle at {}", x[1]);
        for i in 1..x.len() {
            assert!(x[i] > x[i - 1], "slot {i} at or left of its neighbour");
            let gap = x[i] - x[i - 1] - CARD_W;
            assert!(gap > 0.0, "cards overlap by {}", -gap);
        }
    }

    #[test]
    fn a_wide_card_takes_more_room_than_a_narrow_one() {
        let l = LineLayout::new(1920.0, 1.0);
        let narrow = CARD_W * 0.6;
        let wide = CARD_W * 1.6;
        let even = l.pack_x(&[CARD_W, CARD_W]);
        let uneven = l.pack_x(&[wide, narrow]);
        // Neighbours sit half of each of their widths plus the gap apart, so a
        // wide card next to a narrow one must be exactly (wide+narrow)/2+gap.
        let gap = even[1] - even[0] - CARD_W;
        let expected = (wide + narrow) / 2.0 + gap;
        assert!(
            (uneven[1] - uneven[0] - expected).abs() < 0.01,
            "wide+narrow neighbours sit {} apart, expected {}",
            uneven[1] - uneven[0],
            expected
        );
    }

    #[test]
    fn making_cards_smaller_is_what_fits_more_of_them() {
        // The point of the feature: the user shrinks cards because the line ran
        // out of room. Ten small cards must fit where ten normal ones are
        // squeezed; ten normal ones must still fit on screen when they are not.
        let l = LineLayout::new(1920.0, 1.0);
        let small = vec![CARD_W * 0.6; 10];
        assert_eq!(
            l.pack_fit(&small),
            1.0,
            "ten small cards should not need squeezing"
        );
        let xs = l.pack_x(&small);
        assert!(xs[0] > 0.0 && *xs.last().unwrap() < l.width);

        let big = vec![CARD_W * 1.6; 10];
        let fit = l.pack_fit(&big);
        assert!(fit < 1.0, "ten wide cards were allowed to spill");
        let xs = l.pack_x(&big);
        assert!(
            xs[0] - CARD_W * 1.6 * fit / 2.0 > -1.0,
            "first card off the left edge at {}",
            xs[0]
        );
        assert!(*xs.last().unwrap() + CARD_W * 1.6 * fit / 2.0 < l.width + 1.0);
    }

    #[test]
    fn one_card_sits_in_the_middle_however_wide_it_is() {
        let l = LineLayout::new(1920.0, 1.0);
        assert_eq!(l.pack_x(&[CARD_W * 2.0]), vec![960.0]);
        assert!(l.pack_x(&[]).is_empty());
    }

    #[test]
    fn slots_are_centred() {
        let l = LineLayout::new(1920.0, 1.0);
        let mid = l.slot_x(2, 5);
        assert!((mid - 960.0).abs() < 0.01, "mid = {mid}");
    }

    #[test]
    fn slots_never_cross() {
        let l = LineLayout::new(1280.0, 1.5);
        for count in 1..20 {
            for i in 1..count {
                assert!(l.slot_x(i, count) > l.slot_x(i - 1, count));
            }
        }
    }

    #[test]
    fn cards_stay_on_screen_for_reasonable_counts() {
        let l = LineLayout::new(1280.0, 1.0);
        let n = l.max_items();
        assert!(n > 4);
        for i in 0..n {
            let x = l.slot_x(i, n);
            assert!(x > 0.0 && x < l.width, "slot {i} at {x}");
        }
    }

    #[test]
    fn rope_sags_in_the_middle() {
        let l = LineLayout::new(1920.0, 1.0);
        assert!(l.rope_y(960.0) > l.rope_y(0.0));
        assert!((l.rope_y(0.0) - l.rope_y(1920.0)).abs() < 0.01);
        assert!((l.rope_y(960.0) - l.rope_y(0.0) - l.sag()).abs() < 0.01);
    }

    #[test]
    fn a_straight_rope_is_available() {
        let l = LineLayout::with_sag(1920.0, 1.0, 0.0);
        assert_eq!(l.sag(), 0.0);
        assert!((l.rope_y(960.0) - l.rope_y(0.0)).abs() < 0.01);
    }

    #[test]
    fn the_rope_grab_band_is_a_target_not_a_wall() {
        // Wide enough that aiming at a 2 px stroke does not feel like a needle,
        // narrow enough that it cannot start answering for the strip below it.
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let l = LineLayout::new(2560.0, scale);
            let reach = l.rope_reach();
            assert!(
                reach >= 10.0 * scale,
                "a {reach} px grab band at {scale}x is back to feeling like a needle"
            );
            assert!(
                reach < CARD_TOP_GAP * scale,
                "{reach} px at {scale}x must stay above the card tops ({})",
                CARD_TOP_GAP * scale
            );
            assert!((reach - ROPE_REACH * scale).abs() < 0.01);
        }
    }

    #[test]
    fn a_deep_sag_makes_the_strip_taller() {
        let shallow = LineLayout::with_sag(2560.0, 1.0, 0.0);
        let deep = LineLayout::with_sag(2560.0, 1.0, 8.0);
        assert!(deep.panel_height() > shallow.panel_height());
        // The lowest card (at the middle of the rope) must fit inside.
        let lowest = deep.rope_y(1280.0) + CARD_TOP_GAP + CARD_H;
        assert!(
            lowest < deep.panel_height(),
            "lowest = {lowest}, panel = {}",
            deep.panel_height()
        );
    }

    #[test]
    fn the_line_stops_at_the_bottom_of_the_cards() {
        // The floor of the strip the line claims. The overlay window is taller
        // than the cards - it keeps breathing room under them - and that room
        // belongs to the user's windows, so the two must not be confused.
        for scale in [1.0, 1.5] {
            let l = LineLayout::with_sag(2560.0, scale, 1.5);
            let want = l.rope_y(1280.0) + (CARD_TOP_GAP + CARD_H) * scale;
            assert!(
                (l.card_bottom() - want).abs() < 0.01,
                "card_bottom = {}, want = {want}",
                l.card_bottom()
            );
            assert!(
                l.card_bottom() > l.hang_zone(),
                "the cards end below where they start"
            );
            assert!(
                l.card_bottom() < l.panel_height(),
                "card_bottom = {} leaves no room under the cards in a {} panel",
                l.card_bottom(),
                l.panel_height()
            );
        }
    }

    #[test]
    fn the_sag_is_a_percentage_of_the_width() {
        let l = LineLayout::with_sag(2000.0, 1.0, 3.0);
        assert!((l.sag() - 60.0).abs() < 0.01);
        let clamped = LineLayout::with_sag(2000.0, 1.0, 500.0);
        assert!((clamped.sag() - 2000.0 * SAG_PERCENT_MAX / 100.0).abs() < 0.01);
    }

    #[test]
    fn the_default_curve_is_the_one_that_shipped() {
        // A deeper curve was tried and rejected: it pushes the middle of the rope
        // out of the drop zone and reads as a hammock. The default stays at the
        // original 26 logical pixels, capped on narrow screens.
        let l = LineLayout::new(2560.0, 1.0);
        assert!((l.sag() - 26.0).abs() < 0.01, "sag = {}", l.sag());
        let laptop = LineLayout::new(1000.0, 1.0);
        assert!((laptop.sag() - 16.0).abs() < 0.01, "sag = {}", laptop.sag());
        let hidpi = LineLayout::new(2560.0, 1.5);
        assert!(
            (hidpi.sag() - 26.0 * 1.5).abs() < 0.01,
            "sag = {}",
            hidpi.sag()
        );
        let lowest = l.rope_y(1280.0) + CARD_TOP_GAP + CARD_H;
        assert!(
            lowest < l.panel_height(),
            "lowest = {lowest}, panel = {}",
            l.panel_height()
        );
    }

    #[test]
    fn the_drop_zone_stops_at_the_rope() {
        // The band ends where the rope is, not where the cards end. Measured on
        // the user's 2560 px @100% screen with sag 1.5%: the rope's deepest
        // point sits at y=64 and the cards draw down to y=204.
        let l = LineLayout::with_sag(2560.0, 1.0, 1.5);
        let rope_low = l.rope_y(1280.0);
        assert!(
            (l.hang_zone() - (rope_low + CARD_TOP_GAP)).abs() < 0.01,
            "zone = {}, rope = {rope_low}",
            l.hang_zone()
        );
        assert!(
            l.hang_zone() < rope_low + CARD_H,
            "zone = {} must not reach the bottom of the cards",
            l.hang_zone()
        );
        assert_eq!(l.hang_depth(), 86);
        // The strip itself has to contain the band, or the last row of the drop
        // zone would be an invisible band below the picture.
        assert!(l.hang_zone() < l.panel_height());
        // A curve the user dials down moves the rope and the band together.
        let deep = LineLayout::with_sag(2560.0, 1.0, 16.0);
        assert!(
            deep.hang_zone() > l.hang_zone(),
            "zone = {}",
            deep.hang_zone()
        );
        assert!(deep.hang_zone() < deep.panel_height());
    }

    #[test]
    fn a_window_below_the_rope_is_cold() {
        // Reversal of the "band = the whole clothesline" decision, taken because
        // the wider band was grabbing windows he was only moving: on his 2560 px
        // @100% screen, releases at top=171/187/199/203 lit up and hung with the
        // rope 135 px above the window's top edge.
        let l = LineLayout::with_sag(2560.0, 1.0, 1.5);
        for top in [171, 187, 199, 203] {
            assert!(!l.window_on_line(top, 0), "top={top} is below the rope");
        }
        // The window's top edge meeting the rope is the trigger, and the
        // clothespin gap is the tolerance that keeps that from meaning "hit the
        // pixel".
        assert!(l.window_on_line(64, 0), "the deepest point of the rope");
        assert!(l.window_on_line(l.hang_depth(), 0));
        assert!(!l.window_on_line(l.hang_depth() + 1, 0));
    }

    #[test]
    fn the_line_answers_to_the_window_and_not_to_the_pointer() {
        let l = LineLayout::new(2560.0, 1.0);
        // A window whose top edge is still a third of the screen down has not
        // met the rope, however high the pointer is.
        assert!(!l.window_on_line(400, 0));
        assert!(l.window_on_line(60, 0));
        assert!(l.window_on_line(l.hang_depth(), 0));
        assert!(!l.window_on_line(l.hang_depth() + 1, 0));
        // Just clear of the maximize strip and the line answers again. The dead
        // zone has to stay this narrow: a window resting anywhere from 1 to
        // 74 px covers the rope, and refusing all of it is refusing to hang.
        assert!(l.window_on_line(3, 0));
        assert!(!l.window_on_line(1, 0));
        // Where the user feels the contact is the rope itself. A yield wide
        // enough to swallow it reads as "the line goes cold just as the window
        // touches it", which is the whole gesture failing.
        assert!(l.window_on_line((ROPE_TOP * l.scale) as i32, 0));
        let hidpi = LineLayout::new(2560.0, 2.0);
        assert!(hidpi.window_on_line((ROPE_TOP * hidpi.scale) as i32, 0));
        // A curve the user dials up reaches further down.
        let deep = LineLayout::with_sag(2560.0, 1.0, 16.0);
        assert!(
            deep.window_on_line(l.hang_depth() + 1, 0),
            "depth = {}",
            deep.hang_depth()
        );
    }

    #[test]
    fn the_top_of_the_screen_belongs_to_windows_maximize() {
        let l = LineLayout::new(2560.0, 1.0);
        assert!(l.window_flush(0, 0));
        assert!(l.window_flush(2, 0));
        assert!(!l.window_flush(5, 0));
        // Pushed past the edge at all, or on a second monitor below the primary.
        assert!(l.window_flush(-200, 0));
        assert!(l.window_flush(901, 900));
        assert!(!l.window_flush(905, 900));
        // In device pixels, so a scaled display gets a proportionally wider
        // strip - and nothing more.
        let hidpi = LineLayout::new(2560.0, 2.0);
        assert!(hidpi.window_flush(3, 0));
        assert!(!hidpi.window_flush(6, 0));
    }

    #[test]
    fn dropping_a_card_past_the_ends_lands_it_on_the_last_slot() {
        let l = LineLayout::new(1920.0, 1.0);
        assert_eq!(l.index_at(l.slot_x(1, 3), 3), 1);
        assert_eq!(l.index_at(-4000.0, 3), 0);
        assert_eq!(l.index_at(4000.0, 3), 2);
        assert_eq!(l.index_at(960.0, 0), 0);
    }
}
