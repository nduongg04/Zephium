//! The system's AutoFill passwords panel, the one a page's right-click
//! AutoFill ▸ Passwords… opens, shown on request at the field in focus.
//!
//! It is never shown by itself on focus: the panel is drawn by another
//! process that takes the keyboard while it is open, so typing that follows a
//! click into a field would go to the panel instead of the field.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel, ClassType, MainThreadOnly};
use objc2_foundation::{MainThreadMarker, NSString};
use objc2_web_kit::{WKContentWorld, WKUserScript, WKUserScriptInjectionTime, WKWebView};

/// Puts WebKit's AutoFill key in sign-in fields of `page`; a click on it
/// opens the panel through the view's AutoFill button handler. It needs a
/// content world allowed to drive AutoFill, new in macOS 27; before that the
/// fields stay plain and Edit > AutoFill Password remains.
pub(crate) fn show_key_in_sign_in_fields(page: &WKWebView) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(world) = autofill_world() else {
        return;
    };
    let source = NSString::from_str(include_str!("autofill_key.js"));
    // SAFETY: main-thread WebKit calls on a live view and world.
    unsafe {
        let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
            WKUserScript::alloc(mtm),
            &source,
            WKUserScriptInjectionTime::AtDocumentEnd,
            false,
            &world,
        );
        page.configuration()
            .userContentController()
            .addUserScript(&script);
    }
}

fn autofill_world() -> Option<Retained<WKContentWorld>> {
    thread_local! {
        // WebKit cannot hand back a world made from a configuration, so the
        // one world all views share is kept here.
        static WORLD: OnceCell<Option<Retained<WKContentWorld>>> = const { OnceCell::new() };
    }
    WORLD.with(|world| {
        world
            .get_or_init(|| {
                let configuration = AnyClass::get(c"WKContentWorldConfiguration")?;
                let make = sel!(worldWithConfiguration:);
                // SAFETY: public macOS 27 API, checked for before each call.
                unsafe {
                    let made: bool = msg_send![WKContentWorld::class(), respondsToSelector: make];
                    let configuration: Retained<AnyObject> = msg_send![configuration, new];
                    let enable = sel!(setAutofillScriptingEnabled:);
                    let enables: bool = msg_send![&*configuration, respondsToSelector: enable];
                    if !made || !enables {
                        return None;
                    }
                    let _: () = msg_send![&*configuration, setAutofillScriptingEnabled: true];
                    msg_send![WKContentWorld::class(), worldWithConfiguration: &*configuration]
                }
            })
            .clone()
    })
}

/// Shows the panel for the field focused in `page`. False when this macOS
/// lacks the panel, or when `page` does not have the keyboard or no text
/// field in it has focus: WebKit has an input context only for an editable
/// element.
pub(crate) fn show_passwords(page: &WKWebView) -> bool {
    let Some(window) = page.window() else {
        return false;
    };
    let typing_here = window.isKeyWindow()
        && window.attachedSheet().is_none()
        && window
            .firstResponder()
            .is_some_and(|responder| std::ptr::addr_eq(&*responder, page));
    if !typing_here {
        return false;
    }
    let Some(context) = page.inputContext() else {
        return false;
    };
    let Some(class) = AnyClass::get(c"NSAutoFillHeuristicController") else {
        return false;
    };
    // SAFETY: main-thread AppKit calls, each guarded by respondsToSelector:
    // on the object it is sent to; the input context is retained above.
    unsafe {
        let shared: bool = msg_send![class, respondsToSelector: sel!(sharedInstance)];
        if !shared {
            return false;
        }
        let controller: Option<Retained<AnyObject>> = msg_send![class, sharedInstance];
        let Some(controller) = controller else {
            return false;
        };
        let show = sel!(showAutoFillPasswordsPopoverForTextInputContext:);
        let shows: bool = msg_send![&*controller, respondsToSelector: show];
        if !shows {
            return false;
        }
        let _: () = msg_send![
            &*controller,
            showAutoFillPasswordsPopoverForTextInputContext: &*context
        ];
    }
    true
}

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("autofill.rs");

    fn body() -> &'static str {
        SOURCE.split("#[cfg(test)]").next().unwrap()
    }

    #[test]
    fn the_panel_opens_only_for_a_page_typing_in_the_key_window() {
        let body = body();
        for guard in [
            "window.isKeyWindow()",
            "attachedSheet().is_none()",
            "std::ptr::addr_eq(&*responder, page)",
            "page.inputContext()",
        ] {
            assert!(body.contains(guard), "{guard}");
        }
    }

    #[test]
    fn every_private_call_is_guarded() {
        let body = body();
        let at = |text: &str| body.find(text).unwrap_or_else(|| panic!("{text}"));
        assert!(
            at("respondsToSelector: sel!(sharedInstance)") < at("msg_send![class, sharedInstance]")
        );
        assert!(
            at("respondsToSelector: show")
                < at("showAutoFillPasswordsPopoverForTextInputContext: &*context")
        );
    }

    #[test]
    fn the_key_waits_for_focus_and_skips_new_passwords() {
        let script = include_str!("autofill_key.js");
        for idle in [
            "MutationObserver",
            "setTimeout",
            "setInterval",
            "querySelectorAll",
        ] {
            assert!(!script.contains(idle), "{idle}");
        }
        assert!(script.contains(r#""focusin""#) && script.contains(r#""focusout""#));
        assert!(script.contains("new-password|one-time-code"));
    }

    #[test]
    fn this_macos_draws_the_key_only_from_its_own_world() {
        let made = objc2::runtime::AnyClass::get(c"WKContentWorldConfiguration").is_some();
        let major = objc2_foundation::NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion;
        assert_eq!(made, major >= 27);
    }
}
