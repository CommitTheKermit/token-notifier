#[cfg(target_os = "macos")]
mod macos {
    use super::NativeStatusClick;
    use objc2::{
        define_class, rc::Retained, runtime::AnyObject, sel, DeclaredClass, MainThreadMarker,
    };
    use objc2_app_kit::{
        NSApplication, NSEventMask, NSEventType, NSFont, NSStatusBar, NSStatusItem, NSTextAlignment,
    };
    use objc2_foundation::{NSObject, NSString};
    use std::cell::RefCell;
    use std::sync::mpsc::Sender;

    const STATUS_FONT_SIZE: f64 = 8.0;
    const STATUS_ITEM_WIDTH: f64 = 58.0;

    thread_local! {
        static STATUS_STATE: RefCell<Option<StatusState>> = const { RefCell::new(None) };
    }

    struct StatusState {
        item: Retained<NSStatusItem>,
        _target: Retained<StatusTarget>,
    }

    #[derive(Debug)]
    struct StatusTargetIvars {
        sender: Sender<NativeStatusClick>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "TokenNotifierStatusTarget"]
        #[ivars = StatusTargetIvars]
        struct StatusTarget;

        impl StatusTarget {
            #[unsafe(method(statusItemClicked:))]
            fn status_item_clicked(&self, _sender: Option<&AnyObject>) {
                let click = current_click_kind().unwrap_or(NativeStatusClick::OpenPopover);
                let _ = self.ivars().sender.send(click);
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
                let target = mtm.alloc().set_ivars(StatusTargetIvars { sender });
                let target: Retained<StatusTarget> =
                    unsafe { objc2::msg_send![super(target), init] };
                item.setVisible(true);
                set_status_title(&item, mtm, initial_title, tooltip);
                set_status_target(&item, mtm, &target);
                *cell.borrow_mut() = Some(StatusState {
                    item,
                    _target: target,
                });
            } else if let Some(state) = cell.borrow().as_ref() {
                set_status_title(&state.item, mtm, initial_title, tooltip);
            }
        });
    }

    pub fn update_title_on_main(title: &str, tooltip: &str) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        STATUS_STATE.with(|cell| {
            if cell.borrow().is_none() {
                return;
            }
            if let Some(state) = cell.borrow().as_ref() {
                state.item.setVisible(true);
                set_status_title(&state.item, mtm, title, tooltip);
            }
        });
    }

    fn set_status_title(item: &NSStatusItem, mtm: MainThreadMarker, title: &str, tooltip: &str) {
        item.setLength(STATUS_ITEM_WIDTH);
        if let Some(button) = item.button(mtm) {
            let font = NSFont::menuBarFontOfSize(STATUS_FONT_SIZE);
            button.setFont(Some(&font));
            button.setUsesSingleLineMode(false);
            button.setAlignment(NSTextAlignment(2));
            button.setTitle(&NSString::from_str(title));
            button.setToolTip(Some(&NSString::from_str(tooltip)));
        }
    }

    fn set_status_target(item: &NSStatusItem, mtm: MainThreadMarker, target: &StatusTarget) {
        if let Some(button) = item.button(mtm) {
            unsafe {
                button.setTarget(Some(target));
                button.setAction(Some(sel!(statusItemClicked:)));
            }
            button.sendActionOn(NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown);
        }
    }

    fn current_click_kind() -> Option<NativeStatusClick> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        let event = app.currentEvent()?;
        if event.r#type() == NSEventType::RightMouseDown {
            Some(NativeStatusClick::OpenSettings)
        } else {
            Some(NativeStatusClick::OpenPopover)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStatusClick {
    OpenPopover,
    OpenSettings,
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
