//! `--play --cc`: a plain character creation screen before the game starts.
//!
//! Race, subrace, body, class, subclass, background and deity are cycled with `<` / `>`;
//! abilities are bought with `-` / `+` (point buy, `bg3_sim::cc`), the ability bonus targets
//! cycled, and the level-1 choices of the progressions (skills, cantrips and spells, fighting
//! styles, …) toggled. The choice is checked with the game's rules on every change
//! (`CcData::validate`); **Start** (or Enter) is enabled when it is valid.
//!
//! Starting re-runs the viewer with `--character <spec>` (the rest of the command line kept):
//! a second window can't be opened after the first one's event loop ends, and the level
//! viewer sets itself up before its window opens.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use bevy::prelude::*;
use bg3_loca::Loca;
use bg3_rules::context::Ability;
use bg3_sim::Guid;
use bg3_sim::cc::{CcData, CharacterChoice, Pick, SelectorKind, SelectorSlot};
use bg3_vfs::Vfs;

/// Open the screen; when the player starts, run the viewer again with the choice.
pub fn run(vfs: &Vfs, shot: Option<PathBuf>) -> Result<()> {
    let stats = bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER)?;
    let data = CcData::load(vfs, &stats)?;
    let loca = crate::dialog::load_loca(vfs, "English")?;
    let mut choice = bg3_sim::start::trace_choice();
    data.complete(&mut choice);
    let result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "bg3-viewer: character creation".into(),
            ..default()
        }),
        ..default()
    }))
    .insert_resource(Screen {
        data,
        loca,
        choice,
        dirty: true,
        result: result.clone(),
    })
    .insert_resource(ClearColor(Color::srgb(0.05, 0.04, 0.05)))
    .add_systems(Startup, setup)
    .add_systems(Update, (clicks, keys, redraw).chain());
    if let Some(path) = shot {
        app.insert_resource(crate::ShotRequest(path, None))
            .add_systems(Update, crate::take_screenshot);
    }
    app.run();
    let spec = result.lock().ok().and_then(|mut r| r.take());
    match spec {
        Some(spec) => relaunch(&spec),
        None => Ok(()),
    }
}

/// Run the viewer again with the same arguments, `--cc` replaced by `--character <spec>`.
fn relaunch(spec: &str) -> Result<()> {
    let mut args: Vec<String> = Vec::new();
    let mut skip = false;
    for a in std::env::args().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        if a == "--cc" || a.starts_with("--character=") {
            continue;
        }
        if a == "--character" {
            skip = true;
            continue;
        }
        args.push(a);
    }
    args.push("--character".into());
    args.push(spec.to_owned());
    eprintln!("character creation: starting with --character {spec}");
    let exe = std::env::current_exe()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(std::process::Command::new(exe).args(args).exec().into())
    }
    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(exe).args(args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[derive(Resource)]
struct Screen {
    data: CcData,
    loca: Loca,
    choice: CharacterChoice,
    dirty: bool,
    result: Arc<Mutex<Option<String>>>,
}

#[derive(Component)]
struct Root;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Race,
    Subrace,
    Body,
    Class,
    Subclass,
    Background,
    Deity,
}

#[derive(Component, Clone, Debug, PartialEq)]
enum Action {
    Cycle(Field, i32),
    Ability(usize, i32),
    /// Which bonus (0: `+2`, 1: `+1`), direction.
    Bonus(usize, i32),
    /// Slot index in `CcData::slots`, option.
    Toggle(usize, String),
    Start,
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Root,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            padding: UiRect::all(Val::Px(16.0)),
            column_gap: Val::Px(24.0),
            ..default()
        },
    ));
}

fn font(size: f32) -> TextFont {
    TextFont {
        font_size: bevy::text::FontSize::Px(size),
        ..default()
    }
}

const GOLD: Color = Color::srgb(0.85, 0.72, 0.45);
const GREY: Color = Color::srgb(0.65, 0.65, 0.65);
const WHITE: Color = Color::srgb(0.92, 0.92, 0.92);
const RED: Color = Color::srgb(0.9, 0.35, 0.3);

impl Screen {
    fn text(&self, handle: Option<&String>, fallback: &str) -> String {
        handle
            .and_then(|h| self.loca.text(h))
            .map(str::to_owned)
            .unwrap_or_else(|| fallback.to_owned())
    }

    fn race_name(&self, id: Option<Guid>) -> String {
        id.and_then(|g| self.data.race(g))
            .map(|r| self.text(r.display.as_ref(), &r.name))
            .unwrap_or_else(|| "-".into())
    }

    fn class_name(&self, id: Option<Guid>) -> String {
        id.and_then(|g| self.data.class(g))
            .map(|c| self.text(c.display.as_ref(), &c.name))
            .unwrap_or_else(|| "-".into())
    }

    fn option_name(&self, o: &str) -> String {
        self.text(self.data.display_names.get(o), o)
    }

    /// Body type and shape pairs the race has presets for.
    fn bodies(&self) -> Vec<(u8, u8)> {
        let c = &self.choice;
        let mut out: Vec<(u8, u8)> = self
            .data
            .presets
            .iter()
            .filter(|p| p.race == c.race && p.subrace == c.subrace)
            .map(|p| (p.body_type, p.body_shape))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Re-fill what a change left open, dropping picks of selectors the choice no longer has.
    fn settle(&mut self, drop_skills: bool) {
        let slots = self.data.slots(&self.choice);
        self.choice.picks.retain(|p| {
            slots.iter().any(|s| {
                s.progression == p.progression
                    && s.index == p.index
                    && !(drop_skills && s.selector.kind == SelectorKind::SelectSkills)
            })
        });
        if self
            .data
            .class(self.choice.class)
            .is_some_and(|c| !c.has_god)
        {
            self.choice.deity = None;
        } else if let Some(d) = self.choice.deity
            && !self
                .data
                .allowed_gods(self.choice.race, self.choice.subrace)
                .iter()
                .any(|g| g.id == d)
        {
            self.choice.deity = None;
        }
        if !self
            .bodies()
            .contains(&(self.choice.body_type, self.choice.body_shape))
            && let Some((t, s)) = self.bodies().first().copied()
        {
            self.choice.body_type = t;
            self.choice.body_shape = s;
        }
        self.data.complete(&mut self.choice);
        self.dirty = true;
    }

    fn apply(&mut self, a: &Action) {
        let cycle = |ids: Vec<Guid>, cur: Option<Guid>, dir: i32| -> Option<Guid> {
            if ids.is_empty() {
                return None;
            }
            let i = cur
                .and_then(|c| ids.iter().position(|x| *x == c))
                .map_or(0, |i| {
                    (i as i32 + dir).rem_euclid(ids.len() as i32) as usize
                });
            Some(ids[i])
        };
        match a {
            Action::Cycle(f, dir) => {
                let d = &self.data;
                let c = &mut self.choice;
                match f {
                    Field::Race => {
                        let ids = d.playable_races().iter().map(|r| r.id).collect();
                        c.race = cycle(ids, Some(c.race), *dir).unwrap_or(c.race);
                        c.subrace = None;
                        self.settle(true);
                    }
                    Field::Subrace => {
                        let ids = d.subraces(c.race).iter().map(|r| r.id).collect();
                        c.subrace = cycle(ids, c.subrace, *dir);
                        self.settle(true);
                    }
                    Field::Body => {
                        let bodies = self.bodies();
                        if !bodies.is_empty() {
                            let cur = (self.choice.body_type, self.choice.body_shape);
                            let i = bodies.iter().position(|b| *b == cur).map_or(0, |i| {
                                (i as i32 + dir).rem_euclid(bodies.len() as i32) as usize
                            });
                            (self.choice.body_type, self.choice.body_shape) = bodies[i];
                        }
                        self.dirty = true;
                    }
                    Field::Class => {
                        let ids = d.playable_classes().iter().map(|c| c.id).collect();
                        c.class = cycle(ids, Some(c.class), *dir).unwrap_or(c.class);
                        c.subclass = None;
                        c.deity = None;
                        c.abilities = [0; 6];
                        c.picks.clear();
                        self.settle(false);
                    }
                    Field::Subclass => {
                        let ids = d.level1_subclasses(c.class);
                        c.subclass = cycle(ids, c.subclass, *dir);
                        self.settle(false);
                    }
                    Field::Background => {
                        let ids = d.playable_backgrounds().iter().map(|b| b.id).collect();
                        c.background = cycle(ids, Some(c.background), *dir).unwrap_or(c.background);
                        self.settle(true);
                    }
                    Field::Deity => {
                        if d.class(c.class).is_some_and(|x| x.has_god) {
                            let ids = d
                                .allowed_gods(c.race, c.subrace)
                                .iter()
                                .map(|g| g.id)
                                .collect();
                            c.deity = cycle(ids, c.deity, *dir);
                        }
                        self.dirty = true;
                    }
                }
            }
            Action::Ability(i, dir) => {
                let mut scores = self.choice.abilities;
                scores[*i] += dir;
                if let Some(spent) = bg3_sim::cc::point_buy_spent(&scores)
                    && spent <= bg3_sim::cc::POINT_BUY_POINTS
                {
                    self.choice.abilities = scores;
                    self.dirty = true;
                }
            }
            Action::Bonus(which, dir) => {
                let slots = self.data.slots(&self.choice);
                let Some(slot) = slots
                    .iter()
                    .find(|s| s.selector.kind == SelectorKind::SelectAbilityBonus)
                else {
                    return;
                };
                let pick = pick_mut(&mut self.choice, slot);
                let n = slot.options.len() as i32;
                if pick.items.len() < slot.selector.count || n < 2 {
                    return;
                }
                let other = pick.items[1 - which].clone();
                let mut i = slot
                    .options
                    .iter()
                    .position(|o| *o == pick.items[*which])
                    .unwrap_or(0) as i32;
                loop {
                    i = (i + dir).rem_euclid(n);
                    if slot.options[i as usize] != other {
                        break;
                    }
                }
                pick.items[*which] = slot.options[i as usize].clone();
                self.dirty = true;
            }
            Action::Toggle(index, item) => {
                let slots = self.data.slots(&self.choice);
                let Some(slot) = slots.get(*index) else {
                    return;
                };
                let pick = pick_mut(&mut self.choice, slot);
                if let Some(i) = pick.items.iter().position(|x| x == item) {
                    pick.items.remove(i);
                } else {
                    if pick.items.len() >= slot.selector.count && !pick.items.is_empty() {
                        pick.items.remove(0);
                    }
                    pick.items.push(item.clone());
                }
                self.dirty = true;
            }
            Action::Start => {
                if self.data.validate(&self.choice).is_empty()
                    && let Ok(mut r) = self.result.lock()
                {
                    *r = Some(self.choice.to_spec());
                }
            }
        }
    }
}

fn pick_mut<'a>(choice: &'a mut CharacterChoice, slot: &SelectorSlot) -> &'a mut Pick {
    let i = match choice
        .picks
        .iter()
        .position(|p| p.progression == slot.progression && p.index == slot.index)
    {
        Some(i) => i,
        None => {
            choice.picks.push(Pick {
                progression: slot.progression,
                index: slot.index,
                items: Vec::new(),
            });
            choice.picks.len() - 1
        }
    };
    &mut choice.picks[i]
}

fn clicks(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    nodes: Query<(
        &Action,
        &ComputedNode,
        &UiGlobalTransform,
        &InheritedVisibility,
    )>,
    mut screen: ResMut<Screen>,
    mut exit: MessageWriter<AppExit>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(p) = windows
        .iter()
        .next()
        .and_then(|w| w.physical_cursor_position())
    else {
        return;
    };
    let hit = nodes
        .iter()
        .find(|(_, n, t, v)| v.get() && n.contains_point(**t, p))
        .map(|(a, ..)| a.clone());
    if let Some(a) = hit {
        screen.apply(&a);
        if a == Action::Start && screen.result.lock().is_ok_and(|r| r.is_some()) {
            exit.write(AppExit::Success);
        }
    }
}

fn keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut screen: ResMut<Screen>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Enter) {
        screen.apply(&Action::Start);
        if screen.result.lock().is_ok_and(|r| r.is_some()) {
            exit.write(AppExit::Success);
        }
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

fn button(p: &mut ChildSpawnerCommands, label: &str, action: Action, color: Color, on: bool) {
    p.spawn((
        action,
        Node {
            padding: UiRect::axes(Val::Px(6.0), Val::Px(1.0)),
            margin: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(if on {
            Color::srgba(0.45, 0.33, 0.12, 0.95)
        } else {
            Color::srgba(0.18, 0.16, 0.18, 0.95)
        }),
    ))
    .with_child((Text::new(label), font(14.0), TextColor(color)));
}

fn row(p: &mut ChildSpawnerCommands, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn(Node {
        align_items: AlignItems::Center,
        flex_wrap: FlexWrap::Wrap,
        column_gap: Val::Px(4.0),
        ..default()
    })
    .with_children(f);
}

fn label(p: &mut ChildSpawnerCommands, text: &str, width: f32, color: Color) {
    p.spawn((
        Node {
            width: Val::Px(width),
            ..default()
        },
        Text::new(text),
        font(16.0),
        TextColor(color),
    ));
}

fn cycler(p: &mut ChildSpawnerCommands, name: &str, value: &str, field: Field, enabled: bool) {
    row(p, |r| {
        label(r, name, 110.0, GOLD);
        if enabled {
            button(r, "<", Action::Cycle(field, -1), WHITE, false);
        }
        r.spawn((
            Node {
                min_width: Val::Px(220.0),
                ..default()
            },
            Text::new(value),
            font(14.0),
            TextColor(if enabled { WHITE } else { GREY }),
        ));
        if enabled {
            button(r, ">", Action::Cycle(field, 1), WHITE, false);
        }
    });
}

fn redraw(mut commands: Commands, mut screen: ResMut<Screen>, root: Query<Entity, With<Root>>) {
    if !screen.dirty {
        return;
    }
    screen.dirty = false;
    let Ok(root) = root.single() else {
        return;
    };
    commands.entity(root).despawn_related::<Children>();
    let s = &*screen;
    let d = &s.data;
    let c = &s.choice;
    let errors = d.validate(c);
    let built = d.build(c).ok();
    let slots = d.slots(c);
    commands.entity(root).with_children(|root| {
        // Left: identity, abilities, result.
        root.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            width: Val::Percent(42.0),
            ..default()
        })
        .with_children(|col| {
            col.spawn((Text::new("Character creation"), font(24.0), TextColor(GOLD)));
            cycler(col, "Race", &s.race_name(Some(c.race)), Field::Race, true);
            let subs = d.subraces(c.race);
            cycler(
                col,
                "Subrace",
                &s.race_name(c.subrace),
                Field::Subrace,
                subs.len() > 1,
            );
            let body = format!(
                "{}{}",
                if c.body_type == 0 {
                    "Masculine"
                } else {
                    "Feminine"
                },
                if c.body_shape == 1 { ", strong" } else { "" }
            );
            cycler(col, "Body", &body, Field::Body, s.bodies().len() > 1);
            cycler(
                col,
                "Class",
                &s.class_name(Some(c.class)),
                Field::Class,
                true,
            );
            cycler(
                col,
                "Subclass",
                &s.class_name(c.subclass),
                Field::Subclass,
                d.level1_subclasses(c.class).len() > 1,
            );
            let bg = d
                .background(c.background)
                .map(|b| s.text(b.display.as_ref(), "?"))
                .unwrap_or_default();
            cycler(col, "Background", &bg, Field::Background, true);
            let has_god = d.class(c.class).is_some_and(|x| x.has_god);
            let god = c
                .deity
                .and_then(|g| d.god(g))
                .map(|g| s.text(g.display.as_ref(), &g.name))
                .unwrap_or_else(|| "-".into());
            cycler(col, "Deity", &god, Field::Deity, has_god);

            let spent = bg3_sim::cc::point_buy_spent(&c.abilities).unwrap_or(0);
            col.spawn((
                Text::new(format!(
                    "Abilities: {} of {} points left",
                    bg3_sim::cc::POINT_BUY_POINTS - spent,
                    bg3_sim::cc::POINT_BUY_POINTS
                )),
                font(18.0),
                TextColor(GOLD),
            ));
            for (i, a) in Ability::ALL.iter().enumerate() {
                let fin = built.as_ref().map_or(c.abilities[i], |b| b.abilities[i]);
                row(col, |r| {
                    label(r, a.name(), 110.0, WHITE);
                    button(r, "-", Action::Ability(i, -1), WHITE, false);
                    label(r, &format!("{:>2}", c.abilities[i]), 30.0, WHITE);
                    button(r, "+", Action::Ability(i, 1), WHITE, false);
                    label(
                        r,
                        &format!("= {fin} ({:+})", (fin - 10).div_euclid(2)),
                        110.0,
                        GREY,
                    );
                });
            }
            if let Some(slot) = slots
                .iter()
                .find(|x| x.selector.kind == SelectorKind::SelectAbilityBonus)
            {
                let items = c
                    .picks
                    .iter()
                    .find(|p| p.progression == slot.progression && p.index == slot.index)
                    .map(|p| p.items.clone())
                    .unwrap_or_default();
                for (which, amount) in slot.selector.amounts.iter().enumerate().take(2) {
                    let v = items.get(which).cloned().unwrap_or_else(|| "-".into());
                    row(col, |r| {
                        label(r, &format!("Bonus {amount:+}"), 110.0, GOLD);
                        button(r, "<", Action::Bonus(which, -1), WHITE, false);
                        label(r, &v, 140.0, WHITE);
                        button(r, ">", Action::Bonus(which, 1), WHITE, false);
                    });
                }
            }
            if let Some(b) = &built {
                let list = |v: Vec<String>| v.join(", ");
                for line in [
                    format!("HP {}   AC {}   level {}", b.max_hp, b.armor_class, b.level),
                    format!(
                        "Saving throws: {}",
                        list(b.saving_throws.iter().map(|x| x.to_string()).collect())
                    ),
                    format!(
                        "Skills: {}",
                        list(b.skills.iter().map(|x| x.to_string()).collect())
                    ),
                    format!(
                        "Spells: {}",
                        list(b.spells.iter().map(|x| s.option_name(x)).collect())
                    ),
                    format!("Equipment: {}", list(b.equipment.clone())),
                ] {
                    col.spawn((Text::new(line), font(13.0), TextColor(GREY)));
                }
            }
            for e in &errors {
                col.spawn((Text::new(e.to_string()), font(15.0), TextColor(RED)));
            }
            row(col, |r| {
                let ok = errors.is_empty();
                button(
                    r,
                    if ok { "Start (Enter)" } else { "Start" },
                    Action::Start,
                    if ok { WHITE } else { GREY },
                    ok,
                );
            });
        });
        // Right: the progressions' choices.
        root.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            width: Val::Percent(56.0),
            ..default()
        })
        .with_children(|col| {
            for (i, slot) in slots.iter().enumerate() {
                if slot.selector.kind == SelectorKind::SelectAbilityBonus {
                    continue;
                }
                let items = c
                    .picks
                    .iter()
                    .find(|p| p.progression == slot.progression && p.index == slot.index)
                    .map(|p| p.items.clone())
                    .unwrap_or_default();
                col.spawn((
                    Text::new(format!(
                        "{} ({} of {})",
                        slot.label(),
                        items.len(),
                        slot.selector.count
                    )),
                    font(17.0),
                    TextColor(GOLD),
                ));
                row(col, |r| {
                    for o in &slot.options {
                        let on = items.contains(o);
                        button(
                            r,
                            &s.option_name(o),
                            Action::Toggle(i, o.clone()),
                            WHITE,
                            on,
                        );
                    }
                });
            }
        });
    });
}
