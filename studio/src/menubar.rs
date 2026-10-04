//! The menu-bar item of the macOS application (T-413).
//!
//! What the menu shows and what its entries do is plain Rust, tested
//! without any GUI: `menu_model` builds the lines from the gate, the
//! e-stop latch and the output, `MenuActions::perform` carries out an
//! entry. `MenuAction` has no arming variant on purpose: nothing in the
//! menu arms the laser or resets an emergency stop; that stays the
//! on-screen button and Space in the page.
//!
//! The native item (NSStatusItem, objc2) lives in `native`, behind the
//! `macos-app` Cargo feature, so the default build, the tests and the e2e
//! runs pull in no GUI code. Quitting goes through the same path as
//! SIGTERM: cut the output, stop `running`, and `main` lets the engine
//! disarm, send dark frames and close the output before the process ends.

use crate::interlock::{ArmSource, EStop};
use crate::Shared;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenUi,
    EStop,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label_fr: &'static str,
    pub action: Option<MenuAction>,
    pub enabled: bool,
}

/// The menu-bar icon: outline when disarmed, filled when armed, crossed
/// out while an emergency stop is latched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuIcon {
    Disarmed,
    Armed,
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuModel {
    /// « Désarmé », « ARMÉ » or « Arrêt d'urgence ».
    pub status: String,
    /// « Sortie : <nom> » or « Aperçu seulement ».
    pub output: String,
    /// The entries under the (greyed) status line, in order.
    pub items: Vec<MenuItem>,
    pub icon: MenuIcon,
}

impl MenuModel {
    /// The greyed first line of the menu.
    pub fn status_line(&self) -> String {
        format!("{} — {}", self.status, self.output)
    }

    /// The tooltip of the menu-bar icon.
    pub fn tooltip(&self) -> String {
        format!("Laser Studio : {}", self.status_line())
    }

    /// `GET /api/test/menu` (`--test-hooks`): the menu as the native item
    /// would show it, for the tests that can't click it.
    pub fn to_json(&self) -> serde_json::Value {
        let items: Vec<_> = self
            .items
            .iter()
            .map(|i| serde_json::json!({ "label": i.label_fr, "action": i.action.map(action_id), "enabled": i.enabled }))
            .collect();
        let icon = match self.icon {
            MenuIcon::Disarmed => "disarmed",
            MenuIcon::Armed => "armed",
            MenuIcon::Stopped => "stopped",
        };
        serde_json::json!({ "status": self.status_line(), "tooltip": self.tooltip(), "icon": icon, "items": items })
    }
}

fn action_id(action: MenuAction) -> &'static str {
    match action {
        MenuAction::OpenUi => "open",
        MenuAction::EStop => "estop",
        MenuAction::Quit => "quit",
    }
}

/// The menu for a gate state. An emergency stop overrides « armed » (the
/// output is cut whatever the gate says until the operator resets it).
pub fn menu_model(armed: bool, estop: bool, output: Option<&str>) -> MenuModel {
    let (status, icon) = if estop {
        ("Arrêt d'urgence", MenuIcon::Stopped)
    } else if armed {
        ("ARMÉ", MenuIcon::Armed)
    } else {
        ("Désarmé", MenuIcon::Disarmed)
    };
    let output = match output {
        Some(name) => format!("Sortie : {name}"),
        None => "Aperçu seulement".to_string(),
    };
    let item = |label_fr, action| MenuItem { label_fr, action: Some(action), enabled: true };
    MenuModel {
        status: status.to_string(),
        output,
        // The stop is always offered, even already latched: pressing it
        // again is harmless and the operator must never have to wonder.
        items: vec![item("Ouvrir l'interface", MenuAction::OpenUi), item("Arrêt", MenuAction::EStop), item("Quitter Laser Studio", MenuAction::Quit)],
        icon,
    }
}

/// The menu for the studio as it is now. Short lock; the e-stop latch is
/// read without it.
pub fn current_model(shared: &Mutex<Shared>, estop: &EStop) -> MenuModel {
    let latched = estop.is_latched();
    let s = shared.lock().unwrap_or_else(|e| e.into_inner());
    menu_model(s.gate.is_armed(), latched, s.output_name.as_deref())
}

/// What the menu's entries act on.
pub struct MenuActions {
    pub estop: Arc<EStop>,
    pub running: Arc<AtomicBool>,
    /// `http://127.0.0.1:<port>/`.
    pub url: String,
    /// False with `--no-open` (tests): « Ouvrir l'interface » only logs.
    pub open_browser: bool,
}

impl MenuActions {
    pub fn perform(&self, action: MenuAction, shared: &Mutex<Shared>) {
        match action {
            MenuAction::OpenUi => {
                if self.open_browser {
                    if let Err(e) = open_in_browser(&self.url) {
                        log::warn!("Ouverture de l'interface impossible : {e:#}");
                    }
                } else {
                    log::info!("Ouvrir l'interface : {} (--no-open)", self.url);
                }
            }
            MenuAction::EStop => {
                log::warn!("Arrêt d'urgence depuis la barre de menus");
                // Lock-free first, like Escape in the page (web.rs): the
                // DAC is cut before anything else can wait.
                self.estop.trip(ArmSource::Ui);
                if let Ok(mut s) = shared.try_lock() {
                    s.emergency_stop(ArmSource::Ui);
                }
                // Otherwise the engine records it at the top of its next frame.
            }
            MenuAction::Quit => {
                log::info!("Quitter depuis la barre de menus");
                // The Ctrl+C / SIGTERM path: cut now, then `main` shuts
                // down (disarm, dark frames, close the output).
                self.estop.kill_output();
                self.running.store(false, Ordering::SeqCst);
            }
        }
    }
}

/// What the studio thread hands to the menu once it is running (read by
/// the native item only).
#[cfg_attr(not(all(feature = "macos-app", target_os = "macos")), allow(dead_code))]
pub struct Ready {
    pub shared: Arc<Mutex<Shared>>,
    pub actions: Arc<MenuActions>,
}

/// Parses the `action` of `POST /api/test/menu` (`--test-hooks`).
pub fn parse_action(s: &str) -> Option<MenuAction> {
    [MenuAction::OpenUi, MenuAction::EStop, MenuAction::Quit].into_iter().find(|a| action_id(*a) == s)
}

/// Opens `url` in the default browser (`open`, macOS).
pub fn open_in_browser(url: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    let status = std::process::Command::new("/usr/bin/open").arg(url).status().context("open")?;
    anyhow::ensure!(status.success(), "open {url} : {status}");
    Ok(())
}

#[cfg(all(feature = "macos-app", target_os = "macos"))]
pub mod native {
    //! The NSStatusItem. Runs the AppKit event loop on the main thread;
    //! the studio itself (engine, web server, every other thread) runs
    //! unchanged on a thread of its own.

    use super::{current_model, MenuAction, MenuActions, MenuIcon, MenuModel, Ready};
    use crate::Shared;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBezierPath, NSColor, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
        NSVariableStatusItemLength,
    };
    use objc2_foundation::{ns_string, NSObject, NSPoint, NSRect, NSSize, NSString, NSTimer};
    use std::cell::{Cell, RefCell};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    pub struct TargetIvars {
        shared: Arc<Mutex<Shared>>,
        actions: Arc<MenuActions>,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; no Drop impl.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "LaserStudioMenuTarget"]
        #[ivars = TargetIvars]
        struct MenuTarget;

        impl MenuTarget {
            #[unsafe(method(openUi:))]
            fn open_ui(&self, _sender: Option<&AnyObject>) {
                self.act(MenuAction::OpenUi);
            }

            #[unsafe(method(estop:))]
            fn estop(&self, _sender: Option<&AnyObject>) {
                self.act(MenuAction::EStop);
            }

            #[unsafe(method(quit:))]
            fn quit(&self, _sender: Option<&AnyObject>) {
                self.act(MenuAction::Quit);
            }
        }
    );

    impl MenuTarget {
        fn new(mtm: MainThreadMarker, ivars: TargetIvars) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ivars);
            // SAFETY: NSObject's designated initialiser.
            unsafe { msg_send![super(this), init] }
        }

        fn act(&self, action: MenuAction) {
            let iv = self.ivars();
            iv.actions.perform(action, &iv.shared);
        }
    }

    /// The menu-bar icon, drawn here (our own, no image file): a laser
    /// aperture, a circle with a beam leaving it. A template image, so
    /// macOS tints it for the light and dark menu bar.
    fn icon(kind: MenuIcon) -> Retained<NSImage> {
        let draw = RcBlock::new(move |_rect: NSRect| -> Bool {
            NSColor::blackColor().set();
            let ring = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(NSPoint::new(2.5, 2.5), NSSize::new(10.0, 10.0)));
            ring.setLineWidth(1.5);
            if kind == MenuIcon::Armed {
                ring.fill();
            } else {
                ring.stroke();
            }
            let beam = NSBezierPath::bezierPath();
            beam.setLineWidth(1.5);
            beam.moveToPoint(NSPoint::new(11.0, 11.0));
            beam.lineToPoint(NSPoint::new(16.5, 16.5));
            if kind == MenuIcon::Stopped {
                // The stop: a slash across the aperture.
                beam.moveToPoint(NSPoint::new(1.0, 1.0));
                beam.lineToPoint(NSPoint::new(14.0, 14.0));
            }
            beam.stroke();
            Bool::YES
        });
        let image = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 18.0), false, &draw);
        image.setTemplate(true);
        image
    }

    fn entry(mtm: MainThreadMarker, title: &str, action: Option<objc2::runtime::Sel>, target: Option<&MenuTarget>) -> Retained<NSMenuItem> {
        // SAFETY: the selectors are methods of `MenuTarget`, set as the
        // target right below.
        let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), action, ns_string!("")) };
        if let Some(target) = target {
            // SAFETY: the target lives as long as the menu (kept below).
            unsafe { item.setTarget(Some(target)) };
        }
        item.setEnabled(action.is_some());
        item
    }

    fn show(status: &NSStatusItem, line: &NSMenuItem, model: &MenuModel, mtm: MainThreadMarker) {
        line.setTitle(&NSString::from_str(&model.status_line()));
        if let Some(button) = status.button(mtm) {
            button.setImage(Some(&icon(model.icon)));
            button.setToolTip(Some(&NSString::from_str(&model.tooltip())));
        }
    }

    /// Runs `studio` on its own thread and the menu-bar item on this one
    /// (which must be the main thread). Returns only if the studio stops
    /// before it is ready; otherwise the process exits with the studio's
    /// result once it has shut down cleanly.
    pub fn run<F>(studio: F) -> anyhow::Result<()>
    where
        F: FnOnce(Box<dyn FnOnce(Ready) + Send>) -> anyhow::Result<()> + Send + 'static,
    {
        let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("la barre de menus doit tourner sur le fil principal"))?;
        let (tx, rx) = std::sync::mpsc::channel::<Ready>();
        let thread: JoinHandle<anyhow::Result<()>> = std::thread::Builder::new().name("studio".into()).spawn(move || {
            studio(Box::new(move |ready| {
                let _ = tx.send(ready);
            }))
        })?;
        let Ok(ready) = rx.recv() else {
            // The studio stopped before it was running (an error, or a
            // second launch that only opened the first one's page).
            return thread.join().unwrap_or_else(|_| Err(anyhow::anyhow!("le studio s'est arrêté sur une panique")));
        };

        let app = NSApplication::sharedApplication(mtm);
        // A menu-bar app: no Dock icon, no main window.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let model = current_model(&ready.shared, &ready.actions.estop);
        let target = MenuTarget::new(mtm, TargetIvars { shared: Arc::clone(&ready.shared), actions: ready.actions });
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let line = entry(mtm, &model.status_line(), None, None);
        menu.addItem(&line);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for item in &model.items {
            let sel = match item.action {
                Some(MenuAction::OpenUi) => Some(sel!(openUi:)),
                Some(MenuAction::EStop) => Some(sel!(estop:)),
                Some(MenuAction::Quit) => Some(sel!(quit:)),
                None => None,
            };
            if item.action == Some(MenuAction::Quit) {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            let entry = entry(mtm, item.label_fr, sel, Some(&target));
            entry.setEnabled(item.enabled && sel.is_some());
            menu.addItem(&entry);
        }

        let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        status.setMenu(Some(&menu));
        show(&status, &line, &model, mtm);

        // Four times a second: refresh the status line and the icon, and
        // exit once the studio thread has shut down.
        let thread = RefCell::new(Some(thread));
        let last = Cell::new(Some(model));
        let shared = Arc::clone(&ready.shared);
        let tick = RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
            let _keep = (&target, &menu);
            if thread.borrow().as_ref().is_some_and(|t| t.is_finished()) {
                let code = match thread.borrow_mut().take().map(|t| t.join()) {
                    Some(Ok(Ok(()))) => 0,
                    Some(Ok(Err(e))) => {
                        eprintln!("Error: {e:#}");
                        1
                    }
                    _ => 101,
                };
                std::process::exit(code);
            }
            let model = current_model(&shared, &target.ivars().actions.estop);
            let previous = last.take();
            if previous.as_ref() != Some(&model) {
                show(&status, &line, &model, mtm);
            }
            last.set(Some(model));
        });
        // SAFETY: the block is retained by the timer, which the run loop keeps.
        let _timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.25, true, &tick) };
        app.run();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(m: &MenuModel) -> Vec<MenuAction> {
        m.items.iter().filter_map(|i| i.action).collect()
    }

    #[test]
    fn disarmed_in_preview() {
        let m = menu_model(false, false, None);
        assert_eq!(m.status_line(), "Désarmé — Aperçu seulement");
        assert_eq!(m.icon, MenuIcon::Disarmed);
        assert_eq!(actions(&m), [MenuAction::OpenUi, MenuAction::EStop, MenuAction::Quit]);
        assert!(m.items.iter().all(|i| i.enabled));
    }

    #[test]
    fn armed_with_an_output() {
        let m = menu_model(true, false, Some("Ether Dream 1"));
        assert_eq!(m.status_line(), "ARMÉ — Sortie : Ether Dream 1");
        assert_eq!(m.icon, MenuIcon::Armed);
        assert_eq!(m.tooltip(), "Laser Studio : ARMÉ — Sortie : Ether Dream 1");
    }

    #[test]
    fn an_estop_overrides_armed_and_the_stop_stays_offered() {
        let m = menu_model(true, true, Some("IDN"));
        assert_eq!(m.status, "Arrêt d'urgence");
        assert_eq!(m.icon, MenuIcon::Stopped);
        assert!(m.items.iter().any(|i| i.action == Some(MenuAction::EStop) && i.enabled));
    }

    #[test]
    fn nothing_in_the_menu_arms_and_labels_are_french() {
        for (armed, estop, out) in [(false, false, None), (true, false, Some("x")), (false, true, None), (true, true, Some("x"))] {
            let m = menu_model(armed, estop, out);
            let labels: Vec<&str> = m.items.iter().map(|i| i.label_fr).collect();
            assert_eq!(labels, ["Ouvrir l'interface", "Arrêt", "Quitter Laser Studio"]);
            for l in labels.iter().chain([m.status.as_str()].iter()) {
                let low = l.to_lowercase();
                assert!(!low.contains("armer") && !low.contains("réarm") && !low.contains("reset") && !low.contains("réinit"), "{l}");
            }
        }
    }

    #[test]
    fn test_hook_actions_parse() {
        assert_eq!(parse_action("open"), Some(MenuAction::OpenUi));
        assert_eq!(parse_action("estop"), Some(MenuAction::EStop));
        assert_eq!(parse_action("quit"), Some(MenuAction::Quit));
        assert_eq!(parse_action("arm"), None, "no arming entry");
    }

    #[test]
    fn quit_cuts_and_stops_running_estop_latches() {
        let shared = Mutex::new(crate::test_support::shared());
        let estop = Arc::clone(&shared.lock().unwrap().estop);
        let running = Arc::new(AtomicBool::new(true));
        let a = MenuActions { estop: Arc::clone(&estop), running: Arc::clone(&running), url: "http://127.0.0.1:1/".into(), open_browser: false };
        a.perform(MenuAction::OpenUi, &shared);
        assert!(running.load(Ordering::SeqCst) && !estop.is_latched());
        a.perform(MenuAction::EStop, &shared);
        assert!(estop.is_latched());
        assert!(!shared.lock().unwrap().gate.is_armed());
        assert!(running.load(Ordering::SeqCst), "the stop does not quit");
        a.perform(MenuAction::Quit, &shared);
        assert!(!running.load(Ordering::SeqCst));
    }
}
