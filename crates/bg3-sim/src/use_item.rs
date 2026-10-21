//! Item-use lifecycle. The log establishes event order, not animation/frame duration.
//! Headless uses complete on the next tick after their start events were delivered.
use crate::{EntityKind, Game, Guid, Sim, SimValue, story_data::UseAction};

#[derive(Clone, Debug)]
pub struct ItemUse {
    pub user: Guid,
    pub item: Guid,
    pub actions: Vec<UseAction>,
    /// Raw script options: retained until their full meaning is established.
    pub options: [i32; 2],
    pub completion_event: String,
    started: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ItemUses {
    pub pending: Vec<ItemUse>,
}

impl Sim {
    /// The nearest explicit action list wins, including an empty list. Instance overrides
    /// precede root templates. Combat actions are retained but not executed yet.
    // bg3: esv::OsirisItemFunctions::HasOnUse (4.1.1.7631656)
    pub fn item_use_actions(&self, item: Guid) -> Option<Vec<UseAction>> {
        let e = self.entity(item)?;
        if e.kind != EntityKind::Item {
            return None;
        }
        let mut ids = vec![item];
        ids.extend(self.data.template_chain(e.template).iter().map(|t| t.id));
        ids.iter()
            .filter_map(|id| self.data.story.objects.get(id))
            .find_map(|d| d.use_peace.as_ref())
            .cloned()
    }

    /// A player interaction; rejects disabled items and items without use actions.
    pub fn use_item(&mut self, user: Guid, item: Guid) -> bool {
        let Some(e) = self.entity(item) else {
            return false;
        };
        let enabled = e
            .story
            .can_interact
            .or_else(|| {
                self.data
                    .story
                    .objects
                    .get(&item)
                    .and_then(|d| d.can_interact)
            })
            .or_else(|| {
                self.data
                    .template_chain(e.template)
                    .iter()
                    .find_map(|t| self.data.story.objects.get(&t.id)?.can_interact)
            })
            .unwrap_or(true);
        enabled && self.begin_item_use(user, item, [0, 0], "")
    }

    /// Script requests are allowed on items disabled for player interaction (the opening
    /// disables its chairs before ordering NPCs to sit on them).
    // bg3: esv::OsirisCharacterFunctions::Use (4.1.1.7631656)
    pub fn use_item_scripted(
        &mut self,
        user: Guid,
        item: Guid,
        option_a: i32,
        option_b: i32,
        event: &str,
    ) -> bool {
        self.begin_item_use(user, item, [option_a, option_b], event)
    }

    fn begin_item_use(&mut self, user: Guid, item: Guid, options: [i32; 2], event: &str) -> bool {
        if self.in_combat(user)
            || !self
                .entity(user)
                .is_some_and(|e| e.kind == EntityKind::Character)
            || self.item_uses.pending.iter().any(|u| u.user == user)
        {
            return false;
        }
        let Some(actions) = self.item_use_actions(item).filter(|v| !v.is_empty()) else {
            return false;
        };
        // The trace exercises the story interaction (8), sit (14), and lie (15)
        // actions. Other action executors (spells, consumption, locks, ...) are absent.
        // A conditional/healing action also needs an executor before claiming success.
        if actions.iter().any(|a| {
            !matches!(a.kind, 8 | 14 | 15)
                || a.attributes.as_ref().is_some_and(|n| {
                    n.attr_str("Conditions").is_some_and(|s| !s.is_empty())
                        || n.attr("Heal")
                            .and_then(|v| v.as_f64())
                            .is_some_and(|v| v != 0.0)
                })
        }) {
            return false;
        }
        self.item_uses.pending.push(ItemUse {
            user,
            item,
            actions,
            options,
            completion_event: event.into(),
            started: false,
        });
        true
    }

    // bg3: esv::OsirisUseItemTask::OnStart (4.1.1.7631656)
    pub(crate) fn update_item_uses(&mut self) {
        let pending = std::mem::take(&mut self.item_uses.pending);
        for mut u in pending {
            let user = SimValue::Guid(self.render(u.user));
            let item = SimValue::Guid(self.render(u.item));
            let template = self.entity(u.item).and_then(|e| e.template).map(|id| {
                SimValue::Guid(
                    self.data
                        .templates
                        .get(&id)
                        .map_or_else(|| id.to_string(), |t| crate::guid::osiris_name(&t.name, id)),
                )
            });
            if !u.started {
                if u.actions.iter().any(|a| matches!(a.kind, 14 | 15))
                    && let Some(e) = self.entities.get_mut(u.user)
                {
                    e.story.using_item = Some(u.item);
                }

                // In the trace the script completion EntityEvent precedes UseStarted (e.g.
                // the operation chair); the chair's animation/occupancy is not simulated.
                if !u.completion_event.is_empty() {
                    self.push_event(
                        "EntityEvent",
                        vec![user.clone(), SimValue::Str(u.completion_event.clone())],
                    );
                }
                self.push_event("UseStarted", vec![user.clone(), item.clone()]);
                if let Some(template) = template {
                    self.push_event("TemplateUseStarted", vec![user, template, item]);
                }
                u.started = true;
                self.item_uses.pending.push(u);
            } else {
                let success = SimValue::Int(i32::from(self.exists(u.user) && self.exists(u.item)));
                self.push_event(
                    "UseFinished",
                    vec![user.clone(), item.clone(), success.clone()],
                );
                if let Some(template) = template {
                    self.push_event("TemplateUseFinished", vec![user, template, item, success]);
                }
            }
        }
    }
}

impl Game {
    /// Request a use; the next ticks deliver start and completion events to the story.
    pub fn use_item(&mut self, user: Guid, item: Guid) -> bool {
        self.sim.use_item(user, item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entity, GameData, Origin, story_data::ObjectData};

    fn fixture(kind: i32) -> Sim {
        let mut data = GameData::default();
        data.story.objects.insert(
            Guid(2),
            ObjectData {
                use_peace: Some(vec![UseAction {
                    kind,
                    attributes: None,
                }]),
                ..Default::default()
            },
        );
        let mut sim = Sim::new(data);
        sim.entities.insert(Entity::new(
            Guid(1),
            "User",
            EntityKind::Character,
            Origin::Data,
        ));
        sim.entities
            .insert(Entity::new(Guid(2), "Item", EntityKind::Item, Origin::Data));
        sim
    }

    #[test]
    fn disabled_player_interaction_still_allows_the_scripted_chair_use() {
        let mut sim = fixture(14);
        sim.entities.get_mut(Guid(2)).unwrap().story.can_interact = Some(false);
        assert!(!sim.use_item(Guid(1), Guid(2)));
        assert!(sim.use_item_scripted(Guid(1), Guid(2), 1, 0, "Seated"));
        assert!(
            !sim.use_item_scripted(Guid(1), Guid(2), 1, 0, "Seated"),
            "one request per user"
        );
        sim.tick(0.0);
        let events = sim.take_events();
        assert_eq!(
            events.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["EntityEvent", "UseStarted"]
        );
        assert_eq!(sim.entity(Guid(1)).unwrap().story.using_item, Some(Guid(2)));
        sim.tick(0.0);
        let events = sim.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "UseFinished");
        assert_eq!(events[0].args.last(), Some(&SimValue::Int(1)));
        sim.tick(0.0);
        assert!(
            sim.take_events().is_empty(),
            "one completion per accepted request"
        );
    }

    #[test]
    fn unsupported_actions_are_rejected_and_removal_during_use_fails_completion() {
        let mut sim = fixture(999);
        assert!(!sim.use_item(Guid(1), Guid(2)));
        assert!(sim.item_uses.pending.is_empty());
        let mut sim = fixture(8);
        assert!(sim.use_item(Guid(1), Guid(2)));
        sim.tick(0.0);
        sim.take_events();
        sim.entities.remove_where(|e| e.id == Guid(2));
        sim.tick(0.0);
        assert_eq!(sim.take_events()[0].args.last(), Some(&SimValue::Int(0)));
    }
}
