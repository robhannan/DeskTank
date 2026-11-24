//! Applies recognized touches to the shared tank's tools.

use desktank_render::{BrushPreview, Magnifier, Tank};
use desktank_sim::tools::{Button, Inspection, Outcome, PaletteItem, Tool, Tools};
use desktank_sim::{MaterialId, World};

use crate::touch::TouchAction;

pub struct Brush {
    tools: Tools,
    stroke_cells: usize,
}

impl Brush {
    pub fn new(sand: MaterialId) -> Self {
        Self {
            tools: Tools::new(sand),
            stroke_cells: 0,
        }
    }

    /// Applies an action and reports whether it changed the saved world.
    pub fn apply(&mut self, action: TouchAction, tank: &mut Tank, size: (u32, u32)) -> bool {
        let outcome = match action {
            TouchAction::Press(button, p) => {
                self.stroke_cells = 0;
                let cell = self.cell(tank, size, p);
                self.tools.press(tank.world_mut(), button, cell)
            }
            TouchAction::Move(p) => {
                let cell = self.cell(tank, size, p);
                self.tools.moved(tank.world_mut(), cell)
            }
            TouchAction::Release(button) => {
                let verb = match button {
                    Button::Primary => "poured",
                    Button::Secondary => "scooped",
                };
                log::info!("touch: {verb} {} cells", self.stroke_cells);
                self.tools.release(tank.world_mut(), button)
            }
            TouchAction::Leave => {
                self.tools.leave(tank.world_mut());
                Outcome::None
            }
            TouchAction::InspectAt(p) => {
                let cell = self.cell(tank, size, p);
                // Inspect without changing the active tool or brush cursor.
                let mut inspect = self.tools.clone();
                inspect.select(PaletteItem::Tool(Tool::Inspect));
                inspect.press(tank.world_mut(), Button::Primary, cell)
            }
        };
        self.sync_cursor(tank);
        match outcome {
            Outcome::Painted(n) | Outcome::Glued(n) => {
                self.stroke_cells = self.stroke_cells.saturating_add(n);
                n > 0
            }
            Outcome::Inspected(found) => {
                log_inspection(tank.world(), found);
                false
            }
            Outcome::Caught(_) | Outcome::Dropped(_) | Outcome::Removed(_) | Outcome::Added(_) => {
                true
            }
            Outcome::None | Outcome::AddRefused { .. } | Outcome::Zoomed(_) => false,
        }
    }

    pub fn hold(&mut self, world: &mut World, ticks: u32) -> bool {
        let mut changed = 0usize;
        for _ in 0..ticks {
            changed = changed.saturating_add(self.tools.hold(world));
        }
        self.stroke_cells = self.stroke_cells.saturating_add(changed);
        changed > 0
    }

    fn cell(&self, tank: &Tank, size: (u32, u32), p: (f64, f64)) -> (i64, i64) {
        let (x, y) = tank.layout(size).cell_at(p.0, p.1);
        // As on the web, bound the work of an off-surface drag.
        let pad = i64::from(self.tools.radius()) + 1;
        let clamp = |v: i64, len: usize| {
            let len = i64::try_from(len).unwrap_or(0);
            v.clamp(-pad, len - 1 + pad)
        };
        let world = tank.world();
        (clamp(x, world.width()), clamp(y, world.height()))
    }

    fn sync_cursor(&self, tank: &mut Tank) {
        tank.set_cursor(self.tools.preview().map(|(x, y, radius)| BrushPreview {
            cell: (x, y),
            radius,
        }));
        tank.set_device_ghost(self.tools.ghost(tank.world()));
        tank.set_magnifier(
            self.tools
                .magnifier()
                .map(|(x, y, zoom)| Magnifier { cell: (x, y), zoom }),
        );
    }
}

fn log_inspection(world: &World, found: Inspection) {
    let (kind, name, x, y) = match found {
        Inspection::Cell { x, y, material } => (
            "cell",
            world.materials().get(material).map(|m| m.name.as_str()),
            x,
            y,
        ),
        Inspection::Plant { id, x, y, .. } => (
            "plant",
            world
                .plant(id)
                .and_then(|p| world.plant_species().get(p.species))
                .map(|s| s.name.as_str()),
            x,
            y,
        ),
        Inspection::Device { id, x, y } => (
            "device",
            world.device(id).map(|device| device.kind.name()),
            x,
            y,
        ),
        Inspection::Creature { species, x, y, .. } => (
            "creature",
            world.species().get(species).map(|s| s.name.as_str()),
            x,
            y,
        ),
    };
    log::info!(
        "touch: inspect {kind} {} at ({x}, {y})",
        name.unwrap_or("?")
    );
}
