//! Running the story on the sim without a trace: deliver the sim's queued events to the VM
//! one at a time, each evaluated completely before the next (as the engine does), until the
//! queue is empty.
//!
//! [`Game`] packages this for a live front end: it owns the story VM and the [`Sim`], is
//! ticked once per frame, takes the player's input (dialog choices, finished lines) and hands
//! out [`Notification`]s for the presentation layer.

use bg3_osiris::trace::{LogData, LogValue};
use bg3_osiris::{Value, Vm};
use bg3_story::Story;

use crate::dialog::{DialogError, Notification};
use crate::events::{SimEvent, SimValue};
use crate::host::SimHost;
use crate::sim::Sim;

/// The event's arguments as VM values, typed by the event's declared parameters.
pub fn event_values(vm: &mut Vm, e: &SimEvent) -> Vec<Value> {
    let params = vm
        .function_params(&e.name, e.args.len())
        .unwrap_or_default();
    e.args
        .iter()
        .enumerate()
        .map(|(i, a)| match a {
            SimValue::Int(i) => Value::int(*i),
            SimValue::Real(r) => Value::real(*r),
            SimValue::Str(t) => vm.string(t),
            SimValue::Guid(t) => vm.guid(t),
            SimValue::Enum(t, l) => {
                let lv = LogValue {
                    ty: None,
                    v: LogData::Enum(t.clone(), l.clone()),
                };
                bg3_osiris::replay::to_value(vm, &lv, params.get(i).copied().unwrap_or(0))
            }
        })
        .collect()
}

/// Deliver an engine event (one the sim raised, or an outside one like `LevelLoaded`) to the
/// story with the sim as host.
pub fn deliver(vm: &mut Vm, sim: &mut Sim, e: &SimEvent) {
    let args = event_values(vm, e);
    vm.event(&mut SimHost::new(sim), &e.name, &args);
}

/// Deliver queued events until none are left: each round hands out the whole queue (after
/// the end-of-frame tag merge) and delivers it in order; events raised meanwhile wait for the
/// next round. At most `max_rounds` rounds (a story can keep raising events forever, e.g. a
/// zero-length timer relaunched on finish). Returns how many events were delivered.
pub fn run_events(vm: &mut Vm, sim: &mut Sim, max_rounds: usize) -> usize {
    let mut n = 0;
    for _ in 0..max_rounds {
        let batch = sim.take_events();
        if batch.is_empty() {
            break;
        }
        for e in &batch {
            deliver(vm, sim, e);
        }
        n += batch.len();
    }
    n
}

/// The story and the world together: what a front end drives.
///
/// ```text
/// Game::new → init (story INIT + an outside event or two, e.g. LevelLoaded)
/// every frame: tick(dt) → take_notifications() → show lines / choices
/// player input: dialog_advance / dialog_choose (events are delivered at the next tick)
/// ```
pub struct Game {
    pub vm: Vm,
    pub sim: Sim,
    /// Event rounds delivered per tick at most (see [`run_events`]).
    pub max_rounds: usize,
}

impl Game {
    pub fn new(story: &Story, sim: Sim) -> Self {
        Self {
            vm: Vm::new(story),
            sim,
            max_rounds: 64,
        }
    }

    /// Run the story's `INIT` sections.
    pub fn init(&mut self) {
        self.vm.init_game(&mut SimHost::new(&mut self.sim));
    }

    /// Hand an event from outside the sim (level loading, input the sim does not model yet)
    /// to the sim and the story, then deliver what it caused.
    pub fn event(&mut self, e: SimEvent) {
        let texts: Vec<crate::sim::ObservedArg> = e
            .args
            .iter()
            .map(|a| crate::sim::ObservedArg {
                ty: String::new(),
                text: match a {
                    SimValue::Str(s) | SimValue::Guid(s) => s.clone(),
                    other => other.to_string(),
                },
            })
            .collect();
        self.sim.observe(&e.name, &texts);
        deliver(&mut self.vm, &mut self.sim, &e);
        run_events(&mut self.vm, &mut self.sim, self.max_rounds);
    }

    /// One frame: requested dialogs start, timers run, and every event raised is delivered.
    /// Returns how many events the story received.
    pub fn tick(&mut self, dt: f64) -> usize {
        self.sim.tick(dt);
        run_events(&mut self.vm, &mut self.sim, self.max_rounds)
    }

    /// The player finished reading a line.
    pub fn dialog_advance(&mut self, instance: u32) -> Result<(), DialogError> {
        self.sim.dialog_advance(instance)
    }

    /// The player picked a dialog option.
    pub fn dialog_choose(&mut self, instance: u32, index: usize) -> Result<(), DialogError> {
        self.sim.dialog_choose(instance, index)
    }

    pub fn take_notifications(&mut self) -> Vec<Notification> {
        self.sim.take_notifications()
    }
}
