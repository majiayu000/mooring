//! What a terminal pane paints over or around its grid.
use super::screen::{self, CellMetrics};
use crate::ui::{
    terminal_text::{CursorDefault, TerminalText, padding_of},
    theme::{Theme as T, hsla},
};
use fibo_core::terminal::libghostty_vt::terminal::CursorStyle;
use gpui::{Bounds, Pixels, Window, px};

pub(super) fn cursor_style(shape: CursorDefault) -> CursorStyle {
    match shape {
        CursorDefault::Block => CursorStyle::Block,
        CursorDefault::Bar => CursorStyle::Bar,
        CursorDefault::Underline => CursorStyle::Underline,
    }
}
/// How the cursor is drawn this frame: none under text being composed, blinking when
/// focused, and as chosen in a split without focus.
pub(super) fn cursor_paint(
    look: &TerminalText,
    no_marked_text: bool,
    focused: bool,
    blink_on: bool,
) -> screen::CursorPaint {
    use crate::ui::terminal_text::UnfocusedCursor;
    match (no_marked_text, focused, blink_on, look.unfocused_cursor) {
        (false, ..) | (true, true, false, _) | (true, false, _, UnfocusedCursor::Hidden) => {
            screen::CursorPaint::Hidden
        }
        (true, true, true, _) => screen::CursorPaint::Shown,
        (true, false, _, UnfocusedCursor::Hollow) => screen::CursorPaint::Hollow,
    }
}
/// The stronger of the two fades. Ghostty's unfocused-split-opacity: the program's own
/// cursor (Claude and Codex draw one as reversed text) recedes with the rest.
pub(super) fn dim(look: &TerminalText, unfocused_split: bool, background_window: bool) -> f32 {
    let split = if unfocused_split {
        look.unfocused_dim
    } else {
        0.
    };
    let window = if background_window {
        look.window_dim
    } else {
        0.
    };
    split.max(window)
}
/// How far a peek taller than its overlay slides up: whole rows, so the top row is not
/// cut in half under the title bar, and enough of them that the bottom row stays in view.
pub(super) fn peek_lift(rows: usize, row: Pixels, shown: Pixels) -> Pixels {
    let over = f32::from(row * rows as f32 - shown);
    if over <= 0. || row <= px(0.) {
        return px(0.);
    }
    row * (over / f32::from(row)).ceil()
}
/// Where the cells go inside the pane, the margin painted as the terminal's own ground. A
/// peek keeps the terminal's exact size, so it has no margin.
pub(super) fn inset(
    bounds: Bounds<Pixels>,
    sc: &screen::Screen,
    peek: bool,
    window: &mut Window,
) -> Bounds<Pixels> {
    let padding = padding_of();
    if peek || padding <= 0. {
        return bounds;
    }
    if T::palette().opacity == 100 && T::palette().backdrop.is_none() {
        window.paint_quad(gpui::fill(bounds, hsla(sc.background)));
    }
    let margin = px(padding);
    Bounds::new(
        bounds.origin + gpui::point(margin, margin),
        gpui::size(
            (bounds.size.width - margin * 2.).max(px(1.)),
            (bounds.size.height - margin * 2.).max(px(1.)),
        ),
    )
}
pub(super) fn selection(
    sc: &screen::Screen,
    bounds: Bounds<Pixels>,
    metrics: CellMetrics,
    window: &mut Window,
) {
    for (row, cols) in sc.selection.iter().enumerate() {
        let Some((left, right)) = *cols else { continue };
        window.paint_quad(gpui::fill(
            Bounds::new(
                gpui::point(
                    bounds.left() + metrics.width * left as f32,
                    bounds.top() + metrics.height * row as f32,
                ),
                gpui::size(
                    metrics.width * (right + 1).saturating_sub(left) as f32,
                    metrics.height,
                ),
            ),
            T::terminal_selection(),
        ));
    }
}
/// Matches in view, the current one stronger; drawn over the text, so translucent, the
/// current one also outlined. The amber T::matched() washes into the popover; over
/// terminal text the pure hue is needed to stay visible in both schemes.
pub(super) fn matches(
    found: &[fibo_core::terminal::SearchMatch],
    current: Option<usize>,
    (top, len): (usize, usize),
    bounds: Bounds<Pixels>,
    metrics: CellMetrics,
    window: &mut Window,
) {
    let matched = hsla(T::SEARCH_HIT);
    for (i, m) in found.iter().enumerate() {
        if m.row < top || m.row >= top + len {
            continue;
        }
        let is_current = current == Some(i);
        let at = Bounds::new(
            gpui::point(
                bounds.left() + metrics.width * m.col as f32,
                bounds.top() + metrics.height * (m.row - top) as f32,
            ),
            gpui::size(metrics.width * m.cols as f32, metrics.height),
        );
        window.paint_quad(gpui::fill(
            at,
            matched.opacity(if is_current { 0.55 } else { 0.25 }),
        ));
        if is_current {
            window.paint_quad(gpui::outline(at, matched, gpui::BorderStyle::Solid));
        }
    }
}

/// A shown cell with text: where its middle is in the window and its color.
pub type Glyph = (gpui::Point<Pixels>, u32);

/// What 标记完成's dissolve reads of the grid.
impl super::Pane {
    /// The window point at the middle of each shown cell with text, with its color, row
    /// by row. For 标记完成's dissolve, which turns the text into specks.
    pub fn glyph_cells(&self) -> Vec<Glyph> {
        let (Some(bounds), Some(m)) = (self.bounds, self.metrics) else {
            return vec![];
        };
        let mut cells = Vec::new();
        for (row, line) in self.screen.cells.iter().enumerate() {
            let runs = self.screen.rows.get(row).map(Vec::as_slice).unwrap_or(&[]);
            for &(col, width, ch) in line {
                if ch.is_whitespace() || ch == '\0' {
                    continue;
                }
                let fg = runs
                    .iter()
                    .find(|r| col >= r.col && col < r.col + r.cols)
                    .map_or(T::inverse(self.screen.background), |r| r.fg);
                let x = bounds.left() + m.width * (col as f32 + width.max(1) as f32 / 2.);
                let y = bounds.top() + m.height * (row as f32 + 0.5);
                cells.push((gpui::point(x, y), fg));
            }
        }
        cells
    }
    /// Every character on screen in reading order, for the styles that move the text.
    pub fn text_cells(&self) -> Vec<super::super::glyph::TextCell> {
        let (Some(bounds), Some(m)) = (self.bounds, self.metrics) else {
            return vec![];
        };
        super::super::glyph::cells_of(&self.screen.rows, |col, row| {
            gpui::point(
                bounds.left() + m.width * col as f32,
                bounds.top() + m.height * row as f32,
            )
        })
    }
    /// The first line of an ask, as a pane shows it.
    pub(super) fn ask_first_line(&self, i: usize) -> Option<String> {
        self.ask
            .asks
            .get(i)
            .and_then(|a| a.lines().map(str::trim).find(|l| !l.is_empty()))
            .map(|l| fibo_core::content::short(l, 200))
    }
    /// The bar's words and the row they scroll back to, when the bar is showing.
    pub(super) fn ask_bar_line(&self, cx: &gpui::App) -> Option<(String, Option<usize>)> {
        self.ask
            .pinned
            .latest
            .filter(|_| crate::ui::pinned_ask::current(cx).bar)
            .and_then(|(i, row)| Some((self.ask_first_line(i)?, row)))
    }
    /// The mark the bar puts before an ask, as the CLI marks a sent one.
    pub(super) fn ask_mark(&self) -> String {
        self.prompt_line()
            .and_then(|p| p.markers.first())
            .map_or("›".to_owned(), Clone::clone)
    }
    /// The bar along the terminal's top, for what takes over from it when the text moves.
    pub fn ask_bar(&self, cx: &gpui::App) -> Option<super::super::glyph::AskBar> {
        let (text, row) = self.ask_bar_line(cx)?;
        let (grid, m) = (self.bounds?, self.metrics?);
        let margin = px(if self.peek { 0. } else { padding_of().max(0.) });
        Some(super::super::glyph::AskBar {
            area: Bounds::new(
                grid.origin - gpui::point(margin, margin),
                gpui::size(
                    grid.size.width + margin * 2. - px(super::SCROLLBAR_HIT),
                    m.height,
                ),
            ),
            mark: self.ask_mark(),
            text,
            back: row.is_some(),
        })
    }
    /// The cell's size and offsets, for drawing a character as the grid does.
    pub fn cell_metrics(&self) -> Option<CellMetrics> {
        self.metrics
    }
    /// Where the grid was last painted, its row height and its ground.
    pub fn grid_area(&self) -> Option<(gpui::Bounds<Pixels>, Pixels, u32)> {
        Some((self.bounds?, self.metrics?.height, self.screen.background))
    }
}
#[cfg(test)]
mod peek_lift_tests {
    use super::peek_lift;
    use gpui::px;
    #[test]
    fn a_tall_peek_slides_by_whole_rows_and_keeps_the_bottom_row() {
        // 40 rows of 20px in a 590px overlay: 210px over, so 11 rows go above it.
        assert_eq!(peek_lift(40, px(20.), px(590.)), px(220.));
        assert_eq!(peek_lift(40, px(20.), px(600.)), px(200.));
        assert_eq!(peek_lift(20, px(20.), px(590.)), px(0.));
    }
}
