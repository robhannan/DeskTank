//! Modifier keys held right now, for the interact chord (SPEC §2.5, §3.3).
//!
//! Hosts poll [`held_modifiers`] at 10 Hz while a tank is visible and
//! interact-by-chord is on. It reads modifier flags only, never other keys.
//! macOS reads `CGEventSourceFlagsState`; other OSes return `None` until
//! their overlay lands (Windows `GetAsyncKeyState`, X11 `XQueryKeymap`;
//! Wayland has no global key state, so it uses the tray or CLI).

use core::ops::BitOr;

/// A set of modifier keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No modifiers.
    pub const NONE: Self = Self(0);
    /// Shift.
    pub const SHIFT: Self = Self(1);
    /// Control (⌃ on macOS).
    pub const CONTROL: Self = Self(1 << 1);
    /// Alt (⌥ Option on macOS).
    pub const ALT: Self = Self(1 << 2);
    /// Command on macOS, the Windows or Super key elsewhere.
    pub const SUPER: Self = Self(1 << 3);

    /// Whether every modifier in `other` is in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// The modifier keys held now, or `None` where the OS can't say (SPEC §3.3).
#[must_use]
pub fn held_modifiers() -> Option<Modifiers> {
    #[cfg(target_os = "macos")]
    {
        Some(crate::macos::held_modifiers())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_needs_every_modifier() {
        let chord = Modifiers::CONTROL | Modifiers::ALT;
        assert!((chord | Modifiers::SHIFT).contains(chord));
        assert!(!Modifiers::CONTROL.contains(chord));
        assert!(Modifiers::NONE.is_empty());
        assert!(Modifiers::SHIFT.contains(Modifiers::NONE));
    }
}
