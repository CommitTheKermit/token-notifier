#[cfg(target_os = "macos")]
mod macos {
    use super::{NativePopoverAction, NativePopoverState, NativeStatusAnchor, NativeStatusClick};
    use block2::RcBlock;
    use objc2::runtime::AnyObject;
    use objc2::{
        define_class, rc::Retained, AnyThread, DeclaredClass, MainThreadMarker, MainThreadOnly,
        Message,
    };
    use objc2_app_kit::NSAttributedStringNSExtendedStringDrawing;
    use objc2_app_kit::{
        NSBackingStoreType, NSBezierPath, NSColor, NSEvent, NSEventMask, NSFont,
        NSFontAttributeName, NSFontWeight, NSForegroundColorAttributeName, NSLineBreakMode,
        NSMutableParagraphStyle, NSPanel, NSParagraphStyleAttributeName, NSStatusBar, NSStatusItem,
        NSStringDrawingOptions, NSTextAlignment, NSView, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowStyleMask,
    };
    use objc2_foundation::NSMutableAttributedString;
    use objc2_foundation::{
        NSAttributedString, NSDictionary, NSPoint, NSRange, NSRect, NSSize, NSString,
    };
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::sync::mpsc::Sender;

    // 떠 있는 패널을 메뉴바 아래 몇 px 띄운다.
    const PANEL_GAP: f64 = 6.0;
    // 팝업 메뉴 수준(kCGPopUpMenuWindowLevel). 다른 앱 창 위에 뜬다.
    const PANEL_LEVEL: isize = 101;

    // 메뉴바(D6): 단일 줄 `12% 84%`. 큰 숫자 + 작은 % (위치로만 클로드코드/코덱스 구분).
    // 항목 폭은 표시 텍스트에 맞춰 동적으로 정한다(토글로 한쪽을 끄면 빈 공간 없이 줄어듦).
    const STATUS_FONT_SIZE: f64 = 13.0;
    const D6_BIG: f64 = 13.0;
    const D6_PCT: f64 = 9.0;
    // 텍스트 좌우 여백 합. 측정 폭에 더해 항목 폭을 정한다.
    const STATUS_ITEM_PADDING: f64 = 14.0;
    const STATUS_ITEM_MIN_WIDTH: f64 = 30.0;
    const STATUS_ITEM_MAX_WIDTH: f64 = 160.0;

    // 펼친 패널(FinalPanel) 레이아웃 상수 (top-origin 거리).
    const POPOVER_WIDTH: f64 = 296.0;
    const POPOVER_HEIGHT: f64 = 300.0;
    const PANEL_PAD_X: f64 = 16.0;
    const ROWS_TOP: f64 = 34.0;
    const ROW_H: f64 = 74.0;
    const TOGGLES_TOP: f64 = 212.0;
    const TOGGLE_ROW_H: f64 = 40.0;
    const TOGGLE_W: f64 = 40.0;
    const TOGGLE_H: f64 = 24.0;

    thread_local! {
        static STATUS_STATE: RefCell<Option<StatusState>> = const { RefCell::new(None) };
    }

    struct StatusState {
        item: Retained<NSStatusItem>,
        view: Retained<StatusView>,
        action_sender: Sender<NativePopoverAction>,
        // 화살표 없는 떠 있는 패널(NSPanel). 메뉴바 아래에 둥근 모서리로 띄운다.
        panel: RefCell<Option<Retained<NSPanel>>>,
        panel_view: RefCell<Option<Retained<PopoverView>>>,
        // 패널 바깥(다른 앱/데스크톱) 클릭 감지용 전역 이벤트 모니터.
        monitor: RefCell<Option<Retained<AnyObject>>>,
    }

    #[derive(Debug)]
    struct StatusViewIvars {
        sender: Sender<NativeStatusClick>,
        title: RefCell<String>,
    }

    #[derive(Debug)]
    struct PopoverViewIvars {
        state: RefCell<NativePopoverState>,
        action_sender: Sender<NativePopoverAction>,
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
                draw_status_title(self.bounds(), &title);
            }

            // D6 펼친 패널을 부활시켰다. 클릭하면 아이콘 아래로 패널이 열리고 닫힌다.
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _event: &NSEvent) {
                let _ = self.ivars().sender.send(NativeStatusClick::OpenPopover);
            }

            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, _event: &NSEvent) {
                let _ = self.ivars().sender.send(NativeStatusClick::OpenPopover);
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

            // 토글 스위치 영역 클릭만 처리. 어느 토글을 눌렀는지 hit-test 후 Rust로 통지한다.
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                let bounds = self.bounds();
                let window_point = event.locationInWindow();
                let local = self.convertPoint_fromView(window_point, None);
                let top_y = bounds.size.height - local.y;
                let width = bounds.size.width;
                for index in 0..2usize {
                    let (x, top, w, h) = toggle_hit_rect(index, width);
                    if local.x >= x && local.x <= x + w && top_y >= top && top_y <= top + h {
                        let _ = self
                            .ivars()
                            .action_sender
                            .send(NativePopoverAction::ToggleSource(index));
                        return;
                    }
                }
            }

            // 비활성(nonactivating) 패널이라도 첫 클릭이 토글에 바로 전달되도록 한다.
            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
                true
            }
        }
    );

    impl PopoverView {
        fn set_state(&self, state: NativePopoverState) {
            *self.ivars().state.borrow_mut() = state;
            self.setNeedsDisplay(true);
        }
    }

    pub fn install_on_main(
        initial_title: &str,
        tooltip: &str,
        sender: Sender<NativeStatusClick>,
        action_sender: Sender<NativePopoverAction>,
    ) {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("native status item install skipped: not on main thread");
            return;
        };
        STATUS_STATE.with(|cell| {
            if cell.borrow().is_none() {
                let status_bar = NSStatusBar::systemStatusBar();
                let width = status_item_width(initial_title);
                let item = status_bar.statusItemWithLength(width);
                item.setVisible(true);
                item.setLength(width);

                let frame = NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(width, status_bar.thickness()),
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
                    action_sender,
                    panel: RefCell::new(None),
                    panel_view: RefCell::new(None),
                    monitor: RefCell::new(None),
                });
            } else if let Some(state) = cell.borrow().as_ref() {
                set_status_title(&state.view, initial_title, tooltip);
            }
        });
    }

    pub fn update_title_on_main(title: &str, tooltip: &str) {
        STATUS_STATE.with(|cell| {
            if let Some(state) = cell.borrow().as_ref() {
                let width = status_item_width(title);
                state.item.setVisible(true);
                state.item.setLength(width);
                // 항목 폭이 바뀌어도 커스텀 뷰의 프레임은 자동으로 따라오지 않으므로
                // 직접 맞춰줘야 drawRect가 새 폭 안에서 중앙정렬한다.
                let thickness = NSStatusBar::systemStatusBar().thickness();
                state.view.setFrame(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(width, thickness),
                ));
                set_status_title(&state.view, title, tooltip);

                // 폭이 바뀌면 아이콘 위치가 이동하므로, 열려 있는 패널을 새 위치로 옮긴다.
                if let Some(panel) = state.panel.borrow().as_ref() {
                    if let Some(origin) = panel_origin(&state.view) {
                        panel.setFrameOrigin(origin);
                    }
                }
            }
        });
    }

    // 표시 텍스트 폭을 측정해 항목 폭을 정한다. 토글로 숫자가 줄면 폭도 함께 줄어든다.
    fn status_item_width(title: &str) -> f64 {
        let color = NSColor::labelColor();
        let text_width = if title.contains('%') {
            let big = mono_bold(D6_BIG);
            let small = mono_bold(D6_PCT);
            let attributed = mixed_pct(title, &big, &small, &color, NSTextAlignment(2));
            measure(&attributed).width
        } else {
            let font = NSFont::menuBarFontOfSize(STATUS_FONT_SIZE);
            let attributed = attributed_string(title, &font, &color, NSTextAlignment(2));
            measure(&attributed).width
        };
        (text_width + STATUS_ITEM_PADDING).clamp(STATUS_ITEM_MIN_WIDTH, STATUS_ITEM_MAX_WIDTH)
    }

    pub fn update_popover_on_main(popover_state: NativePopoverState) {
        STATUS_STATE.with(|cell| {
            if let Some(state) = cell.borrow().as_ref() {
                if let Some(view) = state.panel_view.borrow().as_ref() {
                    view.set_state(popover_state);
                }
            }
        });
    }

    pub fn toggle_popover_on_main(popover_state: NativePopoverState) {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("native panel skipped: not on main thread");
            return;
        };
        STATUS_STATE.with(|cell| {
            let binding = cell.borrow();
            let Some(state) = binding.as_ref() else {
                return;
            };

            // 이미 떠 있으면 닫고 끝낸다.
            if state.panel.borrow().is_some() {
                close_panel(state);
                return;
            }

            let content_frame = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(POPOVER_WIDTH, POPOVER_HEIGHT),
            );

            // 콘텐츠를 그리는 커스텀 뷰.
            let popover_view = mtm.alloc().set_ivars(PopoverViewIvars {
                state: RefCell::new(popover_state.clone()),
                action_sender: state.action_sender.clone(),
            });
            let popover_view: Retained<PopoverView> =
                unsafe { objc2::msg_send![super(popover_view), initWithFrame: content_frame] };

            // 블러 + 둥근 모서리 컨테이너.
            let effect =
                NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), content_frame);
            effect.setMaterial(NSVisualEffectMaterial::Popover);
            effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            effect.setState(NSVisualEffectState::Active);
            unsafe {
                let _: () = objc2::msg_send![&*effect, setWantsLayer: true];
                let layer: *mut AnyObject = objc2::msg_send![&*effect, layer];
                if !layer.is_null() {
                    let _: () = objc2::msg_send![layer, setCornerRadius: 14.0_f64];
                    let _: () = objc2::msg_send![layer, setMasksToBounds: true];
                }
            }
            effect.addSubview(&popover_view);

            // 화살표 없는 borderless 비활성 패널.
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                NSPanel::alloc(mtm),
                content_frame,
                NSWindowStyleMask::NonactivatingPanel,
                NSBackingStoreType::Buffered,
                false,
            );
            unsafe { panel.setReleasedWhenClosed(false) };
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setLevel(PANEL_LEVEL);
            panel.setFloatingPanel(true);
            panel.setHidesOnDeactivate(false);
            panel.setContentView(Some(&effect));

            if let Some(origin) = panel_origin(&state.view) {
                panel.setFrameOrigin(origin);
            }
            panel.orderFrontRegardless();

            // 패널 바깥(다른 앱/데스크톱/메뉴바 다른 항목) 클릭 시 닫는다. 전역 모니터는
            // 우리 앱으로 전달되는 이벤트(패널 내부 클릭 등)에는 발화하지 않으므로
            // 패널 내부 토글 조작은 영향받지 않는다.
            let handler = RcBlock::new(|_event: NonNull<NSEvent>| {
                close_active_panel();
            });
            let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
                &handler,
            );

            *state.panel.borrow_mut() = Some(panel);
            *state.panel_view.borrow_mut() = Some(popover_view);
            *state.monitor.borrow_mut() = monitor;
        });
    }

    fn close_panel(state: &StatusState) {
        if let Some(panel) = state.panel.borrow_mut().take() {
            panel.orderOut(None);
        }
        *state.panel_view.borrow_mut() = None;
        if let Some(monitor) = state.monitor.borrow_mut().take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }

    fn close_active_panel() {
        STATUS_STATE.with(|cell| {
            if let Some(state) = cell.borrow().as_ref() {
                close_panel(state);
            }
        });
    }

    // 패널 좌상단(화면 좌표) 위치. 아이콘 오른쪽 모서리에 패널 오른쪽을 맞추고(우측 정렬)
    // 메뉴바 바로 아래에 띄운다. 폭이 줄어도 오른쪽 모서리는 안정적이라 위치가 흔들리지 않는다.
    fn panel_origin(view: &StatusView) -> Option<NSPoint> {
        let window = view.window()?;
        let view_frame = view.frame();
        let window_frame = window.frame();
        let right = window_frame.origin.x + view_frame.origin.x + view_frame.size.width;
        let item_bottom = window_frame.origin.y + view_frame.origin.y;
        Some(NSPoint::new(
            right - POPOVER_WIDTH,
            item_bottom - POPOVER_HEIGHT - PANEL_GAP,
        ))
    }

    fn set_status_title(view: &StatusView, title: &str, tooltip: &str) {
        *view.ivars().title.borrow_mut() = title.to_string();
        view.setToolTip(Some(&NSString::from_str(tooltip)));
        view.setNeedsDisplay(true);
    }

    fn status_font(size: f64) -> Retained<NSFont> {
        NSFont::userFixedPitchFontOfSize(size).unwrap_or_else(|| NSFont::menuBarFontOfSize(size))
    }

    // 큰 숫자에 어울리는 굵은 등폭(tabular) 시스템 폰트. (NSFontWeightBold = 0.4)
    fn mono_bold(size: f64) -> Retained<NSFont> {
        let weight: NSFontWeight = 0.4;
        NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
    }

    // ── 메뉴바 D6 단일 줄 ─────────────────────────────────────────────
    fn draw_status_title(bounds: NSRect, title: &str) {
        let color = NSColor::labelColor();
        if title.contains('%') {
            let big = mono_bold(D6_BIG);
            let small = mono_bold(D6_PCT);
            let attributed = mixed_pct(title, &big, &small, &color, NSTextAlignment(2));
            let size = measure(&attributed);
            let w = size.width.min(bounds.size.width);
            let h = size.height;
            let x = ((bounds.size.width - w) / 2.0).max(0.0);
            let top = ((bounds.size.height - h) / 2.0).max(0.0);
            let rect = rect_from_top(x, top, w + 1.0, h + 1.0, bounds);
            attributed.drawWithRect_options_context(rect, draw_options(), None);
        } else {
            let font = NSFont::menuBarFontOfSize(STATUS_FONT_SIZE);
            let line = STATUS_FONT_SIZE + 4.0;
            let top = ((bounds.size.height - line) / 2.0).max(0.0);
            draw_text(
                title,
                0.0,
                top,
                bounds.size.width,
                line,
                &font,
                &color,
                NSTextAlignment(2),
                bounds,
            );
        }
    }

    // ── 펼친 패널(FinalPanel) ─────────────────────────────────────────
    fn draw_popover_content(bounds: NSRect, state: &NativePopoverState) {
        let width = bounds.size.width;
        let pad = PANEL_PAD_X;
        let label = NSColor::labelColor();
        let section_font = status_font(11.0);
        let section_color = label.colorWithAlphaComponent(0.45);

        draw_text(
            "토큰 한도 · 잔량",
            pad,
            12.0,
            width - pad * 2.0,
            16.0,
            &section_font,
            &section_color,
            NSTextAlignment(0),
            bounds,
        );

        let sources: Vec<&super::NativePopoverSourceState> = state.sources.iter().take(2).collect();
        for (index, source) in sources.iter().enumerate() {
            let row_top = ROWS_TOP + index as f64 * ROW_H;
            draw_limit_row(bounds, width, row_top, source);
            if index + 1 < sources.len() {
                let div_y = ROWS_TOP + (index as f64 + 1.0) * ROW_H - 4.0;
                draw_separator(
                    pad,
                    div_y,
                    width - pad * 2.0,
                    &label.colorWithAlphaComponent(0.10),
                    bounds,
                );
            }
        }

        let rows_bottom = ROWS_TOP + sources.len() as f64 * ROW_H;
        draw_text(
            "표시 정보 설정",
            pad,
            rows_bottom + 12.0,
            width - pad * 2.0,
            16.0,
            &section_font,
            &section_color,
            NSTextAlignment(0),
            bounds,
        );

        for (index, source) in sources.iter().enumerate() {
            let top = TOGGLES_TOP + index as f64 * TOGGLE_ROW_H;
            draw_toggle_row(bounds, width, top, source);
        }
    }

    fn draw_limit_row(
        bounds: NSRect,
        width: f64,
        row_top: f64,
        source: &super::NativePopoverSourceState,
    ) {
        let pad = PANEL_PAD_X;
        let factor = if source.enabled { 1.0 } else { 0.4 };
        let label = NSColor::labelColor();
        let status = status_color(source.remaining, source.has_percent);
        let heading_top = row_top + 9.0;

        // 상태 점
        let dot_rect = rect_from_top(pad, heading_top + 6.0, 8.0, 8.0, bounds);
        draw_rounded_rect(dot_rect, 4.0, &status.colorWithAlphaComponent(factor), None);

        // 서비스명
        let name_font = NSFont::boldSystemFontOfSize(14.0);
        draw_text(
            &source.label,
            pad + 15.0,
            heading_top,
            150.0,
            20.0,
            &name_font,
            &label.colorWithAlphaComponent(factor),
            NSTextAlignment(0),
            bounds,
        );

        // 큰 퍼센트 (상태색) + 작은 %
        let big = mono_bold(17.0);
        let small = mono_bold(11.0);
        let pct = mixed_pct(
            &source.percent_text,
            &big,
            &small,
            &status.colorWithAlphaComponent(factor),
            NSTextAlignment(0),
        );
        let pct_size = measure(&pct);
        let pct_x = width - pad - pct_size.width;
        let pct_center = heading_top + 10.0;
        let pct_rect = rect_from_top(
            pct_x,
            pct_center - pct_size.height / 2.0,
            pct_size.width + 1.0,
            pct_size.height + 1.0,
            bounds,
        );
        pct.drawWithRect_options_context(pct_rect, draw_options(), None);

        // 게이지
        let meter_top = heading_top + 28.0;
        let meter_w = width - pad * 2.0;
        let track_rect = rect_from_top(pad, meter_top, meter_w, 6.0, bounds);
        draw_rounded_rect(
            track_rect,
            3.0,
            &label.colorWithAlphaComponent(0.12 * factor),
            None,
        );
        let fill_w = (source.fraction.clamp(0.0, 1.0) * meter_w).max(0.0);
        if fill_w > 0.5 {
            let fill_rect = rect_from_top(pad, meter_top, fill_w, 6.0, bounds);
            draw_rounded_rect(fill_rect, 3.0, &status.colorWithAlphaComponent(factor), None);
        }

        // 다음 갱신까지
        let detail_top = meter_top + 13.0;
        let detail_color = label.colorWithAlphaComponent(0.55 * factor);
        if source.has_reset {
            let body_font = NSFont::systemFontOfSize(12.0);
            let mono_font = status_font(12.0);
            draw_text(
                "다음 갱신까지",
                pad,
                detail_top,
                140.0,
                16.0,
                &body_font,
                &detail_color,
                NSTextAlignment(0),
                bounds,
            );
            draw_text(
                &source.detail,
                width - pad - 200.0,
                detail_top,
                200.0,
                16.0,
                &mono_font,
                &detail_color,
                NSTextAlignment(2),
                bounds,
            );
        } else {
            let body_font = NSFont::systemFontOfSize(12.0);
            draw_text(
                &source.detail,
                pad,
                detail_top,
                width - pad * 2.0,
                16.0,
                &body_font,
                &detail_color,
                NSTextAlignment(0),
                bounds,
            );
        }
    }

    fn draw_toggle_row(
        bounds: NSRect,
        width: f64,
        top: f64,
        source: &super::NativePopoverSourceState,
    ) {
        let pad = PANEL_PAD_X;
        let label = NSColor::labelColor();
        let status = status_color(source.remaining, source.has_percent);

        let dot_rect = rect_from_top(pad, top + 16.0, 8.0, 8.0, bounds);
        draw_rounded_rect(dot_rect, 4.0, &status, None);

        let name_font = NSFont::systemFontOfSize(13.5);
        draw_text(
            &source.label,
            pad + 15.0,
            top + 10.0,
            160.0,
            20.0,
            &name_font,
            &label,
            NSTextAlignment(0),
            bounds,
        );

        let toggle_x = width - pad - TOGGLE_W;
        draw_toggle(bounds, toggle_x, top + 8.0, source.enabled);
    }

    fn draw_toggle(bounds: NSRect, x: f64, top: f64, on: bool) {
        let track_rect = rect_from_top(x, top, TOGGLE_W, TOGGLE_H, bounds);
        let track_color = if on {
            srgb(0x46, 0xa8, 0x6b)
        } else {
            NSColor::labelColor().colorWithAlphaComponent(0.20)
        };
        draw_rounded_rect(track_rect, TOGGLE_H / 2.0, &track_color, None);

        let knob = 20.0;
        let knob_x = if on {
            x + TOGGLE_W - 2.0 - knob
        } else {
            x + 2.0
        };
        let knob_rect = rect_from_top(knob_x, top + 2.0, knob, knob, bounds);
        draw_rounded_rect(knob_rect, knob / 2.0, &NSColor::whiteColor(), None);
    }

    fn toggle_hit_rect(index: usize, width: f64) -> (f64, f64, f64, f64) {
        let top = TOGGLES_TOP + index as f64 * TOGGLE_ROW_H + 8.0;
        (width - PANEL_PAD_X - TOGGLE_W, top, TOGGLE_W, TOGGLE_H)
    }

    // 잔량 → 상태색. 여유는 시스템 라벨색(메뉴바에서 안 튐), 낮을수록 경고색.
    fn status_color(remaining: u8, has_percent: bool) -> Retained<NSColor> {
        if !has_percent {
            return NSColor::labelColor();
        }
        if remaining < 15 {
            srgb(0xa8, 0x3f, 0x2a)
        } else if remaining < 35 {
            srgb(0xb8, 0x7a, 0x3a)
        } else {
            NSColor::labelColor()
        }
    }

    fn srgb(r: u8, g: u8, b: u8) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
            1.0,
        )
    }

    fn draw_options() -> NSStringDrawingOptions {
        NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::UsesFontLeading
    }

    fn measure(attributed: &NSAttributedString) -> NSSize {
        attributed
            .boundingRectWithSize_options_context(
                NSSize::new(f64::INFINITY, f64::INFINITY),
                draw_options(),
                None,
            )
            .size
    }

    // 큰 숫자 + 작은 % 한 덩어리. base(큰 폰트)에 '%' 글자에만 작은 폰트를 덧씌운다.
    fn mixed_pct(
        text: &str,
        big: &Retained<NSFont>,
        small: &Retained<NSFont>,
        color: &NSColor,
        alignment: NSTextAlignment,
    ) -> Retained<NSMutableAttributedString> {
        let base = attributed_string(text, big, color, alignment);
        let attributed: Retained<NSMutableAttributedString> = unsafe {
            objc2::msg_send![NSMutableAttributedString::alloc(), initWithAttributedString: &*base]
        };
        let mut utf16_index = 0usize;
        for ch in text.chars() {
            let len = ch.len_utf16();
            if ch == '%' {
                let range = NSRange::new(utf16_index, len);
                unsafe {
                    let _: () = objc2::msg_send![
                        &*attributed,
                        addAttribute: NSFontAttributeName,
                        value: &**small,
                        range: range
                    ];
                }
            }
            utf16_index += len;
        }
        attributed
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
        string.drawWithRect_options_context(rect, draw_options(), None);
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
    /// 큰 숫자로 그릴 텍스트. 예: "12%", "~84%", "--".
    pub percent_text: String,
    /// 잔량(%). 상태색/게이지 계산용. has_percent=false면 0.
    pub remaining: u8,
    pub has_percent: bool,
    /// "2시간 10분 · 오후 7:20" 또는 상태 메시지.
    pub detail: String,
    pub has_reset: bool,
    pub fraction: f64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NativePopoverState {
    /// 항상 [클로드코드, 코덱스] 순서. 토글 off여도 흐리게 표시.
    pub sources: Vec<NativePopoverSourceState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStatusClick {
    OpenPopover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePopoverAction {
    /// 메뉴바 표시 토글. 0 = 클로드코드, 1 = 코덱스.
    ToggleSource(usize),
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
    action_sender: std::sync::mpsc::Sender<NativePopoverAction>,
) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || {
            macos::install_on_main(&title, &tooltip, click_sender, action_sender)
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, title, tooltip, click_sender, action_sender);
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
