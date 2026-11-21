//! The palette's category tree (SPEC §2.5): top categories, each with one or
//! more sub-categories, in one fixed order with one set of labels for every
//! host.
//!
//! Assets name the sub-category they are offered in with a path:
//! `palette = "animals/fish"` in a species, plant or material TOML. A top
//! category with a single sub-category is named on its own
//! (`palette = "water"`). Assets without a `palette` key aren't offered.
//! The tools and devices sub-categories are filled from code
//! ([`crate::tools::Tool`], [`crate::Placeable::ALL`]); the rest from the
//! assets naming them ([`crate::tools::Tools::palette_tree`]).

use thiserror::Error;

/// What a sub-category holds, so a path in an asset is checked against
/// the kind of asset naming it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Holds {
    /// Scoop, net, inspect and glue (filled from code).
    Tools,
    /// Placeable devices (filled from code).
    Devices,
    /// Pourable materials.
    Materials,
    /// Plant species, one item each.
    Plants,
    /// Creature species (stocked or live food), one item each.
    Creatures,
}

impl Holds {
    /// What an asset of this kind is called, for errors.
    fn noun(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Devices => "devices",
            Self::Materials => "materials",
            Self::Plants => "plants",
            Self::Creatures => "creatures",
        }
    }
}

/// One sub-category's definition.
struct SubDef {
    /// Its name in a path (`""` for a top category's only sub-category).
    name: &'static str,
    label: &'static str,
    holds: Holds,
}

/// One top category's definition.
struct TopDef {
    name: &'static str,
    label: &'static str,
    subs: &'static [SubDef],
}

const fn sub(name: &'static str, label: &'static str, holds: Holds) -> SubDef {
    SubDef { name, label, holds }
}

/// The tree, in palette order: the one place its names, labels and order
/// are defined. Keep names stable: assets name them.
const TREE: &[TopDef] = &[
    TopDef {
        name: "tools",
        label: "Tools",
        subs: &[sub("", "Tools", Holds::Tools)],
    },
    TopDef {
        name: "build",
        label: "Build",
        subs: &[
            sub("substrate", "Substrate", Holds::Materials),
            sub("hardscape", "Hardscape", Holds::Materials),
            sub("devices", "Devices", Holds::Devices),
        ],
    },
    TopDef {
        name: "water",
        label: "Water",
        subs: &[sub("", "Water", Holds::Materials)],
    },
    TopDef {
        name: "food",
        label: "Food",
        subs: &[
            sub("fish", "Fish food", Holds::Materials),
            sub("live", "Live", Holds::Creatures),
        ],
    },
    TopDef {
        name: "care",
        label: "Care",
        subs: &[sub("", "Care", Holds::Materials)],
    },
    TopDef {
        name: "plants",
        label: "Plants",
        subs: &[
            sub("stem", "Stem", Holds::Plants),
            sub("rosette", "Rosette", Holds::Plants),
            sub("carpet", "Carpet", Holds::Plants),
            sub("floating", "Floating", Holds::Plants),
            sub("moss", "Moss & epiphytes", Holds::Plants),
            sub("algae", "Algae", Holds::Materials),
        ],
    },
    TopDef {
        name: "animals",
        label: "Animals",
        subs: &[
            sub("fish", "Fish", Holds::Creatures),
            sub("shrimp", "Shrimp & crabs", Holds::Creatures),
            sub("snails", "Snails", Holds::Creatures),
            sub("amphibians", "Amphibians", Holds::Creatures),
        ],
    },
];

/// A sub-category of the palette: a top category and one of its
/// sub-categories, ordered as the palette shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PalettePath {
    top: u8,
    sub: u8,
}

/// A `palette = "…"` path that names no sub-category, or one that doesn't
/// hold that kind of asset.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PalettePathError {
    /// No such category.
    #[error("unknown palette category {path:?}; expected one of: {expected}")]
    Unknown {
        /// The path as written.
        path: String,
        /// Every path that holds this kind of asset, comma-separated.
        expected: String,
    },
    /// The category holds another kind of asset.
    #[error("palette category {path:?} holds {holds}, not {wanted}")]
    WrongKind {
        /// The path as written.
        path: String,
        /// What it holds.
        holds: &'static str,
        /// What named it.
        wanted: &'static str,
    },
}

impl PalettePath {
    /// `build/substrate`: the loose substrate burrowers dig through.
    pub const SUBSTRATE: Self = Self { top: 1, sub: 0 };

    /// Every sub-category, in palette order.
    pub fn all() -> impl Iterator<Item = Self> {
        TREE.iter().enumerate().flat_map(|(t, top)| {
            (0..top.subs.len()).map(move |s| Self {
                top: u8::try_from(t).unwrap_or(u8::MAX),
                sub: u8::try_from(s).unwrap_or(u8::MAX),
            })
        })
    }

    /// Parses `path` (`"top/sub"`, or `"top"` for a top category with one
    /// sub-category) for an asset of kind `holds`.
    ///
    /// # Errors
    /// [`PalettePathError::Unknown`] if it names no sub-category,
    /// [`PalettePathError::WrongKind`] if that sub-category holds another
    /// kind of asset.
    pub fn parse(path: &str, holds: Holds) -> Result<Self, PalettePathError> {
        let Some(found) = Self::all().find(|p| p.name() == path) else {
            let expected: Vec<_> = Self::all()
                .filter(|p| p.holds() == holds)
                .map(Self::name)
                .collect();
            return Err(PalettePathError::Unknown {
                path: path.to_owned(),
                expected: expected.join(", "),
            });
        };
        if found.holds() != holds {
            return Err(PalettePathError::WrongKind {
                path: path.to_owned(),
                holds: found.holds().noun(),
                wanted: holds.noun(),
            });
        }
        Ok(found)
    }

    fn top_def(self) -> &'static TopDef {
        &TREE[usize::from(self.top)]
    }

    fn sub_def(self) -> &'static SubDef {
        &self.top_def().subs[usize::from(self.sub)]
    }

    /// Its path as assets write it: `"animals/fish"`, or `"water"` for a
    /// top category's only sub-category.
    #[must_use]
    pub fn name(self) -> String {
        let (top, sub) = (self.top_def().name, self.sub_def().name);
        if sub.is_empty() {
            top.to_owned()
        } else {
            format!("{top}/{sub}")
        }
    }

    /// Index of its top category in palette order.
    #[must_use]
    pub fn top_index(self) -> usize {
        usize::from(self.top)
    }

    /// Index of the sub-category within its top category.
    #[must_use]
    pub fn sub_index(self) -> usize {
        usize::from(self.sub)
    }

    /// Its top category's name (`"animals"`).
    #[must_use]
    pub fn top_name(self) -> &'static str {
        self.top_def().name
    }

    /// Its top category's label (`"Animals"`).
    #[must_use]
    pub fn top_label(self) -> &'static str {
        self.top_def().label
    }

    /// Its own label (`"Fish"`; the top category's for an only
    /// sub-category).
    #[must_use]
    pub fn label(self) -> &'static str {
        self.sub_def().label
    }

    /// What it holds.
    #[must_use]
    pub fn holds(self) -> Holds {
        self.sub_def().holds
    }

    /// Whether its top category has more than one sub-category (hosts show
    /// a sub-tab row only then).
    #[must_use]
    pub fn has_siblings(self) -> bool {
        self.top_def().subs.len() > 1
    }
}

impl core::fmt::Display for PalettePath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_parse_and_name_themselves() {
        let fish = PalettePath::parse("animals/fish", Holds::Creatures).unwrap();
        assert_eq!(fish.name(), "animals/fish");
        assert_eq!((fish.top_label(), fish.label()), ("Animals", "Fish"));
        assert!(fish.has_siblings());
        assert_eq!(PalettePath::SUBSTRATE.name(), "build/substrate");
        let water = PalettePath::parse("water", Holds::Materials).unwrap();
        assert_eq!((water.top_label(), water.label()), ("Water", "Water"));
        assert!(!water.has_siblings());
        // Every path round-trips, names are unique and in tree order.
        let all: Vec<_> = PalettePath::all().collect();
        for p in &all {
            assert_eq!(PalettePath::parse(&p.name(), p.holds()), Ok(*p));
        }
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        let mut tops: Vec<_> = all.iter().map(|p| p.top_label()).collect();
        tops.dedup();
        assert_eq!(
            tops,
            [
                "Tools", "Build", "Water", "Food", "Care", "Plants", "Animals"
            ]
        );
    }

    #[test]
    fn bad_paths_say_what_would_do() {
        let err = PalettePath::parse("animals/fsh", Holds::Creatures).unwrap_err();
        assert!(matches!(err, PalettePathError::Unknown { .. }));
        let text = err.to_string();
        assert!(text.contains("\"animals/fsh\""), "{text}");
        assert!(text.contains("animals/fish, animals/shrimp"), "{text}");
        assert!(!text.contains("water"), "only creature categories: {text}");
        // The top alone isn't enough where it has several sub-categories.
        assert!(PalettePath::parse("animals", Holds::Creatures).is_err());
        assert_eq!(
            PalettePath::parse("animals/fish", Holds::Materials),
            Err(PalettePathError::WrongKind {
                path: "animals/fish".into(),
                holds: "creatures",
                wanted: "materials",
            })
        );
        // Code fills the tools and devices.
        assert!(PalettePath::parse("tools", Holds::Materials).is_err());
        assert!(PalettePath::parse("build/devices", Holds::Materials).is_err());
    }
}
