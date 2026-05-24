#[cfg(target_os = "macos")]
mod macos {
    use super::{NativePopoverState, NativeStatusAnchor, NativeStatusClick};
    use objc2::{
        define_class, rc::Retained, DeclaredClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{NSAttributedStringNSExtendedStringDrawing, NSStringDrawingOptions};
    use objc2_app_kit::{
        NSBezierPath, NSColor, NSEvent, NSFont, NSFontAttributeName,
        NSForegroundColorAttributeName, NSLineBreakMode, NSMutableParagraphStyle,
        NSParagraphStyleAttributeName, NSPopover, NSPopoverBehavior, NSStatusBar, NSStatusItem,
        NSTextAlignment, NSView, NSViewController,
    };
    use objc2_foundation::{
        NSAttributedString, NSDictionary, NSPoint, NSRect, NSRectEdge, NSSize, NSString,
    };
    use std::cell::RefCell;
    use std::sync::mpsc::Sender;

    const STATUS_FONT_SIZE: f64 = 8.0;
    const STATUS_LINE_HEIGHT: f64 = 9.0;
    const STATUS_ITEM_WIDTH: f64 = 66.0;
    const POPOVER_WIDTH: f64 = 380.0;
    const POPOVER_HEIGHT: f64 = 430.0;

    thread_local! {
        static STATUS_STATE: RefCell<Option<StatusState>> = const { RefCell::new(None) };
    }

    struct StatusState {
        item: Retained<NSStatusItem>,
        view: Retained<StatusView>,
        popover: RefCell<Option<Retained<NSPopover>>>,
        popover_view: RefCell<Option<Retained<PopoverView>>>,
    }

    #[derive(Debug)]
    struct StatusViewIvars {
        sender: Sender<NativeStatusClick>,
        title: RefCell<String>,
    }

    #[derive(Debug)]
    struct PopoverViewIvars {
        state: RefCell<NativePopoverState>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[name = "TokenNotifierStatusView"]
        #[ivars = StatusViewIvars]
        struct StatusView;

        impl StatusView {
            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty_rect: NSRect) {
                let title = self.ivars().title.borrow().clone();
                let font = status_font();
                let attributed_title = status_attributed_title(&title, &font);
                let bounds = self.bounds();
                let options = NSStringDrawingOptions::UsesLineFragmentOrigin
                    | NSStringDrawingOptions::UsesFontLeading;
                let measured = attributed_title.boundingRectWithSize_options_context(
                    NSSize::new(bounds.size.width, f64::INFINITY),
                    options,
                    None,
                );
                let draw_height = measured.size.height.ceil().min(bounds.size.height);
                let y = ((bounds.size.height - draw_height) / 2.0).max(0.0);
                let draw_rect = NSRect::new(
                    NSPoint::new(0.0, y),
                    NSSize::new(bounds.size.width, draw_height),
                );
                attributed_title.drawWithRect_options_context(draw_rect, options, None);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _event: &NSEvent) {
                let _ = self.ivars().sender.send(NativeStatusClick::OpenPopover);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, _event: &NSEvent) {
                let _ = self.ivars().sender.send(NativeStatusClick::OpenSettings);
            }
        }
    );

    define_class!(
        #[unsafe(super(NSView))]
        #[name = "TokenNotifierPopoverView"]
        #[ivars = PopoverViewIvars]
        struct PopoverView;

        impl PopoverView {
            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty_rect: NSRect) {
                draw_popover_content(self.bounds(), &self.ivars().state.borrow());
            }
        }
    );

    impl PopoverView {
        fn set_state(&self, state: NativePopoverState) {
            *self.ivars().state.borrow_mut() = state;
            self.setNeedsDisplay(true);
        }
    }

    pub fn install_on_main(initial_title: &str, tooltip: &str, sender: Sender<NativeStatusClick>) {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("native status item install skipped: not on main thread");
            return;
        };
        STATUS_STATE.with(|cell| {
            if cell.borrow().is_none() {
                let status_bar = NSStatusBar::systemStatusBar();
                let item = status_bar.statusItemWithLength(STATUS_ITEM_WIDTH);
                item.setVisible(true);
                item.setLength(STATUS_ITEM_WIDTH);

                let frame = NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(STATUS_ITEM_WIDTH, status_bar.thickness()),
                );
                let view = mtm.alloc().set_ivars(StatusViewIvars {
                    sender,
                    title: RefCell::new(initial_title.to_string()),
                });
                let view: Retained<StatusView> =
                    unsafe { objc2::msg_send![super(view), initWithFrame: frame] };
                view.setToolTip(Some(&NSString::from_str(tooltip)));
                // NSStatusBarButton does not vertically center multi-line titles.
                // A custom view lets drawRect center the measured text block automatically.
                #[allow(deprecated)]
                item.setView(Some(&view));

                *cell.borrow_mut() = Some(StatusState {
                    item,
                    view,
                    popover: RefCell::new(None),
                    popover_view: RefCell::new(None),
                });
            } else if let Some(state) = cell.borrow().as_ref() {
                set_status_title(&state.view, initial_title, tooltip);
            }
        });
    }

    pub fn update_title_on_main(title: &str, tooltip: &str) {
        STATUS_STATE.with(|cell| {
            if let Some(state) = cell.borrow().as_ref() {
                state.item.setVisible(true);
                state.item.setLength(STATUS_ITEM_WIDTH);
                set_status_title(&state.view, title, tooltip);
            }
        });
    }

    pub fn update_popover_on_main(popover_state: NativePopoverState) {
        STATUS_STATE.with(|cell| {
            if let Some(state) = cell.borrow().as_ref() {
                if let Some(view) = state.popover_view.borrow().as_ref() {
                    view.set_state(popover_state);
                }
            }
        });
    }

    pub fn toggle_popover_on_main(popover_state: NativePopoverState) {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("native popover skipped: not on main thread");
            return;
        };
        STATUS_STATE.with(|cell| {
            let binding = cell.borrow();
            let Some(state) = binding.as_ref() else {
                return;
            };

            if let Some(popover) = state.popover.borrow().as_ref() {
                if popover.isShown() {
                    popover.close();
                    return;
                }
            }

            let existing_popover = state.popover.borrow().as_ref().cloned();
            let popover = match existing_popover {
                Some(popover) => popover,
                None => {
                    let popover = NSPopover::init(NSPopover::alloc(mtm));
                    popover.setBehavior(NSPopoverBehavior::Transient);
                    popover.setAnimates(true);
                    popover.setContentSize(NSSize::new(POPOVER_WIDTH, POPOVER_HEIGHT));

                    let content_frame = NSRect::new(
                        NSPoint::new(0.0, 0.0),
                        NSSize::new(POPOVER_WIDTH, POPOVER_HEIGHT),
                    );
                    let popover_view = mtm.alloc().set_ivars(PopoverViewIvars {
                        state: RefCell::new(popover_state.clone()),
                    });
                    let popover_view: Retained<PopoverView> = unsafe {
                        objc2::msg_send![super(popover_view), initWithFrame: content_frame]
                    };

                    let controller = NSViewController::new(mtm);
                    controller.setView(&popover_view);
                    popover.setContentViewController(Some(&controller));

                    *state.popover.borrow_mut() = Some(popover.clone());
                    *state.popover_view.borrow_mut() = Some(popover_view);
                    popover
                }
            };

            if let Some(view) = state.popover_view.borrow().as_ref() {
                view.set_state(popover_state);
            }

            popover.showRelativeToRect_ofView_preferredEdge(
                state.view.bounds(),
                &state.view,
                NSRectEdge::MinY,
            );
        });
    }

    fn set_status_title(view: &StatusView, title: &str, tooltip: &str) {
        *view.ivars().title.borrow_mut() = title.to_string();
        view.setToolTip(Some(&NSString::from_str(tooltip)));
        view.setNeedsDisplay(true);
    }

    fn status_font() -> Retained<NSFont> {
        NSFont::userFixedPitchFontOfSize(STATUS_FONT_SIZE)
            .unwrap_or_else(|| NSFont::menuBarFontOfSize(STATUS_FONT_SIZE))
    }

    fn status_attributed_title(
        title: &str,
        font: &Retained<NSFont>,
    ) -> Retained<NSAttributedString> {
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setAlignment(NSTextAlignment(2));
        paragraph.setLineBreakMode(NSLineBreakMode::ByClipping);
        paragraph.setMinimumLineHeight(STATUS_LINE_HEIGHT);
        paragraph.setMaximumLineHeight(STATUS_LINE_HEIGHT);

        let color = NSColor::labelColor();
        attributed_string(title, font, &color, NSTextAlignment(2))
    }

    fn draw_popover_content(bounds: NSRect, state: &NativePopoverState) {
        let inset = 16.0;
        let width = bounds.size.width;
        let title_font = NSFont::boldSystemFontOfSize(13.0);
        let body_font = NSFont::systemFontOfSize(13.0);
        let small_font = NSFont::systemFontOfSize(11.0);
        let value_font = NSFont::boldSystemFontOfSize(18.0);
        let label = NSColor::labelColor();
        let secondary = label.colorWithAlphaComponent(0.58);
        let tertiary = label.colorWithAlphaComponent(0.38);
        let separator = NSColor::separatorColor().colorWithAlphaComponent(0.72);
        let section_fill = NSColor::colorWithWhite_alpha(1.0, 0.07);
        let track = NSColor::colorWithWhite_alpha(1.0, 0.12);
        let accent = NSColor::controlAccentColor().colorWithAlphaComponent(0.82);

        draw_text(
            "토큰 한도",
            inset,
            14.0,
            120.0,
            18.0,
            &small_font,
            &secondary,
            NSTextAlignment(0),
            bounds,
        );
        draw_text(
            &state.updated_text,
            width - 170.0,
            14.0,
            154.0,
            18.0,
            &small_font,
            &tertiary,
            NSTextAlignment(2),
            bounds,
        );

        let source_section = rect_from_top(inset, 38.0, width - inset * 2.0, 146.0, bounds);
        draw_rounded_rect(source_section, 14.0, &section_fill, Some(&separator));

        let mut row_top = 50.0;
        for (index, source) in state.sources.iter().enumerate() {
            let icon_rect = rect_from_top(inset + 15.0, row_top + 15.0, 26.0, 16.0, bounds);
            draw_rounded_rect(icon_rect, 5.0, &track, None);
            let fill_width = (source.fraction.clamp(0.0, 1.0) * 26.0).max(3.0);
            let icon_fill = NSRect::new(
                icon_rect.origin,
                NSSize::new(fill_width, icon_rect.size.height),
            );
            draw_rounded_rect(icon_fill, 5.0, &accent, None);

            draw_text(
                &source.label,
                inset + 52.0,
                row_top + 7.0,
                180.0,
                20.0,
                &title_font,
                &label,
                NSTextAlignment(0),
                bounds,
            );
            draw_text(
                &source.percent_text,
                width - inset - 76.0,
                row_top + 7.0,
                60.0,
                20.0,
                &title_font,
                &label,
                NSTextAlignment(2),
                bounds,
            );

            let meter_rect = rect_from_top(
                inset + 52.0,
                row_top + 34.0,
                width - inset * 2.0 - 68.0,
                5.0,
                bounds,
            );
            draw_rounded_rect(meter_rect, 2.5, &track, None);
            let meter_fill = NSRect::new(
                meter_rect.origin,
                NSSize::new(
                    meter_rect.size.width * source.fraction.clamp(0.0, 1.0),
                    meter_rect.size.height,
                ),
            );
            draw_rounded_rect(meter_fill, 2.5, &accent, None);

            draw_text(
                &source.reset_text,
                inset + 52.0,
                row_top + 44.0,
                width - inset * 2.0 - 68.0,
                18.0,
                &small_font,
                &secondary,
                NSTextAlignment(0),
                bounds,
            );

            if index + 1 < state.sources.len() {
                draw_separator(
                    inset + 52.0,
                    row_top + 70.0,
                    width - inset * 2.0 - 52.0,
                    &separator,
                    bounds,
                );
            }
            row_top += 72.0;
        }

        let graph_top = 200.0;
        draw_text(
            "사용량 그래프",
            inset,
            graph_top,
            120.0,
            18.0,
            &small_font,
            &secondary,
            NSTextAlignment(0),
            bounds,
        );
        draw_text(
            "Last 24 hours",
            inset,
            graph_top + 24.0,
            160.0,
            22.0,
            &title_font,
            &label,
            NSTextAlignment(0),
            bounds,
        );
        draw_text(
            "로컬 추정 + API 보정",
            width - 160.0,
            graph_top + 24.0,
            144.0,
            20.0,
            &small_font,
            &tertiary,
            NSTextAlignment(2),
            bounds,
        );
        let graph_rect = rect_from_top(inset, graph_top + 56.0, width - inset * 2.0, 112.0, bounds);
        draw_rounded_rect(graph_rect, 14.0, &section_fill, Some(&separator));
        draw_chart_placeholder(graph_rect, &accent, &separator);

        let rollup_top = 332.0;
        let rollup_rect = rect_from_top(inset, rollup_top, width - inset * 2.0, 76.0, bounds);
        draw_rounded_rect(rollup_rect, 14.0, &section_fill, Some(&separator));
        let cell_w = rollup_rect.size.width / 3.0;
        let rollups = [
            ("Today", &state.rollup_day),
            ("This week", &state.rollup_week),
            ("This month", &state.rollup_month),
        ];
        for (index, (name, value)) in rollups.iter().enumerate() {
            let x = inset + cell_w * index as f64;
            if index > 0 {
                draw_vertical_separator(x, rollup_top, 76.0, &separator, bounds);
            }
            draw_text(
                name,
                x + 12.0,
                rollup_top + 16.0,
                cell_w - 24.0,
                18.0,
                &body_font,
                &secondary,
                NSTextAlignment(0),
                bounds,
            );
            draw_text(
                value,
                x + 12.0,
                rollup_top + 44.0,
                cell_w - 24.0,
                24.0,
                &value_font,
                &label,
                NSTextAlignment(0),
                bounds,
            );
        }
    }

    fn draw_chart_placeholder(rect: NSRect, accent: &NSColor, separator: &NSColor) {
        separator.setStroke();
        let base_y = rect.origin.y + 24.0;
        for i in 0..4 {
            let y = base_y + i as f64 * 20.0;
            NSBezierPath::strokeLineFromPoint_toPoint(
                NSPoint::new(rect.origin.x + 14.0, y),
                NSPoint::new(rect.origin.x + rect.size.width - 14.0, y),
            );
        }

        accent.setStroke();
        let line = NSBezierPath::bezierPath();
        line.setLineWidth(2.0);
        line.moveToPoint(NSPoint::new(rect.origin.x + 18.0, rect.origin.y + 35.0));
        line.lineToPoint(NSPoint::new(rect.origin.x + 74.0, rect.origin.y + 48.0));
        line.lineToPoint(NSPoint::new(rect.origin.x + 132.0, rect.origin.y + 41.0));
        line.lineToPoint(NSPoint::new(rect.origin.x + 206.0, rect.origin.y + 72.0));
        line.lineToPoint(NSPoint::new(
            rect.origin.x + rect.size.width - 20.0,
            rect.origin.y + 58.0,
        ));
        line.stroke();
    }

    fn rect_from_top(x: f64, top: f64, width: f64, height: f64, bounds: NSRect) -> NSRect {
        NSRect::new(
            NSPoint::new(x, bounds.size.height - top - height),
            NSSize::new(width, height),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        text: &str,
        x: f64,
        top: f64,
        width: f64,
        height: f64,
        font: &Retained<NSFont>,
        color: &NSColor,
        alignment: NSTextAlignment,
        bounds: NSRect,
    ) {
        let rect = rect_from_top(x, top, width, height, bounds);
        let string = attributed_string(text, font, color, alignment);
        string.drawWithRect_options_context(
            rect,
            NSStringDrawingOptions::UsesLineFragmentOrigin
                | NSStringDrawingOptions::UsesFontLeading,
            None,
        );
    }

    fn attributed_string(
        text: &str,
        font: &Retained<NSFont>,
        color: &NSColor,
        alignment: NSTextAlignment,
    ) -> Retained<NSAttributedString> {
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setAlignment(alignment);
        paragraph.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let attribute_keys = unsafe {
            [
                NSFontAttributeName,
                NSParagraphStyleAttributeName,
                NSForegroundColorAttributeName,
            ]
        };
        let attributes = NSDictionary::from_retained_objects(
            &attribute_keys,
            &[
                font.clone().into_super().into_super(),
                paragraph.into_super().into_super().into(),
                color.retain().into_super().into_super(),
            ],
        );
        unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(text), &attributes) }
    }

    fn draw_rounded_rect(rect: NSRect, radius: f64, fill: &NSColor, stroke: Option<&NSColor>) {
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
        fill.setFill();
        path.fill();
        if let Some(stroke) = stroke {
            stroke.setStroke();
            path.setLineWidth(1.0);
            path.stroke();
        }
    }

    fn draw_separator(x: f64, top: f64, width: f64, color: &NSColor, bounds: NSRect) {
        color.setStroke();
        let y = bounds.size.height - top;
        NSBezierPath::strokeLineFromPoint_toPoint(NSPoint::new(x, y), NSPoint::new(x + width, y));
    }

    fn draw_vertical_separator(x: f64, top: f64, height: f64, color: &NSColor, bounds: NSRect) {
        color.setStroke();
        let y = bounds.size.height - top - height;
        NSBezierPath::strokeLineFromPoint_toPoint(NSPoint::new(x, y), NSPoint::new(x, y + height));
    }

    pub fn anchor_rect_on_main() -> Option<NativeStatusAnchor> {
        STATUS_STATE.with(|cell| {
            let state = cell.borrow();
            let view = &state.as_ref()?.view;
            let window = view.window()?;
            let view_frame = view.frame();
            let window_frame = window.frame();
            Some(NativeStatusAnchor {
                x: window_frame.origin.x + view_frame.origin.x,
                y: window_frame.origin.y + view_frame.origin.y,
                width: view_frame.size.width,
                height: view_frame.size.height,
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativePopoverSourceState {
    pub label: String,
    pub percent_text: String,
    pub reset_text: String,
    pub fraction: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativePopoverState {
    pub sources: Vec<NativePopoverSourceState>,
    pub rollup_day: String,
    pub rollup_week: String,
    pub rollup_month: String,
    pub updated_text: String,
}

impl Default for NativePopoverState {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            rollup_day: "--".to_string(),
            rollup_week: "--".to_string(),
            rollup_month: "--".to_string(),
            updated_text: "--".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStatusClick {
    OpenPopover,
    OpenSettings,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeStatusAnchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn install_initial<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    title: String,
    tooltip: String,
    click_sender: std::sync::mpsc::Sender<NativeStatusClick>,
) {
    #[cfg(target_os = "macos")]
    {
        let _ =
            app.run_on_main_thread(move || macos::install_on_main(&title, &tooltip, click_sender));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, title, tooltip, click_sender);
    }
}

pub fn update_title<R: tauri::Runtime>(app: &tauri::AppHandle<R>, title: String, tooltip: String) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || macos::update_title_on_main(&title, &tooltip));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, title, tooltip);
    }
}

pub fn update_popover<R: tauri::Runtime>(app: &tauri::AppHandle<R>, state: NativePopoverState) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || macos::update_popover_on_main(state));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
    }
}

pub fn toggle_popover(state: NativePopoverState) {
    #[cfg(target_os = "macos")]
    {
        macos::toggle_popover_on_main(state);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
    }
}

pub fn anchor_rect() -> Option<NativeStatusAnchor> {
    #[cfg(target_os = "macos")]
    {
        macos::anchor_rect_on_main()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
