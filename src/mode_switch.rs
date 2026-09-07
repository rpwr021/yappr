//! Native macOS NSSwitch toggle for the Dictate + Chat / Dictation Only tier.
//!
//! The `tray-icon`/`muda` menu API only offers text and checkmark items, so to
//! get the real pill-style switch (like Control Center's Wi-Fi toggle) we drop
//! to AppKit: build an `NSMenuItem` whose custom view hosts an `NSSwitch` plus a
//! label, then insert it into muda's underlying `NSMenu` (`ns_menu()`).
//!
//! The switch's target/action is a small Objective-C class defined here; its
//! action reads the new switch state and routes back into `runtime::handle_menu`
//! exactly as a normal menu click would, so persistence/restart logic is shared.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSBox, NSBoxType, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem,
    NSSwitch, NSTextField, NSTitlePosition, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

define_class!(
    // Target object for the switch's action. Holds nothing; the action reads the
    // switch state and forwards to the shared runtime handler.
    #[unsafe(super(objc2::runtime::NSObject))]
    #[name = "YapprModeSwitchTarget"]
    struct ModeSwitchTarget;

    impl ModeSwitchTarget {
        #[unsafe(method(modeSwitchToggled:))]
        fn mode_switch_toggled(&self, sender: &NSSwitch) {
            // On => Dictate + Chat, Off => Dictation Only. Route through the
            // shared handler so persistence and restart logging stay together.
            let on = sender.state() == NSControlStateValueOn;
            let tier = if on { "rich" } else { "poor" };
            update_labels(tier);
            if let Some(runtime) = crate::runtime::runtime() {
                runtime.handle_menu(&format!("mode:{tier}"));
            }
        }
    }
);

impl ModeSwitchTarget {
    fn new() -> Retained<Self> {
        unsafe { msg_send![Self::alloc(), init] }
    }
}

// Keep the target and labels alive for the process lifetime. The switch's
// `target` is a weak reference, and the action updates the text in place.
thread_local! {
    static TARGET: std::cell::RefCell<Option<Retained<ModeSwitchTarget>>> =
        const { std::cell::RefCell::new(None) };
    static TITLE_LABEL: std::cell::RefCell<Option<Retained<NSTextField>>> =
        const { std::cell::RefCell::new(None) };
    static DETAIL_LABEL: std::cell::RefCell<Option<Retained<NSTextField>>> =
        const { std::cell::RefCell::new(None) };
    static TRACK: std::cell::RefCell<Option<Retained<NSBox>>> =
        const { std::cell::RefCell::new(None) };
    static KNOB: std::cell::RefCell<Option<Retained<NSBox>>> =
        const { std::cell::RefCell::new(None) };
    static ACTIVE_IS_RICH: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn update_labels(selected_tier: &str) {
    let active_tier = ACTIVE_IS_RICH.with(|value| if value.get() { "rich" } else { "poor" });
    let (title, detail) = crate::ui::mode_switch_text(active_tier, selected_tier);
    let pending = selected_tier != active_tier;
    let accent = if pending {
        NSColor::systemOrangeColor()
    } else if selected_tier == "rich" {
        NSColor::systemGreenColor()
    } else {
        NSColor::systemBlueColor()
    };
    TITLE_LABEL.with(|slot| {
        if let Some(label) = slot.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(&title));
            label.setTextColor(Some(&accent));
        }
    });
    DETAIL_LABEL.with(|slot| {
        if let Some(label) = slot.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(&detail));
            let color = if pending {
                NSColor::systemOrangeColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            label.setTextColor(Some(&color));
        }
    });
    TRACK.with(|slot| {
        if let Some(track) = slot.borrow().as_ref() {
            track.setFillColor(&accent);
        }
    });
    KNOB.with(|slot| {
        if let Some(knob) = slot.borrow().as_ref() {
            let x = if selected_tier == "rich" {
                286.0
            } else {
                264.0
            };
            knob.setFrame(NSRect::new(NSPoint::new(x, 13.0), NSSize::new(20.0, 20.0)));
        }
    });
}

fn colored_box(
    frame: NSRect,
    radius: f64,
    color: &NSColor,
    mtm: MainThreadMarker,
) -> Retained<NSBox> {
    let box_view = NSBox::new(mtm);
    box_view.setFrame(frame);
    box_view.setBoxType(NSBoxType::Custom);
    box_view.setTitlePosition(NSTitlePosition::NoTitle);
    box_view.setBorderWidth(0.0);
    box_view.setCornerRadius(radius);
    box_view.setFillColor(color);
    box_view
}

/// Insert the NSSwitch toggle row into muda's underlying NSMenu, just below the
/// status/hint lines. `ns_menu` is the pointer from `ContextMenu::ns_menu()`.
/// `is_rich` sets the initial switch position. Best-effort: returns without
/// effect if the menu pointer is null or we're off the main thread.
pub fn install(ns_menu: *mut std::ffi::c_void, is_rich: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if ns_menu.is_null() {
        return;
    }
    // SAFETY: muda hands us a valid NSMenu pointer for the menu it owns; we only
    // read/insert while it's alive (during menu construction on the main thread).
    let menu: &NSMenu = unsafe { &*(ns_menu as *const NSMenu) };

    let target = ModeSwitchTarget::new();
    ACTIVE_IS_RICH.with(|value| value.set(is_rich));

    // Row container sized to the menu width-ish; AppKit lays the menu out.
    let row_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 46.0));
    let view = NSView::initWithFrame(mtm.alloc::<NSView>(), row_frame);

    let tier = if is_rich { "rich" } else { "poor" };
    let title = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    title.setFrame(NSRect::new(
        NSPoint::new(14.0, 23.0),
        NSSize::new(240.0, 18.0),
    ));
    view.addSubview(&title);
    TITLE_LABEL.with(|slot| *slot.borrow_mut() = Some(title.clone()));

    let detail = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    detail.setFrame(NSRect::new(
        NSPoint::new(14.0, 5.0),
        NSSize::new(260.0, 18.0),
    ));
    view.addSubview(&detail);
    DETAIL_LABEL.with(|slot| *slot.borrow_mut() = Some(detail.clone()));

    // Draw the visible switch ourselves because AppKit always renders an
    // NSSwitch's Off state gray. The native switch remains invisibly on top as
    // the click target and accessibility element.
    let track = colored_box(
        NSRect::new(NSPoint::new(262.0, 11.0), NSSize::new(46.0, 24.0)),
        12.0,
        &NSColor::systemGreenColor(),
        mtm,
    );
    view.addSubview(&track);
    TRACK.with(|slot| *slot.borrow_mut() = Some(track.clone()));

    let knob = colored_box(
        NSRect::new(NSPoint::new(286.0, 13.0), NSSize::new(20.0, 20.0)),
        10.0,
        &NSColor::whiteColor(),
        mtm,
    );
    view.addSubview(&knob);
    KNOB.with(|slot| *slot.borrow_mut() = Some(knob.clone()));

    update_labels(tier);

    let switch = NSSwitch::new(mtm);
    switch.setFrame(NSRect::new(
        NSPoint::new(262.0, 11.0),
        NSSize::new(46.0, 24.0),
    ));
    switch.setState(if is_rich {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    switch.setAlphaValue(0.01);
    // SAFETY: target outlives the switch (held in TARGET); selector matches the
    // action method defined on ModeSwitchTarget.
    unsafe {
        switch.setTarget(Some(&*target as &AnyObject));
        switch.setAction(Some(sel!(modeSwitchToggled:)));
    }
    view.addSubview(&switch);

    let item = NSMenuItem::new(mtm);
    item.setView(Some(&view));
    // Sit directly under the first separator, which muda places after the status
    // line and the two hotkey hints. Searching for the separator instead of
    // hardcoding index 4 means adding or reordering those rows can't silently
    // misplace the switch.
    let count = menu.numberOfItems();
    let mut index = 4.min(count);
    for i in 0..count {
        if let Some(row) = menu.itemAtIndex(i) {
            if row.isSeparatorItem() {
                index = (i + 1).min(count);
                break;
            }
        }
    }
    menu.insertItem_atIndex(&item, index);

    TARGET.with(|t| *t.borrow_mut() = Some(target));
}
