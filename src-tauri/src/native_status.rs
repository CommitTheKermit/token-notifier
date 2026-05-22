#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
    use objc2_foundation::NSString;
    use std::cell::RefCell;

    thread_local! {
        static STATUS_ITEM: RefCell<Option<objc2::rc::Retained<NSStatusItem>>> = const { RefCell::new(None) };
    }

    pub fn install_on_main(initial_title: &str) {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("native status item install skipped: not on main thread");
            return;
        };
        STATUS_ITEM.with(|cell| {
            if cell.borrow().is_none() {
                let status_bar = NSStatusBar::systemStatusBar();
                let item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
                item.setVisible(true);
                if let Some(button) = item.button(mtm) {
                    let title = NSString::from_str(initial_title);
                    button.setTitle(&title);
                }
                *cell.borrow_mut() = Some(item);
            } else if let Some(item) = cell.borrow().as_ref() {
                if let Some(button) = item.button(mtm) {
                    let title = NSString::from_str(initial_title);
                    button.setTitle(&title);
                }
            }
        });
    }

    pub fn update_title_on_main(title: &str) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        STATUS_ITEM.with(|cell| {
            if cell.borrow().is_none() {
                drop(cell.borrow());
                install_on_main(title);
                return;
            }
            if let Some(item) = cell.borrow().as_ref() {
                item.setVisible(true);
                if let Some(button) = item.button(mtm) {
                    let title = NSString::from_str(title);
                    button.setTitle(&title);
                }
            }
        });
    }
}

pub fn install_initial<R: tauri::Runtime>(app: &tauri::AppHandle<R>, title: String) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || macos::install_on_main(&title));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, title);
    }
}

pub fn update_title<R: tauri::Runtime>(app: &tauri::AppHandle<R>, title: String) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(move || macos::update_title_on_main(&title));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, title);
    }
}
