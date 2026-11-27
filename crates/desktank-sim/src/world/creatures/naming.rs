//! Names and the inspect card (SPEC §2.4, D4; T9 part 3).
//!
//! - **Names** are user-given and optional: a creature has none until a
//!   host names it ([`World::set_creature_name`]), and young are born
//!   without one. A name is cleaned before it is kept ([`clean_name`]):
//!   control characters dropped, runs of whitespace collapsed to one space,
//!   trimmed, and cut to [`MAX_NAME_CHARS`] characters; a name that cleans
//!   to nothing removes it. Names are saved (format version 6) and hashed
//!   only when set, so unnamed tanks hash as before. They don't change how
//!   a creature behaves.
//! - **The card** ([`World::creature_card`]) gathers what the inspect card
//!   shows: name, species, stage, age, health, energy, sex, the morph it
//!   shows, and how long a carrying female has to go. Hosts format it
//!   ([`CreatureCard::summary`] is one line for a title bar or a log).

use super::{Creature, CreatureId, ENERGY_RESERVE, LIFE_DAY_TICKS, LifeStage, World};

/// Longest name kept, in characters (Unicode scalar values).
pub const MAX_NAME_CHARS: usize = 24;

/// `raw` as a creature's name: control characters dropped, whitespace
/// runs collapsed to one space, trimmed and cut to [`MAX_NAME_CHARS`]
/// characters. Empty means no name.
#[must_use]
pub fn clean_name(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = 0;
    for word in raw
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
    {
        if chars > 0 {
            if chars + 1 >= MAX_NAME_CHARS {
                break;
            }
            out.push(' ');
            chars += 1;
        }
        for c in word.chars() {
            if chars == MAX_NAME_CHARS {
                break;
            }
            out.push(c);
            chars += 1;
        }
        if chars == MAX_NAME_CHARS {
            break;
        }
    }
    out
}

/// What the inspect card shows about a creature (see
/// [`World::creature_card`]).
#[derive(Clone, Debug, PartialEq)]
pub struct CreatureCard {
    /// Its handle.
    pub id: CreatureId,
    /// Its name, if it has one.
    pub name: Option<String>,
    /// Its species' name (`neon_tetra`).
    pub species: String,
    /// Its life stage.
    pub stage: LifeStage,
    /// Whole days of its life it has lived ([`LIFE_DAY_TICKS`] each).
    pub age_days: u32,
    /// Health, 0 to 1 ([`Creature::health`]).
    pub health: f32,
    /// How full its energy reserve is, 0 to 1.
    pub energy: f32,
    /// Satisfaction of supported needs, in display order.
    pub needs: Vec<super::NeedReading>,
    /// Its sex.
    pub female: bool,
    /// The morph it shows, if its species has more than one.
    pub morph: Option<String>,
    /// Whole days (rounded up) a carrying female has to go, if carrying.
    pub carrying_days: Option<u32>,
}

impl CreatureCard {
    /// The card as one line: `Bubbles · neon tetra #3 · adult female ·
    /// 12 days old · health 80% · energy 60% · red · carrying, 5 days to go ·
    /// Hunger 49%, low · Social 100%, comfortable` (the parts that apply).
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(name) = &self.name {
            parts.push(name.clone());
        }
        parts.push(format!("{} #{}", self.species.replace('_', " "), self.id.0));
        let sex = if self.female { "female" } else { "male" };
        parts.push(format!("{} {sex}", self.stage.name()));
        parts.push(match self.age_days {
            0 => "under a day old".to_owned(),
            1 => "1 day old".to_owned(),
            d => format!("{d} days old"),
        });
        parts.push(format!("health {:.0}%", self.health * 100.0));
        parts.push(format!("energy {:.0}%", self.energy * 100.0));
        if let Some(morph) = &self.morph {
            parts.push(morph.replace('_', " "));
        }
        if let Some(days) = self.carrying_days {
            parts.push(if days == 1 {
                "carrying, 1 day to go".to_owned()
            } else {
                format!("carrying, {days} days to go")
            });
        }
        parts.extend(self.needs.iter().map(|need| need.describe()));
        parts.join(" · ")
    }
}

impl Creature {
    /// Its name, if it has been given one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl World {
    /// Names a creature `name`, cleaned by [`clean_name`] (a name that
    /// cleans to nothing removes its name). Returns the name it now has
    /// (`Some("")` for none), or `None` if there is no such creature.
    pub fn set_creature_name(&mut self, id: CreatureId, name: &str) -> Option<&str> {
        let name = clean_name(name);
        let c = self.creature_mut(id)?;
        c.name = (!name.is_empty()).then(|| name.into_boxed_str());
        Some(c.name.as_deref().unwrap_or(""))
    }

    /// The inspect card for a creature, or `None` if there is no such
    /// creature.
    #[must_use]
    pub fn creature_card(&self, id: CreatureId) -> Option<CreatureCard> {
        let c = self.creature(id)?;
        let species = self.species().get(c.species)?;
        let morph = (species.morphs.len() > 1)
            .then(|| {
                species
                    .morphs
                    .get(usize::from(self.creature_morph(c)))
                    .cloned()
            })
            .flatten();
        #[allow(clippy::cast_precision_loss)] // ≤ 576 000
        let energy = c.energy as f32 / ENERGY_RESERVE as f32;
        Some(CreatureCard {
            id,
            name: c.name().map(str::to_owned),
            species: species.name.clone(),
            stage: self.life_stage(c),
            age_days: c.age / LIFE_DAY_TICKS,
            health: c.health(),
            energy,
            needs: self.creature_needs(c),
            female: c.female,
            morph,
            carrying_days: (c.carrying > 0).then(|| c.carrying.div_ceil(LIFE_DAY_TICKS)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Bubbles  "), "Bubbles");
        assert_eq!(clean_name("Mr\t\n  Fin\u{7}s"), "Mr Fin s");
        assert_eq!(clean_name(" \u{1b} \t"), "");
        assert_eq!(clean_name("Zoë 🐟"), "Zoë 🐟");
        let long = "abcdefghij".repeat(4);
        assert_eq!(clean_name(&long).chars().count(), MAX_NAME_CHARS);
        // A cut never ends in a space.
        let words = format!("{} b", "a".repeat(MAX_NAME_CHARS - 1));
        assert_eq!(clean_name(&words), "a".repeat(MAX_NAME_CHARS - 1));
        // Clean names are kept as they are.
        for n in ["Bubbles", "Mr Fin s", "Zoë 🐟", &clean_name(&long)] {
            assert_eq!(clean_name(n), n);
        }
    }
}
