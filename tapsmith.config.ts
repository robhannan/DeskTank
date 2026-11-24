// Tapsmith runs the Android tests (tests/android/, SPEC §7): it installs the
// debug APK on the attached device or emulator and drives it. Build the APK
// first (`cd android && ./gradlew assembleDebug`), then `pnpm test:android`.
// With several devices attached, set ANDROID_SERIAL to pick one (the tests'
// own adb calls read it too).
import { defineConfig } from "tapsmith";

export default defineConfig({
  apk: "./android/app/build/outputs/apk/debug/app-debug.apk",
  // The tests start their own hosts. Setting package would launch the
  // Activity before either test, including the one that starts the Dream.
  appReset: "none",
  // Only these: Tapsmith's default would also pick up tests/web/*.spec.ts.
  testMatch: ["tests/android/**/*.tapsmith.ts"],
  ...(process.env.ANDROID_SERIAL ? { device: process.env.ANDROID_SERIAL } : {}),
  // CI emulators run on software rendering and are slow.
  timeout: 60_000,
  screenshot: "always",
  // DeskTank makes no network requests: no proxy on the device.
  trace: { mode: "retain-on-failure", network: false },
  telemetry: false,
  reporter: process.env.CI ? ["list", ["html", { open: "never" }]] : "list",
});
