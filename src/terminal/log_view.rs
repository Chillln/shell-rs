//! 在现有 SSH 连接上定时读取日志，仅在日志页可见时运行。

use std::{rc::Rc, time::Duration};

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::*;

use super::{TerminalView, exec_answer};
use crate::app::ControlLog;
use crate::i18n::{UiLocale, t};
use crate::shared::{Segment, SegmentedControl};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const LOG_TIMEOUT: Duration = Duration::from_secs(60);
const RANGES: [u64; 4] = [0, 900, 3600, 86400];

/// 日志工具栏的命令，统一由工作区处理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogControl {
    Refresh,
    TogglePause,
    ToggleScroll,
    Range(usize),
}

/// Docker 和系统服务共用的日志视图；关闭或隐藏时取消刷新任务。
pub struct LogView {
    id: &'static str,
    terminal: WeakEntity<TerminalView>,
    command: Rc<dyn Fn(u64) -> Option<String>>,
    dispatch: FocusHandle,
    search: Entity<InputState>,
    query: String,
    output: Option<String>,
    problem: Option<String>,
    visible: bool,
    paused: bool,
    auto_scroll: bool,
    range: usize,
    scroll: ScrollHandle,
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LogView {
    pub fn new(
        id: &'static str,
        terminal: WeakEntity<TerminalView>,
        dispatch: FocusHandle,
        command: impl Fn(u64) -> Option<String> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("logs.search"))
                .clean_on_escape()
        });
        let subscription = cx.subscribe(&search, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_lowercase();
                if this.auto_scroll {
                    this.scroll.scroll_to_bottom();
                }
                cx.notify();
            }
        });
        Self {
            id,
            terminal,
            command: Rc::new(command),
            dispatch,
            search,
            query: String::new(),
            output: None,
            problem: None,
            visible: false,
            paused: false,
            auto_scroll: true,
            range: 0,
            scroll: ScrollHandle::new(),
            task: None,
            _subscriptions: vec![
                subscription,
                cx.observe_global_in::<UiLocale>(window, |this, window, cx| {
                    this.search.update(cx, |input, cx| {
                        input.set_placeholder(t!("logs.search"), window, cx);
                    });
                }),
            ],
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.task = None;
        if visible && !self.paused {
            self.refresh(cx);
        }
        cx.notify();
    }

    pub fn control(&mut self, control: LogControl, cx: &mut Context<Self>) {
        match control {
            LogControl::Refresh => self.refresh(cx),
            LogControl::TogglePause => {
                self.paused = !self.paused;
                self.task = None;
                if !self.paused && self.visible {
                    self.refresh(cx);
                }
            }
            LogControl::ToggleScroll => {
                self.auto_scroll = !self.auto_scroll;
                if self.auto_scroll {
                    self.scroll.scroll_to_bottom();
                }
            }
            LogControl::Range(ix) if ix < RANGES.len() => {
                self.range = ix;
                self.refresh(cx);
            }
            LogControl::Range(_) => return,
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        if !self.visible {
            return;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            loop {
                let reply = this.update(cx, |this, cx| {
                    let command = (this.command)(RANGES[this.range])?;
                    this.terminal
                        .upgrade()?
                        .read(cx)
                        .exec_with_timeout(command, LOG_TIMEOUT, cx)
                });
                let answer = match reply {
                    Ok(Some(reply)) => exec_answer(reply, cx).await,
                    Ok(None) => Some(Err(t!("tools.not_connected").into())),
                    Err(_) => break,
                }
                .unwrap_or_else(|| Err(t!("tools.no_answer").into()));
                let keep_reading = this.update(cx, |this, cx| {
                    match answer {
                        Ok(output) => {
                            this.output = Some(output.trim_end().to_owned());
                            this.problem = None;
                            if this.auto_scroll {
                                this.scroll.scroll_to_bottom();
                            }
                        }
                        Err(why) => this.problem = Some(why),
                    }
                    cx.notify();
                    this.visible && !this.paused && this.problem.is_none()
                });
                if !matches!(keep_reading, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(REFRESH_INTERVAL).await;
            }
        }));
    }

    fn action(&self, control: LogControl, cx: &Context<Self>) -> ControlLog {
        ControlLog::new(cx.entity().downgrade(), control)
    }
}

impl Render for LogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pause = self.action(LogControl::TogglePause, cx);
        let refresh = self.action(LogControl::Refresh, cx);
        let scroll = self.action(LogControl::ToggleScroll, cx);
        let dispatch_pause = self.dispatch.clone();
        let dispatch_refresh = self.dispatch.clone();
        let dispatch_scroll = self.dispatch.clone();
        let dispatch_range = self.dispatch.clone();
        let view = cx.entity().downgrade();
        let output = self.output.as_deref().unwrap_or_default();
        let filtered = filter_lines(output, &self.query);
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.search).id("log-search").small())
                    .child(
                        Button::new("log-pause")
                            .outline()
                            .small()
                            .label(if self.paused {
                                t!("logs.resume")
                            } else {
                                t!("logs.pause")
                            })
                            .on_click(move |_, window, cx| {
                                dispatch_pause.dispatch_action(&pause, window, cx)
                            }),
                    )
                    .child(
                        Button::new("log-refresh")
                            .ghost()
                            .small()
                            .label(t!("tools.refresh"))
                            .on_click(move |_, window, cx| {
                                dispatch_refresh.dispatch_action(&refresh, window, cx)
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        SegmentedControl::new("log-range")
                            .small()
                            .selected_index(Some(self.range))
                            .segments([
                                Segment::new(t!("logs.recent")),
                                Segment::new(t!("logs.minutes")),
                                Segment::new(t!("logs.hour")),
                                Segment::new(t!("logs.day")),
                            ])
                            .on_change(move |ix, window, cx| {
                                dispatch_range.dispatch_action(
                                    &ControlLog::new(view.clone(), LogControl::Range(*ix)),
                                    window,
                                    cx,
                                )
                            }),
                    )
                    .child(
                        Checkbox::new("log-auto-scroll")
                            .checked(self.auto_scroll)
                            .label(t!("logs.auto_scroll"))
                            .on_click(move |_, window, cx| {
                                dispatch_scroll.dispatch_action(&scroll, window, cx)
                            }),
                    ),
            )
            .children(self.problem.as_ref().map(|why| {
                div()
                    .id("log-error")
                    .test_support()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(why.clone())
            }))
            .child(
                div()
                    .id(self.id)
                    .test_support()
                    .aria_label(filtered.clone())
                    .h(rems(20.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(if self.output.is_none() && self.problem.is_none() {
                        t!("tools.reading").to_string()
                    } else if filtered.is_empty() {
                        t!("logs.empty").to_string()
                    } else {
                        filtered
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("logs.interval")),
            )
    }
}

fn filter_lines(output: &str, query: &str) -> String {
    output
        .lines()
        .filter(|line| line.to_lowercase().contains(query))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::filter_lines;

    #[test]
    fn log_search_preserves_lines_and_ignores_case() {
        assert_eq!(
            filter_lines("INFO ready\nERROR failed\nerror again", "error"),
            "ERROR failed\nerror again"
        );
        assert_eq!(filter_lines("一\n二", ""), "一\n二");
        assert_eq!(filter_lines("ready", "missing"), "");
    }
}
