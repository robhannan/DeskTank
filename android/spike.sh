#!/usr/bin/env bash
# TEMPORARY (A0 spike): install the APK on a booted emulator, start the Dream
# in several ways and collect evidence into $2. Removed before hand-off.
set -u
apk=$1
out=$2
mkdir -p "$out"
pkg=io.github.robhannan.desktank
comp=$pkg/.DeskTankDream

log() { echo "== $*" | tee -a "$out/steps.txt"; }
run() {
  log "\$ $*"
  "$@" 2>&1 | tee -a "$out/steps.txt"
}
shot() { adb exec-out screencap -p > "$out/$1.png"; log "screenshot $1.png"; }
dreaming() { adb shell dumpsys dreams | grep -iE 'dreaming|mCurrentDream|Dream' | head -20 | tee -a "$out/steps.txt"; }
evidence() {
  shot "$1"
  dreaming
  adb shell dumpsys meminfo "$pkg" > "$out/meminfo-$1.txt" 2>&1
  grep -E 'TOTAL|Graphics|GL mtrack|EGL mtrack|Native Heap' "$out/meminfo-$1.txt" | tee -a "$out/steps.txt"
  adb logcat -d -s DeskTank:* AndroidRuntime:E DEBUG:* libc:F vulkan:* > "$out/logcat-$1.txt" 2>&1
  tee -a "$out/steps.txt" < "$out/logcat-$1.txt"
}
stop() {
  run adb shell cmd dreams stop-dreaming
  run adb shell input keyevent KEYCODE_HOME
  sleep 2
}

run adb shell getprop ro.build.fingerprint
run adb shell getprop ro.build.version.sdk
run adb shell getprop ro.product.cpu.abilist
run adb shell getprop ro.hardware.vulkan
run adb shell getprop ro.hardware.egl
run adb shell pm list features
run adb install -r "$apk"
run adb shell settings get secure screensaver_components
run adb shell settings get secure screensaver_enabled
run adb shell settings put secure screensaver_enabled 1
run adb shell settings put secure screensaver_components "$comp"
run adb shell settings get secure screensaver_components
run adb shell cmd dreams help
adb shell dumpsys dreams > "$out/dumpsys-dreams-before.txt" 2>&1

log "method 1 (shell user): service call dreams 1 (IDreamManager.dream)"
adb logcat -c
run adb shell service call dreams 1
sleep 8
evidence 1-service-call
stop

log "method 2 (shell user): idle timeout with screensaver_activate_on_sleep"
run adb shell settings put secure screensaver_activate_on_sleep 1
run adb shell settings put system screen_off_timeout 15000
run adb shell input keyevent KEYCODE_WAKEUP
adb logcat -c
sleep 30
evidence 2-idle-timeout
stop
run adb shell settings put system screen_off_timeout 1800000

log "method 3 (shell user): cmd dreams start-dreaming"
adb logcat -c
run adb shell cmd dreams start-dreaming
sleep 8
evidence 3-cmd-dreams-shell
stop

log "method 4: adb root, then cmd dreams start-dreaming"
run adb root
sleep 5
run adb wait-for-device
run adb shell id
adb logcat -c
run adb shell cmd dreams start-dreaming
sleep 8
evidence 4-cmd-dreams-root
stop

log "method 5: GLES forced (debug.desktank.backend=gl), cmd dreams (root)"
run adb shell am force-stop "$pkg"
run adb shell setprop debug.desktank.backend gl
adb logcat -c
run adb shell cmd dreams start-dreaming
sleep 8
evidence 5-gles
stop

log "method 6: Vulkan forced (debug.desktank.backend=vulkan), cmd dreams (root)"
run adb shell am force-stop "$pkg"
run adb shell setprop debug.desktank.backend vulkan
adb logcat -c
run adb shell cmd dreams start-dreaming
sleep 8
evidence 6-vulkan
stop
run adb shell setprop debug.desktank.backend ""

log "after stopping: process state"
run adb shell pidof "$pkg"
run adb shell top -b -n 1 -p "$(adb shell pidof "$pkg" | tr -d '\r')"
adb logcat -d > "$out/logcat-full.txt" 2>&1
exit 0
