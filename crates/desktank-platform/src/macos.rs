//! macOS power and session event sources (SPEC §6.2, O3), overlay window
//! configuration and full-screen app detection (SPEC §3.2, §6.2, O1).
//!
//! Uses event-driven OS notifications with zero polling timers:
//! - `IOKit` `IOPSNotificationCreateRunLoopSource` for AC/battery/percentage;
//! - `NSProcessInfoPowerStateDidChangeNotification` for Low Power Mode;
//! - `NSWorkspace` notifications for display sleep and wake;
//! - `NSDistributedNotificationCenter` for screen lock and unlock;
//! - `NSWorkspace` Space and application notifications, then a
//!   `CGWindowListCopyWindowInfo` bounds check, for full-screen apps.

use core::cell::RefCell;
use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, Message, class, msg_send};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFRunLoop, CFRunLoopSource, CFString,
    CFType, kCFRunLoopCommonModes,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSObjectProtocol,
    NSProcessInfo, NSProcessInfoPowerStateDidChangeNotification, NSRect, NSString,
};

use desktank_render::wgpu::rwh::RawWindowHandle;

use crate::modifiers::Modifiers;
use crate::overlay::{Interactivity, OverlayError, OverlayOptions};
use crate::power::{PowerSignals, PowerSource};

/// `IOPSCopyPowerSourcesInfo` / `IOPSNotificationCreateRunLoopSource` keys from
/// `<IOKit/ps/IOPSKeys.h>`.
const BATTERY_POWER: &str = "Battery Power";
const UPS_POWER: &str = "UPS Power";
const KEY_IS_PRESENT: &str = "Is Present";
const KEY_CURRENT_CAPACITY: &str = "Current Capacity";
const KEY_MAX_CAPACITY: &str = "Max Capacity";

const DISPLAY_SLEEP_NOTIFICATION: &str = "NSWorkspaceScreensDidSleepNotification";
const DISPLAY_WAKE_NOTIFICATION: &str = "NSWorkspaceScreensDidWakeNotification";
const SCREEN_LOCKED_NOTIFICATION: &str = "com.apple.screenIsLocked";
const SCREEN_UNLOCKED_NOTIFICATION: &str = "com.apple.screenIsUnlocked";

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> *const CFType;
    fn IOPSCopyPowerSourcesList(blob: &CFType) -> *const CFArray;
    fn IOPSGetPowerSourceDescription(blob: &CFType, ps: *const c_void) -> *const CFDictionary;
    fn IOPSGetProvidingPowerSourceType(snapshot: &CFType) -> *const CFString;
    fn IOPSNotificationCreateRunLoopSource(
        callback: extern "C" fn(*mut c_void),
        context: *mut c_void,
    ) -> *mut CFRunLoopSource;
}

// Link AppKit so `NSWorkspace` resolves even in binaries that don't otherwise
// depend on AppKit (such as `desktank-platform` unit tests).
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const CFArray;
    fn CGRectMakeWithDictionaryRepresentation(dict: &CFDictionary, rect: *mut Rect) -> bool;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> Rect;
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

struct Shared {
    signals: Mutex<PowerSignals>,
    on_change: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut PowerSignals)) {
        let changed = {
            let mut guard = self.signals.lock().unwrap_or_else(PoisonError::into_inner);
            let before = *guard;
            f(&mut guard);
            *guard != before
        };
        if changed {
            (self.on_change)();
        }
    }

    fn signals(&self) -> PowerSignals {
        *self.signals.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct IopsRegistration {
    run_loop: CFRetained<CFRunLoop>,
    source: CFRetained<CFRunLoopSource>,
    ctx: *const Shared,
}

type ObserverEntry = (
    Retained<NSNotificationCenter>,
    Retained<ProtocolObject<dyn NSObjectProtocol>>,
);

pub(crate) struct PowerMonitor {
    shared: Arc<Shared>,
    observers: Vec<ObserverEntry>,
    iops: Option<IopsRegistration>,
    // Keep Drop on the creating thread so `iops` run-loop teardown matches the
    // thread whose CFRunLoop drives `iops_callback`.
    _not_send: PhantomData<*mut ()>,
}

impl PowerMonitor {
    pub(crate) fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        let initial = PowerSignals {
            source: read_power_source(),
            low_power_mode: is_low_power_mode(),
            display_asleep: false,
            session_locked: false,
        };
        let shared = Arc::new(Shared {
            signals: Mutex::new(initial),
            on_change: Box::new(on_change),
        });

        let process_center = NSNotificationCenter::defaultCenter();
        let workspace_center = workspace_notification_center();
        let distributed_center =
            Retained::into_super(NSDistributedNotificationCenter::defaultCenter());
        let observers = register_observers(
            &shared,
            &process_center,
            workspace_center.as_deref(),
            &distributed_center,
        );
        let iops = register_iops(&shared);

        Self {
            shared,
            observers,
            iops,
            _not_send: PhantomData,
        }
    }

    pub(crate) fn signals(&self) -> PowerSignals {
        self.shared.signals()
    }
}

impl Drop for PowerMonitor {
    fn drop(&mut self) {
        for (center, observer) in self.observers.drain(..) {
            // SAFETY: `observer` is the token returned by
            // `addObserverForName:object:queue:usingBlock:` on `center`, and
            // is removed at most once here.
            unsafe { center.removeObserver(observer.as_ref()) };
        }
        if let Some(iops) = self.iops.take() {
            // SAFETY: `kCFRunLoopCommonModes` is a valid CoreFoundation mode constant.
            iops.run_loop
                .remove_source(Some(&iops.source), unsafe { kCFRunLoopCommonModes });
            iops.source.invalidate();
            drop(iops.source);
            // SAFETY: `iops.source` was removed from the creating thread's run
            // loop (`!Send`) and invalidated above so `iops_callback` can no
            // longer fire; this balances `Arc::into_raw` in `register_iops`.
            unsafe { drop(Arc::from_raw(iops.ctx)) };
        }
    }
}

fn register_observers(
    shared: &Arc<Shared>,
    process_center: &NSNotificationCenter,
    workspace_center: Option<&NSNotificationCenter>,
    distributed_center: &NSNotificationCenter,
) -> Vec<ObserverEntry> {
    let mut observers = Vec::with_capacity(5);

    // SAFETY: `NSProcessInfoPowerStateDidChangeNotification` is a valid static
    // `NSNotificationName` constant from Foundation.
    let low_power_name = unsafe { NSProcessInfoPowerStateDidChangeNotification };
    let s = Arc::clone(shared);
    observers.push((
        process_center.retain(),
        observe_center(process_center, low_power_name, move || {
            let enabled = is_low_power_mode();
            s.update(|sig| sig.low_power_mode = enabled);
        }),
    ));

    if let Some(workspace_center) = workspace_center {
        let sleep_name = NSString::from_str(DISPLAY_SLEEP_NOTIFICATION);
        let s = Arc::clone(shared);
        observers.push((
            workspace_center.retain(),
            observe_center(workspace_center, &sleep_name, move || {
                s.update(|sig| sig.display_asleep = true);
            }),
        ));

        let wake_name = NSString::from_str(DISPLAY_WAKE_NOTIFICATION);
        let s = Arc::clone(shared);
        observers.push((
            workspace_center.retain(),
            observe_center(workspace_center, &wake_name, move || {
                s.update(|sig| sig.display_asleep = false);
            }),
        ));
    }

    let lock_name = NSString::from_str(SCREEN_LOCKED_NOTIFICATION);
    let s = Arc::clone(shared);
    observers.push((
        distributed_center.retain(),
        observe_center(distributed_center, &lock_name, move || {
            s.update(|sig| sig.session_locked = true);
        }),
    ));

    let unlock_name = NSString::from_str(SCREEN_UNLOCKED_NOTIFICATION);
    let s = Arc::clone(shared);
    observers.push((
        distributed_center.retain(),
        observe_center(distributed_center, &unlock_name, move || {
            s.update(|sig| sig.session_locked = false);
        }),
    ));

    observers
}

fn observe_center(
    center: &NSNotificationCenter,
    name: &NSString,
    handler: impl Fn() + Send + Sync + 'static,
) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
        handler();
    });
    // SAFETY: `name` is a valid `NSNotificationName` (`NSString`), `obj` and
    // `queue` are `None` (deliver synchronously on the posting thread, where
    // `handler` is `Send + Sync`), and `RcBlock` keeps the block valid across
    // `Block_copy` inside Foundation until `removeObserver:` runs in `Drop`.
    unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block) }
}

fn workspace_notification_center() -> Option<Retained<NSNotificationCenter>> {
    // SAFETY: `NSWorkspace` is a standard AppKit class (linked via `AppKit`),
    // `+sharedWorkspace` returns `NSWorkspace *`, and `-notificationCenter`
    // returns its `NSNotificationCenter *` (possibly nil only in degraded
    // environments).
    unsafe {
        let workspace: Option<Retained<AnyObject>> =
            msg_send![class!(NSWorkspace), sharedWorkspace];
        let workspace = workspace?;
        msg_send![&*workspace, notificationCenter]
    }
}

fn register_iops(shared: &Arc<Shared>) -> Option<IopsRegistration> {
    let run_loop = CFRunLoop::current()?;
    let ctx_ptr = Arc::into_raw(Arc::clone(shared));
    // SAFETY: `iops_callback` only borrows `ctx_ptr` as `&Shared` while the
    // run-loop source is valid; if source creation fails we reclaim `ctx_ptr`
    // immediately below, otherwise `Drop` removes & invalidates the source
    // before reclaiming `ctx_ptr`.
    let source_ptr =
        unsafe { IOPSNotificationCreateRunLoopSource(iops_callback, ctx_ptr.cast_mut().cast()) };
    let Some(source_nn) = NonNull::new(source_ptr) else {
        // SAFETY: Source creation failed, so `iops_callback` cannot run; balance
        // `Arc::into_raw` above.
        unsafe { drop(Arc::from_raw(ctx_ptr)) };
        return None;
    };
    // SAFETY: `IOPSNotificationCreateRunLoopSource` returns a newly created
    // `CFRunLoopSourceRef` owned by the caller.
    let source = unsafe { CFRetained::from_raw(source_nn) };
    // SAFETY: `kCFRunLoopCommonModes` is a valid CoreFoundation mode constant.
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    Some(IopsRegistration {
        run_loop,
        source,
        ctx: ctx_ptr,
    })
}

extern "C" fn iops_callback(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    // SAFETY: `context` came from `Arc::into_raw` in `register_iops`, and
    // remains valid until `Drop` removes and invalidates this run-loop source on
    // the same thread.
    let shared = unsafe { &*context.cast::<Shared>() };
    let source = read_power_source();
    shared.update(|sig| sig.source = source);
}

fn is_low_power_mode() -> bool {
    NSProcessInfo::processInfo().isLowPowerModeEnabled()
}

fn read_power_source() -> PowerSource {
    // SAFETY: `IOPSCopyPowerSourcesInfo` takes no arguments and returns an owned
    // `CFTypeRef` (or NULL on error), which we wrap in `CFRetained::from_raw`.
    let Some(blob_nn) = NonNull::new(unsafe { IOPSCopyPowerSourcesInfo() }.cast_mut()) else {
        return PowerSource::Ac;
    };
    // SAFETY: `IOPSCopyPowerSourcesInfo` follows the CoreFoundation Copy rule.
    let blob = unsafe { CFRetained::from_raw(blob_nn) };

    // SAFETY: `blob` is a valid snapshot from `IOPSCopyPowerSourcesInfo`; the
    // returned `CFStringRef` follows the Get rule (borrowed for `blob`'s
    // lifetime) or is NULL.
    let providing = NonNull::new(unsafe { IOPSGetProvidingPowerSourceType(&blob) }.cast_mut())
        .map(|p| unsafe { p.as_ref() });

    // SAFETY: `blob` is valid; `IOPSCopyPowerSourcesList` returns an owned
    // `CFArrayRef` (or NULL).
    let Some(list_nn) = NonNull::new(unsafe { IOPSCopyPowerSourcesList(&blob) }.cast_mut()) else {
        return parse_power_source(providing, &[]);
    };
    // SAFETY: `IOPSCopyPowerSourcesList` follows the CoreFoundation Copy rule.
    let list = unsafe { CFRetained::from_raw(list_nn) };

    let count = list.count();
    let mut descriptions = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for i in 0..count {
        // SAFETY: `0 <= i < count` is within bounds of `list`.
        let ps = unsafe { list.value_at_index(i) };
        if ps.is_null() {
            continue;
        }
        // SAFETY: `blob` and `ps` (element of `list` derived from `blob`) are
        // valid; the returned `CFDictionaryRef` follows the Get rule and stays
        // alive as long as `blob` does.
        let desc_ptr = unsafe { IOPSGetPowerSourceDescription(&blob, ps) };
        if let Some(desc_nn) = NonNull::new(desc_ptr.cast_mut()) {
            // SAFETY: `desc_nn` points to a valid `CFDictionary` alive for
            // `&blob`.
            descriptions.push(unsafe { desc_nn.as_ref() });
        }
    }
    parse_power_source(providing, &descriptions)
}

fn parse_power_source(providing: Option<&CFString>, descriptions: &[&CFDictionary]) -> PowerSource {
    let on_battery = providing.is_some_and(|s| {
        let name = s.to_string();
        name == BATTERY_POWER || name == UPS_POWER
    });
    if !on_battery {
        return PowerSource::Ac;
    }

    let present_key = CFString::from_static_str(KEY_IS_PRESENT);
    let current_key = CFString::from_static_str(KEY_CURRENT_CAPACITY);
    let max_key = CFString::from_static_str(KEY_MAX_CAPACITY);

    let mut total_current: i64 = 0;
    let mut total_max: i64 = 0;
    for dict in descriptions {
        if let Some(present) = dict_value::<CFBoolean>(dict, &present_key)
            && !present.as_bool()
        {
            continue;
        }
        let Some(cur) = dict_value::<CFNumber>(dict, &current_key).and_then(CFNumber::as_i64)
        else {
            continue;
        };
        let Some(max) = dict_value::<CFNumber>(dict, &max_key).and_then(CFNumber::as_i64) else {
            continue;
        };
        if cur >= 0 && max > 0 {
            total_current = total_current.saturating_add(cur);
            total_max = total_max.saturating_add(max);
        }
    }

    let percent = if total_max > 0 {
        let pct = (total_current.saturating_mul(100) / total_max).clamp(0, 100);
        u8::try_from(pct).ok()
    } else {
        None
    };
    PowerSource::Battery { percent }
}

fn dict_value<'a, T: objc2_core_foundation::ConcreteType>(
    dict: &'a CFDictionary,
    key: &CFString,
) -> Option<&'a T> {
    let key_ptr: *const CFString = key;
    // SAFETY: `dict` is a valid property-list `CFDictionary` whose keys and
    // values are CoreFoundation types; `key_ptr` is a valid `&CFString`.
    let raw = unsafe { dict.value(key_ptr.cast()) };
    let val_nn = NonNull::new(raw.cast_mut())?;
    // SAFETY: Non-null values in CoreFoundation property-list dictionaries
    // (`IOPSGetPowerSourceDescription`) are `CFType` instances alive for `'a`.
    let cf_type: &'a CFType = unsafe { val_nn.cast::<CFType>().as_ref() };
    cf_type.downcast_ref::<T>()
}

// --- Overlay window (SPEC §3.2) ---

/// `NSFloatingWindowLevel`: above normal windows (0), below the Dock (20) and
/// the menu bar (24).
const NS_FLOATING_WINDOW_LEVEL: isize = 3;
/// `NSWindowCollectionBehaviorCanJoinAllSpaces | …Stationary | …IgnoresCycle`.
/// Deliberately not `…FullScreenAuxiliary` (`1 << 8`), so the overlay never
/// appears in full-screen Spaces.
const OVERLAY_COLLECTION_BEHAVIOR: usize = (1 << 0) | (1 << 4) | (1 << 6);
const NS_WINDOW_SHARING_NONE: usize = 0;
const NS_WINDOW_SHARING_READ_ONLY: usize = 1;

/// Resolves the `NSWindow` behind an `AppKit` window handle.
fn appkit_window(handle: RawWindowHandle) -> Result<Retained<AnyObject>, OverlayError> {
    let RawWindowHandle::AppKit(handle) = handle else {
        return Err(OverlayError::UnsupportedHandle);
    };
    if MainThreadMarker::new().is_none() {
        return Err(OverlayError::NotMainThread);
    }
    // SAFETY: `ns_view` is the `NSView *` the window provider vends through
    // `HasWindowHandle`, valid for the handle's borrow, and we are on the main
    // thread as AppKit requires.
    let view: &AnyObject = unsafe { handle.ns_view.cast::<AnyObject>().as_ref() };
    // SAFETY: `-[NSView window]` returns `NSWindow *` or nil.
    let window: Option<Retained<AnyObject>> = unsafe { msg_send![view, window] };
    window.ok_or(OverlayError::NoWindow)
}

pub(crate) fn configure_overlay_window(
    handle: RawWindowHandle,
    opts: OverlayOptions,
) -> Result<(), OverlayError> {
    let window = appkit_window(handle)?;
    let sharing = sharing_type(opts.exclude_from_capture);
    // SAFETY: `window` is a live `NSWindow` and we are on the main thread
    // (checked in `appkit_window`); each selector below is an `NSWindow`
    // setter taking the argument type used (`NSInteger`, `NSUInteger`, `BOOL`
    // or an `NSColor *`), and `+[NSColor clearColor]` returns a valid colour.
    unsafe {
        let clear: Retained<AnyObject> = msg_send![class!(NSColor), clearColor];
        let _: () = msg_send![&*window, setLevel: NS_FLOATING_WINDOW_LEVEL];
        let _: () = msg_send![&*window, setCollectionBehavior: OVERLAY_COLLECTION_BEHAVIOR];
        let _: () = msg_send![&*window, setIgnoresMouseEvents: opts.click_through];
        let _: () = msg_send![&*window, setSharingType: sharing];
        let _: () = msg_send![&*window, setOpaque: false];
        let _: () = msg_send![&*window, setHasShadow: false];
        let _: () = msg_send![&*window, setBackgroundColor: &*clear];
        let _: () = msg_send![&*window, orderFrontRegardless];
    }
    Ok(())
}

const fn sharing_type(exclude_from_capture: bool) -> usize {
    if exclude_from_capture {
        NS_WINDOW_SHARING_NONE
    } else {
        NS_WINDOW_SHARING_READ_ONLY
    }
}

pub(crate) fn set_overlay_capture_excluded(
    handle: RawWindowHandle,
    excluded: bool,
) -> Result<(), OverlayError> {
    let window = appkit_window(handle)?;
    let sharing = sharing_type(excluded);
    // SAFETY: `window` is a live `NSWindow` on the main thread and
    // `-setSharingType:` takes an `NSUInteger`.
    unsafe {
        let _: () = msg_send![&*window, setSharingType: sharing];
    }
    Ok(())
}

pub(crate) fn set_overlay_click_through(
    handle: RawWindowHandle,
    click_through: bool,
) -> Result<(), OverlayError> {
    let window = appkit_window(handle)?;
    // SAFETY: `window` is a live `NSWindow` on the main thread and
    // `-setIgnoresMouseEvents:` takes a `BOOL`.
    unsafe {
        let _: () = msg_send![&*window, setIgnoresMouseEvents: click_through];
    }
    Ok(())
}

pub(crate) fn show_overlay_without_focus(handle: RawWindowHandle) -> Result<(), OverlayError> {
    let window = appkit_window(handle)?;
    // SAFETY: `window` is a live `NSWindow` on the main thread;
    // `-orderFrontRegardless` takes no arguments and neither activates the app
    // nor makes the window key.
    unsafe {
        let _: () = msg_send![&*window, orderFrontRegardless];
    }
    Ok(())
}

pub(crate) fn hide_overlay(handle: RawWindowHandle) -> Result<(), OverlayError> {
    let window = appkit_window(handle)?;
    // SAFETY: `window` is a live `NSWindow` on the main thread and
    // `-orderOut:` takes a nullable sender.
    unsafe {
        let _: () = msg_send![&*window, orderOut: Option::<&AnyObject>::None];
    }
    Ok(())
}

// --- Interact input (SPEC §3.3) ---

/// `kCGEventSourceStateCombinedSessionState`.
const EVENT_SOURCE_COMBINED_SESSION: i32 = 0;
/// `kCGEventFlagMaskShift`, `…Control`, `…Alternate` and `…Command`.
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_ALTERNATE: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

/// The modifier keys held now. Reads modifier flags only, never other keys,
/// so it needs neither Accessibility nor Input Monitoring permission (P1).
pub(crate) fn held_modifiers() -> Modifiers {
    // SAFETY: `CGEventSourceFlagsState` takes a source state ID by value and
    // returns a flags word; it has no preconditions.
    let flags = unsafe { CGEventSourceFlagsState(EVENT_SOURCE_COMBINED_SESSION) };
    let mut held = Modifiers::NONE;
    for (mask, modifier) in [
        (FLAG_SHIFT, Modifiers::SHIFT),
        (FLAG_CONTROL, Modifiers::CONTROL),
        (FLAG_ALTERNATE, Modifiers::ALT),
        (FLAG_COMMAND, Modifiers::SUPER),
    ] {
        if flags & mask != 0 {
            held = held | modifier;
        }
    }
    held
}

/// `NSTrackingMouseMoved | NSTrackingActiveAlways | NSTrackingInVisibleRect`:
/// pointer moves reach the overlay's view even while another app is active,
/// which `NSWindow` mouse-moved events (key window only) don't.
const POINTER_TRACKING_OPTIONS: usize = 0x02 | 0x80 | 0x200;

/// The overlay's interact state: the tracking area added while it takes
/// the pointer, and the app that was in front when interact mode began.
struct Interacting {
    view: Retained<AnyObject>,
    area: Retained<AnyObject>,
    previous: Option<Retained<AnyObject>>,
}

thread_local! {
    // AppKit objects, touched on the main thread only (one overlay).
    static INTERACTING: RefCell<Option<Interacting>> = const { RefCell::new(None) };
}

pub(crate) fn set_overlay_interactivity(
    handle: RawWindowHandle,
    mode: Interactivity,
) -> Result<(), OverlayError> {
    let RawWindowHandle::AppKit(raw) = handle else {
        return Err(OverlayError::UnsupportedHandle);
    };
    let window = appkit_window(handle)?;
    // SAFETY: `ns_view` is the live `NSView *` behind `handle` (checked by
    // `appkit_window`, which also checked we are on the main thread).
    let view: Retained<AnyObject> = unsafe { raw.ns_view.cast::<AnyObject>().as_ref() }.retain();
    INTERACTING.with_borrow_mut(|state| {
        if mode == Interactivity::ClickThrough {
            // SAFETY: `window` is a live `NSWindow` on the main thread.
            unsafe {
                let _: () = msg_send![&*window, setIgnoresMouseEvents: true];
            }
            if let Some(old) = state.take() {
                // SAFETY: `old.area` was added to `old.view` below; both are
                // live (retained) AppKit objects on the main thread.
                unsafe {
                    let _: () = msg_send![&*old.view, removeTrackingArea: &*old.area];
                }
                if let Some(previous) = old.previous {
                    return_focus(&previous);
                }
            }
            return Ok(());
        }
        if state.is_none() {
            // SAFETY: `+[NSWorkspace sharedWorkspace]` and
            // `-frontmostApplication` return a live object or nil;
            // `NSTrackingArea`'s designated initialiser takes an `NSRect`, an
            // `NSTrackingAreaOptions` (`NSUInteger`), an owner (the view,
            // whose `-mouseMoved:` winit implements) and nullable user info.
            let interacting = unsafe {
                let workspace: Retained<AnyObject> =
                    msg_send![class!(NSWorkspace), sharedWorkspace];
                let previous: Option<Retained<AnyObject>> =
                    msg_send![&*workspace, frontmostApplication];
                let alloc: objc2::rc::Allocated<AnyObject> =
                    msg_send![class!(NSTrackingArea), alloc];
                let area: Retained<AnyObject> = msg_send![
                    alloc,
                    initWithRect: NSRect::ZERO,
                    options: POINTER_TRACKING_OPTIONS,
                    owner: &*view,
                    userInfo: Option::<&AnyObject>::None
                ];
                let _: () = msg_send![&*view, addTrackingArea: &*area];
                let _: () = msg_send![&*window, setIgnoresMouseEvents: false];
                Interacting {
                    view,
                    area,
                    previous: previous.filter(|app| !is_current_app(app)),
                }
            };
            *state = Some(interacting);
        }
        if mode == Interactivity::Focused {
            // SAFETY: `NSApp` exists once winit's event loop runs (the only
            // caller); `-activateIgnoringOtherApps:` takes a `BOOL` and
            // `-makeKeyAndOrderFront:` a nullable sender, on the main thread.
            unsafe {
                let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
                let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
                let _: () = msg_send![&*window, makeKeyAndOrderFront: Option::<&AnyObject>::None];
            }
        }
        Ok(())
    })
}

/// Whether `app` (an `NSRunningApplication`) is this process.
fn is_current_app(app: &AnyObject) -> bool {
    // SAFETY: `-processIdentifier` returns a `pid_t` (`i32`).
    let pid: i32 = unsafe { msg_send![app, processIdentifier] };
    pid == i32::try_from(std::process::id()).unwrap_or(-1)
}

/// Gives focus back to `previous` if interact mode made this app active (a
/// toggle, or a click while holding the chord).
fn return_focus(previous: &AnyObject) {
    // SAFETY: `NSApp` exists while the event loop runs; `-isActive` returns
    // a `BOOL`; `-activateWithOptions:` takes `NSApplicationActivationOptions`
    // (`NSUInteger`) and returns a `BOOL`; all on the main thread.
    unsafe {
        let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
        let active: bool = msg_send![&*app, isActive];
        if active {
            let _: bool = msg_send![previous, activateWithOptions: 0_usize];
        }
    }
}

// --- Full-screen app detection (SPEC §6.2) ---

/// `NSWorkspace` notifications after which the front window set may have
/// changed.
const FULLSCREEN_NOTIFICATIONS: [&str; 5] = [
    "NSWorkspaceActiveSpaceDidChangeNotification",
    "NSWorkspaceDidActivateApplicationNotification",
    "NSWorkspaceDidDeactivateApplicationNotification",
    "NSWorkspaceDidHideApplicationNotification",
    "NSWorkspaceDidUnhideApplicationNotification",
];

/// `kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements`.
const WINDOW_LIST_OPTIONS: u32 = (1 << 0) | (1 << 4);
/// How far (points) a window may fall short of a display and still cover it.
const COVER_TOLERANCE: f64 = 1.0;
/// `kCGWindowBounds`, `kCGWindowLayer` and `kCGWindowOwnerPID`. Window names
/// are never read, so Screen Recording permission is never needed.
const KEY_WINDOW_BOUNDS: &str = "kCGWindowBounds";
const KEY_WINDOW_LAYER: &str = "kCGWindowLayer";
const KEY_WINDOW_OWNER_PID: &str = "kCGWindowOwnerPID";

/// A `CGRect` (`CGFloat` is `f64` on every 64-bit macOS target).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    /// Whether `self` reaches every edge of `other`, within the tolerance.
    fn covers(&self, other: &Rect) -> bool {
        other.width > 0.0
            && other.height > 0.0
            && self.x <= other.x + COVER_TOLERANCE
            && self.y <= other.y + COVER_TOLERANCE
            && self.x + self.width >= other.x + other.width - COVER_TOLERANCE
            && self.y + self.height >= other.y + other.height - COVER_TOLERANCE
    }
}

/// The three window fields the check needs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowInfo {
    owner_pid: i32,
    layer: i32,
    bounds: Rect,
}

/// Whether any window owned by another process covers a whole display.
///
/// Normal windows and full-screen app windows sit at layer 0. Negative layers
/// are desktop elements; the menu bar and Dock are positive but never reach
/// the full bounds of a display. The overlay's own windows (same PID) are
/// ignored.
fn has_fullscreen_window(windows: &[WindowInfo], displays: &[Rect], own_pid: i32) -> bool {
    windows
        .iter()
        .filter(|w| w.owner_pid != own_pid && w.layer >= 0)
        .any(|w| displays.iter().any(|d| w.bounds.covers(d)))
}

fn active_display_bounds() -> Vec<Rect> {
    const MAX_DISPLAYS: usize = 16;
    let mut ids = [0_u32; MAX_DISPLAYS];
    let mut count = 0_u32;
    // SAFETY: `ids` has room for `MAX_DISPLAYS` entries (16 fits `u32`) and
    // `count` is a valid out-pointer.
    let status = unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &raw mut count) };
    let count = usize::try_from(count).unwrap_or(0).min(MAX_DISPLAYS);
    if status != 0 || count == 0 {
        // SAFETY: both calls take no pointers and are always valid.
        return vec![unsafe { CGDisplayBounds(CGMainDisplayID()) }];
    }
    ids[..count]
        .iter()
        // SAFETY: each id came from `CGGetActiveDisplayList` above.
        .map(|&id| unsafe { CGDisplayBounds(id) })
        .collect()
}

/// Reads the on-screen window list (bounds, layer and owner PID only).
fn query_windows() -> Option<Vec<WindowInfo>> {
    // SAFETY: Takes two scalars and returns an owned `CFArrayRef` of
    // `CFDictionary` (or NULL), per the CoreFoundation Copy rule.
    let list_nn =
        NonNull::new(unsafe { CGWindowListCopyWindowInfo(WINDOW_LIST_OPTIONS, 0) }.cast_mut())?;
    // SAFETY: `CGWindowListCopyWindowInfo` follows the Copy rule.
    let list = unsafe { CFRetained::from_raw(list_nn) };

    let bounds_key = CFString::from_static_str(KEY_WINDOW_BOUNDS);
    let layer_key = CFString::from_static_str(KEY_WINDOW_LAYER);
    let pid_key = CFString::from_static_str(KEY_WINDOW_OWNER_PID);

    let count = list.count();
    let mut windows = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for i in 0..count {
        // SAFETY: `0 <= i < count` is within bounds of `list`.
        let item = unsafe { list.value_at_index(i) };
        let Some(item) = NonNull::new(item.cast_mut().cast::<CFDictionary>()) else {
            continue;
        };
        // SAFETY: Elements of the window list are `CFDictionary`s alive for
        // as long as `list`.
        let dict: &CFDictionary = unsafe { item.as_ref() };
        let (Some(bounds_dict), Some(layer), Some(pid)) = (
            dict_value::<CFDictionary>(dict, &bounds_key),
            dict_value::<CFNumber>(dict, &layer_key).and_then(CFNumber::as_i32),
            dict_value::<CFNumber>(dict, &pid_key).and_then(CFNumber::as_i32),
        ) else {
            continue;
        };
        let mut bounds = Rect::default();
        // SAFETY: `bounds_dict` is a valid `CFDictionary` and `bounds` a valid
        // out-pointer.
        if !unsafe { CGRectMakeWithDictionaryRepresentation(bounds_dict, &raw mut bounds) } {
            continue;
        }
        windows.push(WindowInfo {
            owner_pid: pid,
            layer,
            bounds,
        });
    }
    Some(windows)
}

fn is_fullscreen_app_active_now() -> bool {
    let Some(windows) = query_windows() else {
        return false;
    };
    let own_pid = i32::try_from(std::process::id()).unwrap_or(-1);
    has_fullscreen_window(&windows, &active_display_bounds(), own_pid)
}

struct FullScreenShared {
    fullscreen: AtomicBool,
    on_change: Box<dyn Fn() + Send + Sync>,
}

type Detect = Arc<dyn Fn() -> bool + Send + Sync>;

pub(crate) struct FullScreenMonitor {
    shared: Arc<FullScreenShared>,
    detect: Detect,
    observers: Vec<ObserverEntry>,
}

impl FullScreenMonitor {
    pub(crate) fn new(on_change: impl Fn() + Send + Sync + 'static) -> Self {
        let detect: Detect = Arc::new(is_fullscreen_app_active_now);
        let shared = Arc::new(FullScreenShared {
            fullscreen: AtomicBool::new(detect()),
            on_change: Box::new(on_change),
        });
        let observers = workspace_notification_center().map_or_else(Vec::new, |center| {
            register_fullscreen_observers(&shared, &center, &detect)
        });
        Self {
            shared,
            detect,
            observers,
        }
    }

    pub(crate) fn is_fullscreen_app_active(&self) -> bool {
        self.shared.fullscreen.load(Ordering::Acquire)
    }

    pub(crate) fn refresh(&self) -> bool {
        let now = (self.detect)();
        self.shared.fullscreen.store(now, Ordering::Release);
        now
    }
}

impl Drop for FullScreenMonitor {
    fn drop(&mut self) {
        for (center, observer) in self.observers.drain(..) {
            // SAFETY: `observer` is the token returned by
            // `addObserverForName:object:queue:usingBlock:` on `center`, and
            // is removed at most once here.
            unsafe { center.removeObserver(observer.as_ref()) };
        }
    }
}

fn register_fullscreen_observers(
    shared: &Arc<FullScreenShared>,
    center: &NSNotificationCenter,
    detect: &Detect,
) -> Vec<ObserverEntry> {
    FULLSCREEN_NOTIFICATIONS
        .iter()
        .map(|name| {
            let s = Arc::clone(shared);
            let detect = Arc::clone(detect);
            let is_space_change = *name == FULLSCREEN_NOTIFICATIONS[0];
            (
                center.retain(),
                observe_center(center, &NSString::from_str(name), move || {
                    let now = detect();
                    let prev = s.fullscreen.swap(now, Ordering::AcqRel);
                    // Always wake on Space switches so the host can schedule a
                    // one-shot re-check after the Space transition animation settles.
                    if prev != now || is_space_change {
                        (s.on_change)();
                    }
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use objc2_core_foundation::{CFRunLoopRunResult, Type, kCFRunLoopDefaultMode};

    use super::*;

    fn make_desc(
        present: Option<bool>,
        current: Option<i64>,
        max: Option<i64>,
    ) -> CFRetained<CFDictionary> {
        let mut keys: Vec<CFRetained<CFString>> = Vec::new();
        let mut vals: Vec<CFRetained<CFType>> = Vec::new();
        if let Some(p) = present {
            keys.push(CFString::from_static_str(KEY_IS_PRESENT));
            vals.push(Type::retain(CFBoolean::new(p)).into());
        }
        if let Some(c) = current {
            keys.push(CFString::from_static_str(KEY_CURRENT_CAPACITY));
            vals.push(CFNumber::new_i64(c).into());
        }
        if let Some(m) = max {
            keys.push(CFString::from_static_str(KEY_MAX_CAPACITY));
            vals.push(CFNumber::new_i64(m).into());
        }
        let key_refs: Vec<&CFString> = keys.iter().map(CFRetained::as_ref).collect();
        let val_refs: Vec<&CFType> = vals.iter().map(CFRetained::as_ref).collect();
        let typed = CFDictionary::<CFString, CFType>::from_slices(&key_refs, &val_refs);
        Type::retain(typed.as_opaque())
    }

    #[test]
    fn live_iokit_and_process_info_queries_return_valid_values() {
        let source = read_power_source();
        if let PowerSource::Battery { percent: Some(p) } = source {
            assert!(p <= 100, "battery percent out of range: {p}");
        }
        let _ = is_low_power_mode();
    }

    #[test]
    fn parses_ac_battery_ups_and_edge_case_dictionaries() {
        let ac = CFString::from_static_str("AC Power");
        let bat = CFString::from_static_str(BATTERY_POWER);
        let ups = CFString::from_static_str(UPS_POWER);

        let d80 = make_desc(Some(true), Some(80), Some(100));
        assert_eq!(parse_power_source(Some(&ac), &[&d80]), PowerSource::Ac);
        assert_eq!(parse_power_source(None, &[&d80]), PowerSource::Ac);
        assert_eq!(
            parse_power_source(Some(&bat), &[&d80]),
            PowerSource::Battery { percent: Some(80) }
        );

        let d_half = make_desc(None, Some(45), Some(90));
        assert_eq!(
            parse_power_source(Some(&ups), &[&d_half]),
            PowerSource::Battery { percent: Some(50) }
        );

        let d_absent = make_desc(Some(false), Some(10), Some(100));
        let d_zero_max = make_desc(Some(true), Some(10), Some(0));
        assert_eq!(
            parse_power_source(Some(&bat), &[&d_absent, &d_zero_max]),
            PowerSource::Battery { percent: None }
        );

        // Multiple present batteries aggregate total current / total max.
        let d1 = make_desc(Some(true), Some(20), Some(100));
        let d2 = make_desc(Some(true), Some(40), Some(100));
        assert_eq!(
            parse_power_source(Some(&bat), &[&d1, &d2]),
            PowerSource::Battery { percent: Some(30) }
        );
    }

    #[test]
    fn local_notification_observers_update_signals_and_unregister_on_drop() {
        let process_center = NSNotificationCenter::new();
        let workspace_center = NSNotificationCenter::new();
        // Use a process-local center for lock tests so we never post fake lock
        // notifications to the system-wide NSDistributedNotificationCenter.
        let distributed_center = NSNotificationCenter::new();

        let changes = Arc::new(AtomicUsize::new(0));
        let changes_cb = Arc::clone(&changes);
        let shared = Arc::new(Shared {
            signals: Mutex::new(PowerSignals::default()),
            on_change: Box::new(move || {
                changes_cb.fetch_add(1, Ordering::Relaxed);
            }),
        });

        let observers = register_observers(
            &shared,
            &process_center,
            Some(&workspace_center),
            &distributed_center,
        );
        let monitor = PowerMonitor {
            shared: Arc::clone(&shared),
            observers,
            iops: None,
            _not_send: PhantomData,
        };

        let post = |center: &NSNotificationCenter, name: &NSString| {
            // SAFETY: posting a named notification with `object: None` to a
            // process-local `NSNotificationCenter`.
            unsafe { center.postNotificationName_object(name, None) };
        };

        let sleep_name = NSString::from_str(DISPLAY_SLEEP_NOTIFICATION);
        let wake_name = NSString::from_str(DISPLAY_WAKE_NOTIFICATION);
        let lock_name = NSString::from_str(SCREEN_LOCKED_NOTIFICATION);
        let unlock_name = NSString::from_str(SCREEN_UNLOCKED_NOTIFICATION);
        // SAFETY: static Foundation notification name.
        let low_power_name = unsafe { NSProcessInfoPowerStateDidChangeNotification };

        post(&workspace_center, &sleep_name);
        assert!(monitor.signals().display_asleep);
        assert_eq!(changes.load(Ordering::Relaxed), 1);

        // Posting the same state twice does not fire `on_change` again.
        post(&workspace_center, &sleep_name);
        assert_eq!(changes.load(Ordering::Relaxed), 1);

        post(&workspace_center, &wake_name);
        assert!(!monitor.signals().display_asleep);
        assert_eq!(changes.load(Ordering::Relaxed), 2);

        post(&distributed_center, &lock_name);
        assert!(monitor.signals().session_locked);
        assert_eq!(changes.load(Ordering::Relaxed), 3);

        post(&distributed_center, &unlock_name);
        assert!(!monitor.signals().session_locked);
        assert_eq!(changes.load(Ordering::Relaxed), 4);

        post(&process_center, low_power_name);
        assert_eq!(monitor.signals().low_power_mode, is_low_power_mode());

        let count_before_drop = changes.load(Ordering::Relaxed);
        drop(monitor);
        post(&workspace_center, &sleep_name);
        post(&distributed_center, &lock_name);
        assert_eq!(changes.load(Ordering::Relaxed), count_before_drop);
    }

    #[test]
    fn power_monitor_idle_waits_without_spurious_wakes_and_tears_down_cleanly() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = Arc::clone(&wakes);
        let monitor = PowerMonitor::new(move || {
            wakes_cb.fetch_add(1, Ordering::Relaxed);
        });
        let _ = monitor.signals();
        let has_iops = monitor.iops.is_some();
        // SAFETY: `kCFRunLoopDefaultMode` is a valid CoreFoundation mode constant.
        let default_mode = unsafe { kCFRunLoopDefaultMode };
        if has_iops {
            let start = Instant::now();
            let result = CFRunLoop::run_in_mode(default_mode, 0.05, false);
            assert_eq!(result, CFRunLoopRunResult::TimedOut);
            assert!(
                start.elapsed() >= Duration::from_millis(35),
                "run loop returned early: {:?}",
                start.elapsed()
            );
        }
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
        drop(monitor);
        if has_iops {
            let start = Instant::now();
            let result = CFRunLoop::run_in_mode(default_mode, 0.05, false);
            assert_eq!(result, CFRunLoopRunResult::Finished);
            assert!(start.elapsed() < Duration::from_millis(25));
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn window(owner_pid: i32, layer: i32, bounds: Rect) -> WindowInfo {
        WindowInfo {
            owner_pid,
            layer,
            bounds,
        }
    }

    const DISPLAY: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1512.0,
        height: 982.0,
    };

    #[test]
    fn full_display_window_of_another_app_is_fullscreen() {
        let windows = [window(100, 0, DISPLAY)];
        assert!(has_fullscreen_window(&windows, &[DISPLAY], 1));
    }

    #[test]
    fn bounds_within_tolerance_or_larger_still_cover() {
        let near = rect(0.5, 0.5, 1511.0, 981.0);
        assert!(has_fullscreen_window(
            &[window(100, 0, near)],
            &[DISPLAY],
            1
        ));
        let larger = rect(-10.0, -10.0, 1600.0, 1100.0);
        assert!(has_fullscreen_window(
            &[window(100, 0, larger)],
            &[DISPLAY],
            1
        ));
        let short = rect(0.0, 0.0, 1512.0, 950.0);
        assert!(!has_fullscreen_window(
            &[window(100, 0, short)],
            &[DISPLAY],
            1
        ));
    }

    #[test]
    fn maximised_window_below_the_menu_bar_is_not_fullscreen() {
        let below_menu_bar = rect(0.0, 38.0, 1512.0, 944.0);
        assert!(!has_fullscreen_window(
            &[window(100, 0, below_menu_bar)],
            &[DISPLAY],
            1
        ));
    }

    #[test]
    fn own_windows_are_ignored() {
        assert!(!has_fullscreen_window(
            &[window(1, 0, DISPLAY)],
            &[DISPLAY],
            1
        ));
    }

    #[test]
    fn desktop_element_layers_are_ignored() {
        let desktop = window(100, -2_147_483_624, DISPLAY);
        assert!(!has_fullscreen_window(&[desktop], &[DISPLAY], 1));
    }

    #[test]
    fn menu_bar_and_dock_strips_are_not_fullscreen() {
        let menu_bar = window(200, 24, rect(0.0, 0.0, 1512.0, 38.0));
        let dock = window(201, 20, rect(300.0, 930.0, 900.0, 52.0));
        assert!(!has_fullscreen_window(&[menu_bar, dock], &[DISPLAY], 1));
    }

    #[test]
    fn any_display_covered_counts_and_no_display_never_does() {
        let second = rect(1512.0, 0.0, 1920.0, 1080.0);
        let on_second = window(100, 0, second);
        assert!(has_fullscreen_window(&[on_second], &[DISPLAY, second], 1));
        assert!(!has_fullscreen_window(&[on_second], &[DISPLAY], 1));
        assert!(!has_fullscreen_window(&[on_second], &[], 1));
        assert!(!has_fullscreen_window(&[], &[DISPLAY], 1));
        // A zero-sized display is never covered.
        assert!(!has_fullscreen_window(&[on_second], &[Rect::default()], 1));
    }

    #[test]
    fn live_window_query_reads_without_screen_recording_and_never_panics() {
        let displays = active_display_bounds();
        assert!(displays.iter().all(|d| d.width > 0.0 && d.height > 0.0));
        let _ = query_windows();
        let _ = is_fullscreen_app_active_now();
    }

    #[test]
    fn appkit_window_rejects_wrong_handle_kinds_and_other_threads() {
        use desktank_render::wgpu::rwh::{AppKitWindowHandle, Win32WindowHandle};

        let win32 = RawWindowHandle::Win32(Win32WindowHandle::new(
            core::num::NonZeroIsize::new(1).unwrap(),
        ));
        assert_eq!(
            appkit_window(win32).unwrap_err(),
            OverlayError::UnsupportedHandle
        );

        let appkit = RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::dangling()));
        if MainThreadMarker::new().is_none() {
            // Rejected before the dangling view is dereferenced.
            assert_eq!(
                appkit_window(appkit).unwrap_err(),
                OverlayError::NotMainThread
            );
            assert_eq!(
                hide_overlay(appkit).unwrap_err(),
                OverlayError::NotMainThread
            );
        }
    }

    #[test]
    fn sharing_type_maps_exclusion_to_none_and_inclusion_to_read_only() {
        assert_eq!(sharing_type(true), 0);
        assert_eq!(sharing_type(false), 1);
    }

    #[test]
    fn overlay_collection_behavior_never_joins_fullscreen_spaces() {
        assert_eq!(OVERLAY_COLLECTION_BEHAVIOR, 0b101_0001);
        assert_eq!(OVERLAY_COLLECTION_BEHAVIOR & (1 << 8), 0);
    }

    #[test]
    fn fullscreen_observers_refresh_on_each_workspace_notification_and_unregister_on_drop() {
        let center = NSNotificationCenter::new();
        let detected = Arc::new(AtomicBool::new(false));
        let detected_cb = Arc::clone(&detected);
        let detect: Detect = Arc::new(move || detected_cb.load(Ordering::Relaxed));

        let changes = Arc::new(AtomicUsize::new(0));
        let changes_cb = Arc::clone(&changes);
        let shared = Arc::new(FullScreenShared {
            fullscreen: AtomicBool::new(false),
            on_change: Box::new(move || {
                changes_cb.fetch_add(1, Ordering::Relaxed);
            }),
        });
        let observers = register_fullscreen_observers(&shared, &center, &detect);
        assert_eq!(observers.len(), FULLSCREEN_NOTIFICATIONS.len());
        let monitor = FullScreenMonitor {
            shared,
            detect: Arc::clone(&detect),
            observers,
        };

        let mut expected_wakes = 0;
        for (i, name) in FULLSCREEN_NOTIFICATIONS.iter().enumerate() {
            let name = NSString::from_str(name);
            let now = i % 2 == 0;
            detected.store(now, Ordering::Relaxed);

            // SAFETY: posting a named notification with `object: None` to a
            // process-local `NSNotificationCenter`.
            unsafe { center.postNotificationName_object(&name, None) };
            expected_wakes += 1;
            // Posting an unchanged state only re-wakes on Space switches (for
            // the host's post-transition one-shot re-check).
            unsafe { center.postNotificationName_object(&name, None) };
            if i == 0 {
                expected_wakes += 1;
            }

            assert_eq!(monitor.is_fullscreen_app_active(), now, "{name}");
            assert_eq!(changes.load(Ordering::Relaxed), expected_wakes, "{name}");
        }

        // `refresh()` updates the cached state directly without firing a wake.
        let flipped = !monitor.is_fullscreen_app_active();
        detected.store(flipped, Ordering::Relaxed);
        assert_eq!(monitor.refresh(), flipped);
        assert_eq!(monitor.is_fullscreen_app_active(), flipped);
        assert_eq!(changes.load(Ordering::Relaxed), expected_wakes);

        let before_drop = changes.load(Ordering::Relaxed);
        let last = monitor.is_fullscreen_app_active();
        detected.store(!last, Ordering::Relaxed);
        drop(monitor);
        let name = NSString::from_str(FULLSCREEN_NOTIFICATIONS[0]);
        // SAFETY: as above.
        unsafe { center.postNotificationName_object(&name, None) };
        assert_eq!(changes.load(Ordering::Relaxed), before_drop);
    }

    #[test]
    fn fullscreen_monitor_registers_without_wakes_and_drops_cleanly() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let wakes_cb = Arc::clone(&wakes);
        let monitor = FullScreenMonitor::new(move || {
            wakes_cb.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(monitor.observers.len(), FULLSCREEN_NOTIFICATIONS.len());
        let _ = monitor.is_fullscreen_app_active();
        // No timers or run-loop sources: nothing fires without a notification.
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
        drop(monitor);
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
    }
}
