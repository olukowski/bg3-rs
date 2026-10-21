//! Running the story on the sim without a trace: deliver the sim's queued events to the VM
//! one at a time, each evaluated completely before the next (as the engine does), until the
//! queue is empty.

use bg3_osiris::{Value, Vm};

use crate::events::{SimEvent, SimValue};
use crate::host::SimHost;
use crate::sim::Sim;

/// The event's arguments as VM values.
pub fn event_values(vm: &mut Vm, e: &SimEvent) -> Vec<Value> {
    e.args
        .iter()
        .map(|a| match a {
            SimValue::Guid(t) => vm.guid(t),
            SimValue::Str(t) => vm.string(t),
            SimValue::Int(i) => Value::int(*i),
            SimValue::Real(r) => Value::real(*r),
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
