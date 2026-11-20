//! macOS power and session event sources (SPEC §6.2, O3).
//!
//! Uses event-driven OS notifications with zero polling timers:
//! - `IOKit` `IOPSNotificationCreateRunLoopSource` for AC/battery/percentage;
//! - `NSProcessInfoPowerStateDidChangeNotification` for Low Power Mode;
//! - `NSWorkspace` notifications for display sleep and wake;
//! - `NSDistributedNotificationCenter` for screen lock and unlock.

use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr::NonNull;
use std::sync::{Arc, Mutex, PoisonError};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{Message, class, msg_send};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFRunLoop, CFRunLoopSource, CFString,
    CFType, kCFRunLoopCommonModes,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSObjectProtocol,
    NSProcessInfo, NSProcessInfoPowerStateDidChangeNotification, NSString,
};

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
}
