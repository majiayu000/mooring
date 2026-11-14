//! Drawing a pane: the grid and everything over and around it. Runs every frame.
use super::paste::{copied_note, line_count};
use super::{ASK_FLOAT_MS, Pane, SCROLLBAR_WIDTH, overlays};
use crate::{
    terminal::screen,
    ui::{theme::Theme as T, tone::Tone},
};
use fibo_core::{protocol::Request, terminal::libghostty_vt::terminal::ScrollViewport};
use gpui::{Bounds, Context, ElementInputHandler, Pixels, Window, canvas, div, prelude::*, px};
use std::{sync::atomic::Ordering, time::Instant};

impl Pane {
    /// Repaint this terminal's defaults after a theme switch. The display copy changes
    /// here; the host's copy, which programs query, only from the writing pane.
    fn sync_scheme(&mut self) {
        let scheme = T::term();
        let shape = crate::ui::terminal_text::cursor_shape();
        let writable = {
            let Ok(mut state) = self.connection.state.lock() else {
                return;
            };
            // Ghostty applies it at once unless the program has asked for a shape.
            if state.cursor_shape != Some(shape)
                && let Some(vt) = state.vt.as_mut()
                && vt.set_default_cursor(overlays::cursor_style(shape)).is_ok()
            {
                state.cursor_shape = Some(shape);
            }
            if state.scheme == Some(scheme) {
                return;
            }
            let Some(vt) = state.vt.as_mut() else {
                return;
            };
            if let Err(e) = vt.set_colors(scheme) {
                self.local_error = Some(e.to_string());
                return;
            }
            state.scheme = Some(scheme);
            state.writable && !state.ended
        };
        if writable {
            let _ = self.connection.send(crate::lifecycle::scheme_request());
        }
    }
}
impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_observers {
            self.focus_observers = true;
            cx.on_focus(&self.focus, window, Self::sync_focus).detach();
            cx.on_blur(&self.focus, window, Self::sync_focus).detach();
            cx.observe_window_activation(window, Self::sync_focus)
                .detach();
        }
        self.sync_focus(window, cx);
        self.refresh_cwd(cx);
        self.connection.dirty.store(false, Ordering::Release);
        self.sync_scheme();
        let shared = self.connection.state.clone();
        let state = shared.lock().unwrap();
        if let Some(vt) = &state.vt {
            let result = (|| {
                if self.capture.is_none() {
                    self.capture = Some(screen::Capture::new()?);
                }
                self.capture.as_mut().unwrap().update(vt, &mut self.screen)
            })();
            self.scrolled_back = !vt.terminal().viewport_active().unwrap_or(true);
            let top = self.viewport.0;
            self.viewport = vt.viewport().unwrap_or(self.viewport);
            if self.scrolled_back && self.viewport.0 != top {
                self.ask.scrolled_at = Some(Instant::now());
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(ASK_FLOAT_MS + 50))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
            self.refresh_pinned(cx);
            match result {
                Ok(_) => {
                    self.cursor_anchor.observe(self.screen.cursor);
                }
                Err(e) => self.local_error = Some(e.to_string()),
            }
        } else if self.capture.is_none() {
            // Nothing captured yet: the ground is the theme's, the same one the snapshot
            // brings, rather than the zero an empty screen holds, which paints black.
            self.screen.background = T::terminal_bg();
        }
        // While reattaching, the status line says so; the broken stream's error is noise.
        let error = self
            .local_error
            .clone()
            .or(state.error.clone())
            .filter(|_| !state.dropped && self.reconnect_delay == 0);
        if self.started_under.is_none() && state.vt.is_some() && !state.ended {
            self.started_under = Some(T::term());
        }
        let tint_stale = self.agent.is_some()
            && !state.ended
            && !self.tint_note_dismissed
            && self.started_under.is_some_and(|s| s != T::term());
        let status = if state.ended {
            t!("进程已退出")
        } else if state.dropped || self.reconnect_delay > 0 {
            t!("连接断开 · 正在重连…")
        } else if state.vt.is_none() {
            t!("正在恢复终端…")
        } else if state.writable {
            t!("终端已连接")
        } else {
            t!("只读 · 可明确接管")
        };
        let writable = state.writable;
        drop(state);
        // OSC 52: the program copied something. Only the writable client owns the clipboard.
        if let Some(text) = self
            .connection
            .clipboard
            .lock()
            .ok()
            .and_then(|mut c| c.take())
            && writable
        {
            // The program copied on its own (e.g. an Agent's /copy); say so, or the
            // user can't tell anything happened.
            let note = t!("程序{v1}", v1 = copied_note(&text, false));
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            self.show_toast(note, cx);
        }
        let entity = cx.entity();
        let content = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let drags = entity.clone();
                window.on_mouse_event(move |e: &gpui::MouseMoveEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Bubble {
                        drags.update(cx, |pane, cx| pane.drag_selection(e, cx));
                    }
                });
                entity.update(cx, |pane, cx| {
                    // A peek draws at the terminal's own size; one taller than the overlay
                    // slides up so the prompt at its bottom stays in view.
                    let bounds = match (pane.peek, pane.metrics) {
                        (true, Some(m)) => {
                            let rows = pane.screen.rows.len();
                            let over = overlays::peek_lift(rows, m.height, bounds.size.height);
                            Bounds::new(bounds.origin - gpui::point(px(0.), over), bounds.size)
                        }
                        _ => bounds,
                    };
                    let bounds = overlays::inset(bounds, &pane.screen, pane.peek, window);
                    // The grid starts on a backing pixel so its pixel-wide cells keep
                    // every glyph stem on whole pixels.
                    let scale = window.scale_factor();
                    let snap = |v: Pixels| px((f32::from(v) * scale).round() / scale);
                    let bounds = Bounds::new(
                        gpui::point(snap(bounds.origin.x), snap(bounds.origin.y)),
                        bounds.size,
                    );
                    pane.bounds = Some(bounds);
                    pane.scale = scale;
                    window.handle_input(
                        &pane.focus,
                        ElementInputHandler::new(bounds, cx.entity()),
                        cx,
                    );
                    let font = T::terminal_mono(cx);
                    // Cells are rounded to backing pixels, so a move to a display with
                    // another scale measures again.
                    let key = (
                        font.clone(),
                        screen::font_size(),
                        crate::ui::terminal_text::line_height(),
                        scale,
                    );
                    if pane.metrics_font.as_ref() != Some(&key) || pane.metrics.is_none() {
                        pane.metrics = screen::CellMetrics::measure(&font, window);
                        pane.metrics_font = Some(key);
                    }
                    let Some(metrics) = pane.metrics else { return };
                    let focused = pane.focus.is_focused(window);
                    let look = crate::ui::terminal_text::current();
                    let cursor = overlays::cursor_paint(
                        &look,
                        pane.marked.is_empty(),
                        focused,
                        pane.blink_on,
                    );
                    screen::paint_cached(
                        &pane.screen,
                        &font,
                        bounds,
                        metrics,
                        cursor,
                        &mut pane.layout_cache,
                        window,
                        cx,
                    );
                    if look.cursor_guide && focused {
                        screen::paint_guide(&pane.screen, bounds, metrics, window);
                    }
                    // Underlined only while ⌘ is held, when a click would open it.
                    let hovered: Vec<&screen::Link> = match pane.links.hover_link {
                        _ if !pane.links.modifiers.secondary() => Vec::new(),
                        Some((y, x)) => pane.screen.link_at(y, x).map_or(Vec::new(), |hover| {
                            pane.screen
                                .links
                                .iter()
                                .filter(|l| l.uri == hover.uri)
                                .collect()
                        }),
                        None => match (pane.shown_target(), pane.links.hover_word.as_ref()) {
                            (Some(target), Some(word)) if target.whole => pane
                                .screen
                                .paths
                                .iter()
                                .filter(|l| l.start == word.start && l.uri == word.uri)
                                .collect(),
                            (Some(_), Some(word)) => vec![word],
                            _ => Vec::new(),
                        },
                    };
                    for l in hovered {
                        window.paint_quad(gpui::fill(
                            Bounds::new(
                                gpui::point(
                                    bounds.left() + metrics.width * l.col as f32,
                                    bounds.top() + metrics.height * (l.row + 1) as f32
                                        - metrics.stroke,
                                ),
                                gpui::size(metrics.width * l.cols as f32, metrics.stroke),
                            ),
                            crate::ui::theme::hsla(
                                T::palette().term_link.unwrap_or(T::terminal_fg()),
                            ),
                        ));
                    }
                    let (top, total, len) = pane.viewport;
                    overlays::matches(
                        &pane.search.matches,
                        pane.search.current_match,
                        (top, len),
                        bounds,
                        metrics,
                        window,
                    );
                    if pane.scrollbar_shown() {
                        let h = bounds.size.height;
                        let thumb = (h * (len as f32 / total as f32)).max(px(T::S20));
                        let y = (h - thumb) * (top as f32 / (total - len).max(1) as f32);
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(
                                    gpui::point(
                                        bounds.right() - px(SCROLLBAR_WIDTH + T::S2),
                                        bounds.top() + y,
                                    ),
                                    gpui::size(px(SCROLLBAR_WIDTH), thumb),
                                ),
                                T::color(T::secondary()).opacity(0.6),
                            )
                            .corner_radii(px(SCROLLBAR_WIDTH / 2.)),
                        );
                    }
                    overlays::selection(&pane.screen, bounds, metrics, window);
                    screen::paint_selected_text(
                        &pane.screen,
                        &font,
                        bounds,
                        metrics,
                        &pane.layout_cache,
                        window,
                        cx,
                    );
                    let dim =
                        overlays::dim(&look, pane.split && !focused, !window.is_window_active());
                    if dim > 0. {
                        window.paint_quad(gpui::fill(
                            bounds,
                            crate::ui::theme::hsla(pane.screen.background).opacity(dim),
                        ));
                    }
                    if !pane.marked.is_empty()
                        && let Some(cursor) = pane.cursor_anchor.get()
                    {
                        screen::MarkedText::new(
                            &pane.marked,
                            font,
                            metrics.cursor_bounds(bounds, cursor),
                            window,
                        )
                        .paint(pane.screen.background, window, cx);
                    }
                    let dims = (
                        (bounds.size.width / metrics.width).floor().clamp(2., 400.) as u16,
                        (bounds.size.height / metrics.height)
                            .floor()
                            .clamp(1., 200.) as u16,
                    );
                    let scale = window.scale_factor();
                    let cell = (
                        (f32::from(metrics.width) * scale).round().max(1.) as u32,
                        (f32::from(metrics.height) * scale).round().max(1.) as u32,
                    );
                    if !pane.peek
                        && (pane.requested_size, pane.requested_cell) != (dims, cell)
                        && pane
                            .connection
                            .send(Request::Resize {
                                cols: dims.0,
                                rows: dims.1,
                                cell_width: cell.0,
                                cell_height: cell.1,
                            })
                            .is_ok()
                    {
                        pane.requested_size = dims;
                        pane.requested_cell = cell;
                    }
                });
            },
        )
        .size_full();
        let mut view = div()
            .id("terminal")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(T::surface(T::pane()))
            .track_focus(&self.focus)
            // Right-click is never reported to the program: it opens Fibo's pane
            // menu even when a full-screen Agent has taken the mouse.
            .on_mouse_down(gpui::MouseButton::Left,cx.listener(Self::mouse_down))
            .on_mouse_down(gpui::MouseButton::Middle,cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_modifiers_changed(cx.listener(|this, e: &gpui::ModifiersChangedEvent, _, cx| {
                this.set_modifiers(e.modifiers, cx)
            }))
            .on_mouse_up(gpui::MouseButton::Left,cx.listener(Self::mouse_up))
            .on_mouse_up(gpui::MouseButton::Middle,cx.listener(Self::mouse_up))
            .on_mouse_up_out(gpui::MouseButton::Left,cx.listener(Self::mouse_up))
            .on_mouse_up_out(gpui::MouseButton::Middle,cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_key_down(cx.listener(Self::key))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                window.focus(&this.focus, cx);
                this.drop_paths(paths, cx);
            }))
            .when(status != t!("终端已连接"), |d| {
                d.child(
                    div()
                        .px(px(T::S12))
                        .py(px(T::S4))
                        .text_size(px(T::META))
                        .text_color(T::color(T::secondary()))
                        .child(status),
                )
            })
            .when_some(self.paste_confirm.as_deref().map(line_count),|d,lines| {
                d.child(div().px(px(T::S12)).py(px(T::S8)).flex().items_center().gap(px(T::S12))
                    .bg(T::color(T::panel())).text_size(px(T::META))
                    .child(div().flex_1().text_color(T::color(T::secondary()))
                        .child(t!("这个程序没有启用安全粘贴：要粘贴的 {lines} 行会逐行立即执行。", lines)))
                    .child(div().id("paste-anyway").cursor_pointer().text_color(T::color(T::link())).child(t!("仍然粘贴 {lines} 行", lines))
                        .on_click(cx.listener(|this,_,_,cx|this.confirm_paste(cx))))
                    .child(div().id("paste-cancel").cursor_pointer().child(t!("取消"))
                        .on_click(cx.listener(|this,_,_,cx|{this.paste_confirm=None;cx.notify();}))))
            })
            .when(tint_stale, |d| {
                d.child(
                    div()
                        .px(px(T::S12))
                        .py(px(T::S4))
                        .flex()
                        .items_center()
                        .gap(px(T::S12))
                        .text_size(px(T::META))
                        .child(div().flex_1().text_color(T::color(T::secondary())).child(
                            t!("这个终端在切换外观前就开着，程序自己画的底色要退出重开（如 codex resume）才会跟上。"),
                        ))
                        .child(
                            div()
                                .id("tint-note-dismiss")
                                .cursor_pointer()
                                .text_color(T::color(T::link()))
                                .child(t!("知道了"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.tint_note_dismissed = true;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.search.field.clone(), |d, field| {
                let count = self.search.matches.len();
                let at = self.search.current_match.map_or(0, |i| i + 1);
                let link = |id: &'static str, text: &'static str| {
                    div().id(id).cursor_pointer().text_color(T::color(T::link())).child(text)
                };
                let mut panel = div()
                    .p(px(T::S8))
                    .flex()
                    .flex_col()
                    .gap(px(T::S4))
                    .child(field)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(T::S12))
                            .child(link("find", t!("查找")).on_click(cx.listener(|this, _, _, cx| this.find_text(cx))))
                            .when(count > 0, |d| {
                                d.child(link("prev-match", t!("↑ 上一个")).on_click(cx.listener(|this, _, _, cx| this.step_match(-1, cx))))
                                    .child(link("next-match", t!("↓ 下一个")).on_click(cx.listener(|this, _, _, cx| this.step_match(1, cx))))
                            })
                            .child(div().text_color(T::color(T::secondary())).child(if self.search.searched.is_none() {
                                t!("按回车查找").to_owned()
                            } else if count == 0 {
                                t!("没有匹配").to_owned()
                            } else {
                                t!("第 {at} / {count} 处", at, count)
                            }))
                            .child(div().flex_1())
                            .child(link("close-find", t!("关闭搜索")).on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx)))),
                    );
                // The newest 20 matches, newest first; clicking one goes to it.
                for (i, m) in self.search.matches.iter().enumerate().rev().take(20) {
                    let current = self.search.current_match == Some(i);
                    panel = panel.child(
                        div()
                            .id(gpui::SharedString::from(format!("match-{i}")))
                            .flex_shrink_0()
                            .truncate()
                            .cursor_pointer()
                            .when(current, |d| d.text_color(T::color(T::primary())).font_weight(gpui::FontWeight::SEMIBOLD))
                            .when(!current, |d| d.text_color(T::color(T::secondary())))
                            .child(gpui::SharedString::from(&m.line))
                            .on_click(cx.listener(move |this, _, _, cx| this.show_match(i, cx))),
                    );
                }
                d.child(panel.id("search-results").max_h(px(T::SEARCH_HEIGHT)).overflow_y_scroll())
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(content)
                    .children(self.ask_overlays(cx))
                    .when(self.notes.mouse_hint(), |d| {
                        // Along the top: a program's own status lines, such as Codex's
                        // "Copied", sit at the bottom, and so does Fibo's copied note.
                        d.child(
                            div()
                                .absolute()
                                .right(px(T::S20))
                                .top(px(T::S8))
                                .px(px(T::S8))
                                .py(px(T::S4))
                                .rounded(px(T::CONTROL_RADIUS))
                                .bg(T::color(T::raised()))
                                .border(px(T::BORDER_WIDTH))
                                .border_color(T::color(T::card_border()))
                                .text_size(px(T::META))
                                .text_color(T::color(T::secondary()))
                                .child(super::notes::MOUSE_HINT),
                        )
                    })
                    .when(self.scrolled_back || self.notes.copied().is_some(), |d| {
                        // Floats over the grid so it never resizes the terminal; clear of
                        // the scrollbar along the right edge. A toast stacks above the
                        // "回到底部" button instead of covering it.
                        let toast = self.notes.copied().map(|toast| {
                            div()
                                .px(px(T::S8))
                                .py(px(T::S4))
                                .rounded(px(T::CONTROL_RADIUS))
                                .bg(T::color(T::raised()))
                                .border(px(T::BORDER_WIDTH))
                                .border_color(T::color(T::card_border()))
                                .text_size(px(T::META))
                                .text_color(T::color(T::secondary()))
                                .child(toast)
                        });
                        let bottom = self.scrolled_back.then(|| {
                            div()
                                .id("scroll-to-bottom")
                                .px(px(T::S8))
                                .py(px(T::S4))
                                .rounded(px(T::CHIP_RADIUS))
                                .bg(T::color(T::accent()))
                                .text_size(px(T::META))
                                .text_color(T::color(T::on_accent()))
                                .cursor_pointer()
                                .child(t!("回到底部 ↓"))
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.scroll_to(ScrollViewport::Bottom, cx);
                                    window.focus(&this.focus, cx);
                                }))
                        });
                        d.child(
                            div()
                                .absolute()
                                .right(px(T::S20))
                                .bottom(px(T::S8))
                                .flex()
                                .flex_col()
                                .items_end()
                                .gap(px(T::S4))
                                .children(toast)
                                .children(bottom),
                        )
                    }),
            )
            .when_some(self.click_hint().zip(self.links.pointer), |d, (hint, at)| {
                // Below and right of the pointer, so the word itself stays readable.
                d.child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(at + gpui::point(px(T::S12), px(T::S16)))
                            .snap_to_window_with_margin(px(T::S8))
                            .child(
                                div()
                                    .id("click-hint")
                                    .px(px(T::S8))
                                    .py(px(T::S2))
                                    .rounded(px(T::CONTROL_RADIUS))
                                    .bg(T::color(T::raised()))
                                    .border(px(T::BORDER_WIDTH))
                                    .border_color(T::color(T::card_border()))
                                    .text_size(px(T::META))
                                    .text_color(T::color(T::secondary()))
                                    .child(hint),
                            ),
                    )
                    .priority(2),
                )
            })
            .when(self.flash, |d| {
                d.relative().child(
                    div()
                        .absolute()
                        .inset_0()
                        .border(px(T::S2))
                        .border_color(T::color(T::accent())),
                )
            });
        if let Some(error) = error {
            view = view.child(
                div()
                    .px(px(T::S12))
                    .py(px(T::S4))
                    .text_size(px(T::META))
                    .text_color(Tone::Danger.ink())
                    .child(error),
            );
        }
        view
    }
}
