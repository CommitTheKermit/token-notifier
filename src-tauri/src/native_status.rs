#[cfg(target_os = "macos")]
mod macos {
    use super::{NativeStatusAnchor, NativeStatusClick};
    use objc2::{define_class, rc::Retained, DeclaredClass, MainThreadMarker};
    use objc2_app_kit::{NSAttributedStringNSExtendedStringDrawing, NSStringDrawingOptions};
    use objc2_app_kit::{
        NSColor, NSEvent, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
        NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStatusBar,
        NSStatusItem, NSTextAlignment, NSView,
    };
    use objc2_foundation::{NSAttributedString, NSDictionary, NSPoint, NSRect, NSSize, NSString};
    use std::cell::RefCell;
    use std::sync::mpsc::Sender;

    const STATUS_FONT_SIZE: f64 = 8.0;
    const STATUS_LINE_HEIGHT: f64 = 9.0;
    const STATUS_ITEM_WIDTH: f64 = 66.0;

    thread_local! {
        static STATUS_STATE: RefCell<Option<StatusState>> = const { RefCell::new(None) };
    }

    struct StatusState {
        item: Retained<NSStatusItem>,
        view: Retained<StatusView>,
    }

    #[derive(Debug)]
    struct StatusViewIvars {
        sender: Sender<NativeStatusClick>,
        title: RefCell<String>,
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

                *cell.borrow_mut() = Some(StatusState { item, view });
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
                color.into_super().into_super(),
            ],
        );
        unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(title), &attributes) }
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
