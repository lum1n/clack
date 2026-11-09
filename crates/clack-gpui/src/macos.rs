//! macOS menu bar, Accessibility, and keystrokes aimed at one process.
//!
//! GPUI 0.2 always builds a titled window and will not make an `NSStatusItem`.
//! The HUD is restyled after it opens so it cannot become the key window.
//! Keystrokes go to the app that was frontmost when the hotkey went down,
//! posted with `CGEventPostToPid`. Clack is never activated.

use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;
use std::time::Duration;

use cocoa::appkit::{
    NSApplication, NSApplicationActivationPolicy, NSStatusBar, NSStatusItem, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use cocoa::base::{id, nil, BOOL, NO, YES};
use cocoa::foundation::{NSAutoreleasePool, NSString};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{CGEvent, CGEventFlags};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc::declare::ClassDecl;
use objc::rc::StrongPtr;
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

pub const ACCESSIBILITY_HINT: &str = "Accessibility is off for this binary, so text is not inserted. Microphone permission does not type. Enable Clack in System Settings → Privacy & Security → Accessibility. Quit and open Clack again if it still does not type.";

const HUD_TITLE: &str = "Clack HUD";
/// `NSWindowStyleMaskNonactivatingPanel`. Combined with an empty border mask
/// this is a borderless panel that does not activate the app.
const NONACTIVATING_PANEL: u64 = 1 << 7;
/// `NSPopUpWindowLevel`, above normal windows including fullscreen auxiliaries.
const POPUP_LEVEL: i64 = 101;
/// `NSApplicationActivateIgnoringOtherApps`.
const ACTIVATE_IGNORING_OTHERS: usize = 1 << 1;
const BACKSPACE_KEYCODE: u16 = 0x33;
const RETURN_KEYCODE: u16 = 0x24;

static STATUS_CLICK: AtomicBool = AtomicBool::new(false);
static TARGET_CLASS: std::sync::atomic::AtomicPtr<objc::runtime::Class> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

pub fn accessory_policy() {
    unsafe {
        let app = NSApplication::sharedApplication(nil);
        let _: BOOL = app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );
    }
}

pub fn install_status_item() {
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        ensure_target_class();
        let status_bar = NSStatusBar::systemStatusBar(nil);
        let item = StrongPtr::retain(
            status_bar.statusItemWithLength_(cocoa::appkit::NSSquareStatusItemLength),
        );
        let target: id = msg_send![target_class(), new];
        let target = StrongPtr::retain(target);
        let button = NSStatusItem::button(*item);
        let symbol = ns_string("waveform");
        let label = ns_string("Clack");
        let image: id = msg_send![
            class!(NSImage),
            imageWithSystemSymbolName: symbol
            accessibilityDescription: label
        ];
        if button.is_null() {
            let _: () = msg_send![*item, setTitle: label];
            let _: () = msg_send![*item, setTarget: *target];
            let _: () = msg_send![*item, setAction: sel!(onClick:)];
        } else if image.is_null() {
            let _: () = msg_send![button, setTitle: label];
            let _: () = msg_send![button, setTarget: *target];
            let _: () = msg_send![button, setAction: sel!(onClick:)];
        } else {
            let _: () = msg_send![image, setTemplate: YES];
            let _: () = msg_send![button, setImage: image];
            let _: () = msg_send![button, setTarget: *target];
            let _: () = msg_send![button, setAction: sel!(onClick:)];
        }
        // The bar removes the item when the last retain goes away.
        std::mem::forget(item);
        std::mem::forget(target);
        pool.drain();
    }
}

pub fn take_status_click() -> bool {
    STATUS_CLICK.swap(false, Ordering::SeqCst)
}

pub fn is_trusted(prompt: bool) -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let value = if prompt {
            CFBoolean::true_value()
        } else {
            CFBoolean::false_value()
        };
        let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

pub fn prompt_accessibility() {
    if !is_trusted(false) {
        let _ = is_trusted(true);
    }
}

/// Pid of the frontmost app when it is not this process.
pub fn frontmost_other_pid() -> Option<i32> {
    let pid = frontmost_pid()?;
    if pid == std::process::id() as i32 {
        None
    } else {
        Some(pid)
    }
}

pub fn focus_target(pid: i32) {
    accessory_policy();
    resign_key_window();
    if frontmost_pid() == Some(pid) {
        return;
    }
    activate_pid(pid);
    // Activation returns before the other app is actually key. A short wait
    // lets the field accept the events that follow. Skipped when it is already front.
    std::thread::sleep(Duration::from_millis(20));
}

pub fn configure_hud_window() {
    unsafe {
        let app = NSApplication::sharedApplication(nil);
        let windows: id = msg_send![app, windows];
        if windows.is_null() {
            return;
        }
        let count: usize = msg_send![windows, count];
        for index in 0..count {
            let window: id = msg_send![windows, objectAtIndex: index];
            if window.is_null() || !is_hud_window(window) {
                continue;
            }
            let frame = NSWindow::frame(window);
            let mask = NSWindowStyleMask::from_bits_retain(NONACTIVATING_PANEL);
            window.setStyleMask_(mask);
            window.setTitle_(ns_string(""));
            window.setHasShadow_(YES);
            window.setMovable_(NO);
            window.setHidesOnDeactivate_(NO);
            window.setIgnoresMouseEvents_(YES);
            window.setLevel_(POPUP_LEVEL);
            window.setCollectionBehavior_(
                NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                    | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary
                    | NSWindowCollectionBehavior::NSWindowCollectionBehaviorIgnoresCycle
                    | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary,
            );
            let _: () = msg_send![window, setBecomesKeyOnlyIfNeeded: YES];
            window.setFrame_display_(frame, YES);
            window.orderFront_(nil);
            if window.isKeyWindow() == YES {
                let _: () = msg_send![window, resignKeyWindow];
            }
            break;
        }
    }
}

pub fn type_text(pid: i32, text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    let source = event_source()?;
    for mut chunk in utf16_chunks(text, 20) {
        if chunk.starts_with(['\n', '\r']) {
            key_click(pid, &source, RETURN_KEYCODE)?;
            chunk = &chunk[1..];
            if chunk.is_empty() {
                continue;
            }
        }
        let event = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| "could not create a keyboard event".to_string())?;
        event.set_string(chunk);
        event.set_flags(CGEventFlags::CGEventFlagNull);
        event.post_to_pid(pid as libc::pid_t);
    }
    Ok(())
}

pub fn backspace(pid: i32, count: usize) -> Result<(), String> {
    if count == 0 {
        return Ok(());
    }
    let source = event_source()?;
    for _ in 0..count {
        key_click(pid, &source, BACKSPACE_KEYCODE)?;
    }
    Ok(())
}

fn key_click(pid: i32, source: &CGEventSource, keycode: u16) -> Result<(), String> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(source.clone(), keycode, down)
            .map_err(|_| "could not create a keyboard event".to_string())?;
        event.set_flags(CGEventFlags::CGEventFlagNull);
        event.post_to_pid(pid as libc::pid_t);
    }
    Ok(())
}

fn event_source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| "could not create a keyboard event source".to_string())
}

fn utf16_chunks(text: &str, limit: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        let next = ch.len_utf16();
        if units > 0 && units + next > limit {
            out.push(&text[start..index]);
            start = index;
            units = 0;
        }
        units += next;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn is_hud_window(window: id) -> bool {
    if title_eq(window, HUD_TITLE) {
        return true;
    }
    // The strip is 44pt. The settings popover is 640pt, so a short panel is the HUD
    // even when it was opened with no title.
    let frame = unsafe { NSWindow::frame(window) };
    frame.size.height > 20.0 && frame.size.height < 120.0
}

fn frontmost_pid() -> Option<i32> {
    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let front: id = msg_send![workspace, frontmostApplication];
        if front.is_null() {
            return None;
        }
        let pid: i32 = msg_send![front, processIdentifier];
        Some(pid)
    }
}

fn activate_pid(pid: i32) {
    unsafe {
        let running: id = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if running.is_null() {
            return;
        }
        let _: BOOL = msg_send![running, activateWithOptions: ACTIVATE_IGNORING_OTHERS];
    }
}

fn resign_key_window() {
    unsafe {
        let app = NSApplication::sharedApplication(nil);
        let key: id = msg_send![app, keyWindow];
        if key.is_null() {
            return;
        }
        let _: () = msg_send![key, resignKeyWindow];
    }
}

fn title_eq(window: id, expected: &str) -> bool {
    unsafe {
        let title = NSWindow::title(window);
        if title.is_null() {
            return false;
        }
        let bytes: *const i8 = msg_send![title, UTF8String];
        if bytes.is_null() {
            return false;
        }
        CStr::from_ptr(bytes).to_bytes() == expected.as_bytes()
    }
}

fn ns_string(text: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(text) }
}

fn ensure_target_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let mut decl = ClassDecl::new("ClackStatusTarget", class!(NSObject)).unwrap();
        decl.add_method(sel!(onClick:), on_click as extern "C" fn(&Object, Sel, id));
        let class = decl.register();
        TARGET_CLASS.store(
            class as *const objc::runtime::Class as *mut _,
            Ordering::SeqCst,
        );
    });
}

fn target_class() -> *const objc::runtime::Class {
    TARGET_CLASS.load(Ordering::SeqCst)
}

extern "C" fn on_click(_this: &Object, _sel: Sel, _sender: id) {
    STATUS_CLICK.store(true, Ordering::SeqCst);
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}
