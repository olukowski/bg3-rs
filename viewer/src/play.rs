//! `--play`: the viewer driven by the live game (`bg3_sim::Game`).
//!
//! The story VM and the sim run on their own thread (rule chains recurse deeply, so it gets a
//! big stack, like `bg3x sim-replay`). Each frame the viewer sends the frame time and the
//! player's input; the thread ticks the game and answers with the presentation state
//! ([`bg3_sim::Presence`]: on stage, visible, position, dead) when it changed, plus the dialog
//! notifications. The viewer:
//!
//! * shows, hides and places characters and items as the sim says (story teleports, scripted
//!   moves, `SetOnStage`);
//! * writes the controlled avatar's position back to the sim, so trigger volumes fire;
//! * sends a left click on a character as the dialog request the engine raises
//!   (`DialogStartRequested`; the story picks the dialog);
//! * shows the dialogs the story runs in a panel (number keys choose, Space/Enter skip a line,
//!   lines also continue after a reading delay), with roll results.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Result, anyhow};
use bevy::prelude::*;
use bg3_dialog::model::SPEAKER_NARRATOR;
use bg3_loca::Loca;
use bg3_sim::{Guid, Notification, Presence};

use crate::characters::CharacterRoot;
use crate::combat_ui::{CombatCmd, CombatInbox, CombatOut, CombatOutbox, CombatThread, CombatWalk};
use crate::level::ObjectId;
use crate::nav::Controlled;
use crate::space;

/// Reading time of a line (see `dialog.rs`).
const LINE_BASE_SECS: f32 = 2.5;
const LINE_SECS_PER_CHAR: f32 = 0.05;
/// Nodes without text (cinematics we don't play) pause this long.
const SILENT_LINE_SECS: f32 = 0.5;
/// A click within this many pixels of a character's chest picks it.
const PICK_RADIUS_PX: f32 = 40.0;

/// Player input for the game thread.
enum Cmd {
    Tick(f64),
    Advance(u32),
    Choose(u32, usize),
    SetPosition(Guid, [f32; 3]),
    Talk(Guid),
    /// The avatar uses an item (a click on it).
    UseItem(Guid),
    Combat(CombatCmd),
}

/// What the game thread reports after a tick.
struct Frame {
    names: HashMap<Guid, String>,
    presences: Option<Vec<Presence>>,
    notifications: Vec<Notification>,
    combat: CombatOut,
}

/// The state right after the new game started.
pub struct Started {
    pub names: HashMap<Guid, String>,
    pub avatar: Guid,
    pub avatar_template: Guid,
    /// Actual CC result, including class/race starting items (read from the sim).
    pub avatar_equipment: Vec<String>,
    /// Active weapon set of each character presence (`Sim::weapon_set`), by id.
    pub weapon_sets: HashMap<String, bg3_world::unsheath::WeaponSet>,
    pub level: String,
    pub presences: Vec<Presence>,
    /// Items with use actions (`Sim::item_use_actions`): clicking them uses them.
    pub usable_items: Vec<(Guid, String)>,
}

/// The connection to the game thread.
#[derive(Resource)]
pub struct SimLink {
    tx: Sender<Cmd>,
    rx: std::sync::Mutex<Receiver<Frame>>,
    pub avatar: Guid,
    usable_items: HashMap<Guid, String>,
}

impl SimLink {
    /// Load the game on its thread and start a new campaign; returns once it has started.
    ///
    /// With `combat_test`, the party is placed for the deck fight right away
    /// ([`crate::combat_ui::setup_test_fight`]).
    pub fn start(
        vfs: Arc<bg3_vfs::Vfs>,
        combat_test: bool,
        grid: Option<Arc<bg3_nav::NavGrid>>,
        character: Option<bg3_sim::cc::CharacterChoice>,
    ) -> Result<(Self, Started)> {
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let (frame_tx, frame_rx) = channel::<Frame>();
        let (start_tx, start_rx) = channel::<Result<Started, String>>();
        std::thread::Builder::new()
            .name("bg3-game".into())
            .stack_size(1 << 30)
            .spawn(move || {
                game_thread(
                    vfs,
                    combat_test,
                    grid,
                    character,
                    start_tx,
                    cmd_rx,
                    frame_tx,
                )
            })?;
        let started = start_rx
            .recv()
            .map_err(|_| anyhow!("game thread died"))?
            .map_err(|e| anyhow!(e))?;
        Ok((
            Self {
                tx: cmd_tx,
                rx: std::sync::Mutex::new(frame_rx),
                avatar: started.avatar,
                usable_items: started.usable_items.iter().cloned().collect(),
            },
            started,
        ))
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

/// The sim's presences plus the origin characters (`Origins.lsx`) it doesn't place in the
/// level yet, off stage: companions the story brings in later (Lae'zel on the lower deck)
/// then already have a model to show.
fn presences(game: &bg3_sim::Game) -> Vec<Presence> {
    let mut out = game.sim.presences();
    for id in game.sim.data.combat.origins.keys() {
        if out.iter().any(|p| p.id == *id) {
            continue;
        }
        let Some(e) = game.sim.entity(*id) else {
            continue;
        };
        out.push(Presence {
            id: *id,
            name: e.name.clone(),
            kind: e.kind,
            template: e.template,
            level: e.level.clone(),
            position: e.position,
            on_stage: false,
            visible: false,
            dead: false,
            moving: false,
        });
    }
    out
}

fn game_thread(
    vfs: Arc<bg3_vfs::Vfs>,
    combat_test: bool,
    grid: Option<Arc<bg3_nav::NavGrid>>,
    character: Option<bg3_sim::cc::CharacterChoice>,
    start_tx: Sender<Result<Started, String>>,
    rx: Receiver<Cmd>,
    tx: Sender<Frame>,
) {
    let t = std::time::Instant::now();
    let (mut game, mut opts) = match bg3_sim::start::load_game(vfs) {
        Ok(g) => g,
        Err(e) => {
            let _ = start_tx.send(Err(e.to_string()));
            return;
        }
    };
    if let Some(grid) = &grid {
        game.sim
            .set_pathfinder(Some(Arc::new(bg3_sim_nav::GridPathfinder::new(
                grid.clone(),
            ))));
    }
    opts.character = character;
    let started = bg3_sim::start::new_game(&mut game, &opts);
    eprintln!(
        "play: new game in {:.1?}: {} in {} ({} triggers)",
        t.elapsed(),
        game.sim.render(started.avatar),
        started.level,
        opts.startup_triggers.len()
    );
    if combat_test {
        crate::combat_ui::setup_test_fight(&mut game, started.avatar);
    }
    let mut last = presences(&game);
    let usable_items: Vec<(Guid, String)> = last
        .iter()
        .filter(|p| p.kind == bg3_sim::EntityKind::Item)
        .filter(|p| {
            game.sim
                .item_use_actions(p.id)
                .is_some_and(|a| !a.is_empty())
        })
        .map(|p| (p.id, p.name.clone()))
        .collect();
    let _ = start_tx.send(Ok(Started {
        names: display_names(&game.sim),
        avatar: started.avatar,
        avatar_template: started.avatar_template,
        avatar_equipment: game
            .sim
            .combat
            .created_characters
            .get(&started.avatar)
            .map(|c| c.equipment.clone())
            .unwrap_or_default(),
        weapon_sets: last
            .iter()
            .filter(|p| p.kind == bg3_sim::EntityKind::Character)
            .map(|p| (p.id.to_string(), game.sim.weapon_set(p.id)))
            .collect(),
        level: started.level,
        presences: last.clone(),
        usable_items,
    }));
    let avatar = started.avatar;
    let mut pending = game.take_notifications();
    let mut combat = CombatThread::new(grid);
    for cmd in rx {
        match cmd {
            Cmd::Tick(dt) => {
                game.tick_live(dt);
                let combat = combat.tick(&mut game);
                pending.extend(game.take_notifications());
                let now = presences(&game);
                let presences = (now != last).then(|| now.clone());
                last = now;
                let frame = Frame {
                    names: display_names(&game.sim),
                    presences,
                    notifications: std::mem::take(&mut pending),
                    combat,
                };
                if tx.send(frame).is_err() {
                    break;
                }
            }
            Cmd::Advance(i) => {
                if let Err(e) = game.dialog_advance(i) {
                    eprintln!("play: dialog {i}: {e}");
                }
            }
            Cmd::Choose(i, n) => {
                if let Err(e) = game.dialog_choose(i, n) {
                    eprintln!("play: dialog {i}: {e}");
                }
            }
            Cmd::SetPosition(id, p) => {
                game.sim.set_position(id, p);
            }
            Cmd::Talk(target) => {
                eprintln!("play: {} asks to talk", game.sim.render(target));
                game.sim.request_dialog_with(target, avatar);
            }
            Cmd::UseItem(item) => {
                let ok = game.use_item(avatar, item);
                eprintln!("play: use {} -> {ok}", game.sim.render(item));
            }
            Cmd::Combat(c) => combat.apply(&mut game, c),
        }
    }
    let mut missing: Vec<_> = game.sim.stage.missing.iter().collect();
    missing.sort_by(|a, b| b.1.cmp(a.1));
    eprintln!(
        "play: {} story functions not implemented (most used: {})",
        missing.len(),
        missing
            .iter()
            .take(8)
            .map(|(n, c)| format!("{n} ×{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// The latest presentation state, by object.
#[derive(Resource, Default)]
pub(crate) struct SimState {
    by_id: HashMap<Guid, Presence>,
    /// Where the controlled avatar was last reported to the sim.
    pub(crate) sent: Option<[f32; 3]>,
}

/// One dialog the panel follows.
#[derive(Default)]
struct DialogView {
    name: String,
    speaker: String,
    text: String,
    /// Waiting for the player to finish reading a line.
    line: bool,
    /// The line has no text (a cinematic node): it continues after a short pause.
    silent: bool,
    timer: f32,
    choices: Vec<(String, bool)>,
    rolls: Vec<String>,
}

/// Dialogs in progress and what the panel shows.
#[derive(Resource)]
pub(crate) struct Dialogs {
    loca: Arc<Loca>,
    /// Display names by object.
    names: HashMap<Guid, String>,
    views: HashMap<u32, DialogView>,
    /// Instance shown in the panel.
    shown: Option<u32>,
    dirty: bool,
}

#[derive(Component)]
pub(crate) struct PlayPanel;

pub struct PlayPlugin {
    pub link: std::sync::Mutex<Option<SimLink>>,
    pub loca: Arc<Loca>,
    pub names: HashMap<Guid, String>,
    /// Talk to this character (name substring) once no dialog runs (`--talk`).
    pub talk: Option<String>,
}

#[derive(Resource)]
struct TalkOnStart(Option<String>);

impl Plugin for PlayPlugin {
    fn build(&self, app: &mut App) {
        let link = self.link.lock().unwrap().take().expect("one build");
        let loca = self.loca.clone();
        app.insert_resource(link)
            .insert_resource(SimState::default())
            .insert_resource(Dialogs {
                loca,
                names: self.names.clone(),
                views: HashMap::new(),
                shown: None,
                dirty: true,
            })
            .insert_resource(TalkOnStart(self.talk.clone()))
            .add_systems(Startup, spawn_panel)
            .add_systems(Update, talk_on_start.after(receive))
            .add_systems(
                Update,
                (
                    receive,
                    (sync_characters, sync_items, dialog_input, refresh_panel),
                    talk_click.before(crate::nav::click_to_move),
                    write_back,
                    send_combat,
                    send_tick,
                )
                    .chain(),
            );
    }
}

fn to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::from_array(space::vec3(p))
}

fn to_bg3(v: Vec3) -> [f32; 3] {
    space::vec3(v.to_array())
}

fn send_combat(link: Res<SimLink>, mut outbox: ResMut<CombatOutbox>) {
    for c in outbox.0.drain(..) {
        link.send(Cmd::Combat(c));
    }
}

fn send_tick(time: Res<Time>, link: Res<SimLink>) {
    link.send(Cmd::Tick(time.delta_secs_f64()));
}

pub(crate) fn receive(
    link: Res<SimLink>,
    mut state: ResMut<SimState>,
    mut dialogs: ResMut<Dialogs>,
    mut combat: ResMut<CombatInbox>,
) {
    let rx = link.rx.lock().expect("frame channel");
    while let Ok(frame) = rx.try_recv() {
        dialogs.names.extend(frame.names);
        if let Some(p) = frame.presences {
            state.by_id = p.into_iter().map(|p| (p.id, p)).collect();
        }
        for n in frame.notifications {
            dialogs.apply(n);
        }
        if !frame.combat.is_empty() {
            combat.0.push(frame.combat);
        }
    }
}

impl Dialogs {
    pub(crate) fn character_name<'a>(&'a self, root: &'a CharacterRoot) -> &'a str {
        Guid::parse(&root.id)
            .and_then(|id| self.names.get(&id))
            .map_or(root.name.as_str(), String::as_str)
    }

    fn text(&self, t: &Option<bg3_dialog::model::TextLine>) -> Option<String> {
        t.as_ref()
            .and_then(|t| self.loca.text(&t.handle))
            .map(crate::dialog::clean_markup)
    }

    fn name(&self, id: Guid) -> String {
        self.names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    fn apply(&mut self, n: Notification) {
        self.dirty = true;
        match n {
            Notification::DialogStarted { instance, dialog } => {
                eprintln!("play: dialog {instance} {dialog}");
                let name = dialog
                    .get(..dialog.len().saturating_sub(37))
                    .unwrap_or(&dialog)
                    .to_owned();
                self.views
                    .insert(instance, DialogView { name, ..default() });
                self.shown = Some(instance);
            }
            Notification::DialogLine { instance, line } => {
                let speaker = match (line.speaker, line.actor) {
                    (Some(SPEAKER_NARRATOR), _) => "Narrator".to_owned(),
                    (_, Some(a)) => self.name(bg3_sim::dialog::from_lsf(&a)),
                    _ => String::new(),
                };
                let text = self.text(&line.text);
                let silent = text.is_none();
                let text = text.unwrap_or_else(|| format!("[{}]", line.constructor));
                let v = self.views.entry(instance).or_default();
                v.silent = silent;
                v.speaker = speaker;
                v.text = text;
                v.line = true;
                v.timer = 0.0;
                v.choices.clear();
                self.shown = Some(instance);
            }
            Notification::DialogChoices { instance, choices } => {
                let list: Vec<(String, bool)> = choices
                    .iter()
                    .map(|c| {
                        let mut t = self
                            .text(&c.text)
                            .unwrap_or_else(|| format!("[{}]", c.constructor));
                        if let Some(r) = &c.roll {
                            t = format!("[{} {}] {t}", r.ability, r.skill);
                        }
                        (t, c.enabled)
                    })
                    .collect();
                let v = self.views.entry(instance).or_default();
                v.choices = list;
                v.line = false;
                self.shown = Some(instance);
            }
            Notification::DialogRoll { instance, roll } => {
                let who = roll.roller.map(|r| self.name(r)).unwrap_or_default();
                let text = match &roll.check {
                    Some(c) => format!("{who}: {c}"),
                    None if roll.success => format!("{who}: roll succeeded"),
                    None => format!("{who}: roll failed"),
                };
                eprintln!("play: roll {text}");
                self.views.entry(instance).or_default().rolls.push(text);
            }
            Notification::TradeRequested { instance } => {
                eprintln!("play: dialog {instance} asks to trade (not modelled)");
            }
            Notification::DialogEnded { instance } => {
                self.views.remove(&instance);
                if self.shown == Some(instance) {
                    // Show another running dialog, if any.
                    self.shown = self.views.keys().max().copied();
                }
            }
        }
    }
}

/// Characters follow the sim: visibility (on stage, visible), position (teleports and
/// scripted moves; the controlled avatar only when the sim moved it itself).
#[allow(clippy::type_complexity)]
pub(crate) fn sync_characters(
    time: Res<Time>,
    mut state: ResMut<SimState>,
    mut roots: Query<
        (
            &CharacterRoot,
            &mut Transform,
            &mut Visibility,
            Option<&mut Controlled>,
        ),
        Without<CombatWalk>,
    >,
) {
    let dt = time.delta_secs();
    for (root, mut t, mut vis, controlled) in &mut roots {
        let Some(p) = Guid::parse(&root.id).and_then(|id| state.by_id.get(&id)) else {
            continue;
        };
        let want = if p.on_stage && p.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            if *vis != Visibility::Inherited || want == Visibility::Hidden {
                eprintln!(
                    "stage: {} {} (on stage {}, visible {})",
                    p.name,
                    if want == Visibility::Hidden {
                        "hidden"
                    } else {
                        "shown"
                    },
                    p.on_stage,
                    p.visible
                );
            }
            *vis = want;
        }
        let target = to_bevy(p.position);
        match controlled {
            Some(mut c) => {
                // The sim moved the avatar (a story teleport) if its position is neither where
                // we put it nor where it was.
                if state.sent.is_some_and(|s| s != p.position)
                    && t.translation.distance(target) > 0.5
                {
                    t.translation = target;
                    c.waypoints.clear();
                    c.moving = false;
                    state.sent = Some(p.position);
                }
            }
            None => {
                let d = target - t.translation;
                if d.length() < 1e-3 {
                    continue;
                }
                if p.moving {
                    // Scripted move: glide toward the sim's position, facing the way.
                    let step = (root.run_speed * dt * 1.5).max(d.length() * 0.2);
                    let flat = Vec3::new(d.x, 0.0, d.z);
                    if flat.length() > 1e-3 {
                        t.rotation = Quat::from_rotation_y(flat.x.atan2(flat.z));
                    }
                    t.translation += d.clamp_length_max(step);
                } else {
                    t.translation = target;
                }
            }
        }
    }
}

/// Level items taken off stage (or made invisible) by the story are hidden.
fn sync_items(state: Res<SimState>, mut items: Query<(&ObjectId, &mut Visibility)>) {
    if !state.is_changed() {
        return;
    }
    for (id, mut vis) in &mut items {
        let Some(p) = Guid::parse(&id.0).and_then(|g| state.by_id.get(&g)) else {
            continue;
        };
        let want = if p.on_stage && p.visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
}

/// The avatar's position goes back to the sim when the player moved it.
#[allow(clippy::type_complexity)]
fn write_back(
    link: Res<SimLink>,
    mut state: ResMut<SimState>,
    controlled: Query<(&CharacterRoot, &Transform), (With<Controlled>, Without<CombatWalk>)>,
) {
    for (root, t) in &controlled {
        let Some(id) = Guid::parse(&root.id) else {
            continue;
        };
        if id != link.avatar {
            continue;
        }
        let p = to_bg3(t.translation);
        let moved = state.sent.is_none_or(|s| {
            let d = [s[0] - p[0], s[1] - p[1], s[2] - p[2]];
            d.iter().map(|v| v * v).sum::<f32>() > 0.01 * 0.01
        });
        if moved {
            link.send(Cmd::SetPosition(id, p));
            state.sent = Some(p);
        }
    }
}

/// A left click on a character (other than the avatar) asks to talk to it; one on an item
/// with use actions uses it (`Game::use_item`, e.g. the Brinepool starts its dialog). The
/// click doesn't reach click-to-move.
pub(crate) fn talk_click(
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    link: Res<SimLink>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    roots: Query<(&CharacterRoot, &GlobalTransform, &InheritedVisibility)>,
    items: Query<(&ObjectId, &GlobalTransform, &InheritedVisibility)>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(cursor) = windows.iter().find_map(Window::cursor_position) else {
        return;
    };
    let Some((camera, cam_t)) = cameras.iter().next() else {
        return;
    };
    let mut best: Option<(f32, Cmd, &str)> = None;
    let mut consider = |at: Vec3, cmd: Cmd, name: &'static str| {
        let Ok(screen) = camera.world_to_viewport(cam_t, at) else {
            return;
        };
        let d = screen.distance(cursor);
        if d < PICK_RADIUS_PX && best.as_ref().is_none_or(|b| d < b.0) {
            best = Some((d, cmd, name));
        }
    };
    for (root, t, vis) in &roots {
        let Some(id) = Guid::parse(&root.id) else {
            continue;
        };
        if id == link.avatar || !vis.get() {
            continue;
        }
        consider(t.translation() + Vec3::Y * 1.2, Cmd::Talk(id), "talk to");
    }
    for (obj, t, vis) in &items {
        let Some(id) = Guid::parse(&obj.0).filter(|g| link.usable_items.contains_key(g)) else {
            continue;
        };
        if !vis.get() {
            continue;
        }
        consider(t.translation() + Vec3::Y * 0.5, Cmd::UseItem(id), "use");
    }
    if let Some((_, cmd, what)) = best {
        let id = match &cmd {
            Cmd::Talk(id) | Cmd::UseItem(id) => id.to_string(),
            _ => String::new(),
        };
        eprintln!("play: {what} {id}");
        link.send(cmd);
        buttons.clear_just_pressed(MouseButton::Left);
    }
}

/// `--talk`: one second in and with no dialog running, ask to talk to the named character.
fn talk_on_start(
    time: Res<Time>,
    link: Res<SimLink>,
    dialogs: Res<Dialogs>,
    mut talk: ResMut<TalkOnStart>,
    roots: Query<&CharacterRoot>,
) {
    if talk.0.is_none() || time.elapsed_secs() < 1.0 || !dialogs.views.is_empty() {
        return;
    }
    let name = talk.0.take().unwrap_or_default().to_lowercase();
    match roots
        .iter()
        .find(|r| crate::characters::name_matches(&r.name, &name))
        .and_then(|r| Guid::parse(&r.id).map(|id| (id, r)))
    {
        Some((id, r)) => {
            eprintln!("play: talk to {}", r.name);
            link.send(Cmd::Talk(id));
        }
        None => match link
            .usable_items
            .iter()
            .find(|(_, n)| crate::characters::name_matches(n, &name))
        {
            // Or an item with use actions (as if clicked).
            Some((&id, n)) => {
                eprintln!("play: use {n}");
                link.send(Cmd::UseItem(id));
            }
            None => eprintln!("play: no character or usable item named like {name}"),
        },
    }
}

const DIGITS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// Lines continue after their reading time (Space/Enter skips the shown one); number keys pick
/// a choice of the shown dialog.
fn dialog_input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    link: Res<SimLink>,
    mut dialogs: ResMut<Dialogs>,
) {
    let shown = dialogs.shown;
    let skip = keys.just_pressed(KeyCode::Space) || keys.just_pressed(KeyCode::Enter);
    let digit = DIGITS.iter().position(|k| keys.just_pressed(*k));
    let mut dirty = false;
    for (&id, v) in dialogs.views.iter_mut() {
        if v.line {
            v.timer += time.delta_secs();
            let read = if v.silent {
                SILENT_LINE_SECS
            } else {
                LINE_BASE_SECS + LINE_SECS_PER_CHAR * v.text.chars().count() as f32
            };
            if v.timer > read || (skip && shown == Some(id)) {
                v.line = false;
                link.send(Cmd::Advance(id));
            }
        } else if shown == Some(id)
            && let Some(i) = digit
            && v.choices.get(i).is_some_and(|c| c.1)
        {
            v.choices.clear();
            v.rolls.clear();
            link.send(Cmd::Choose(id, i));
            dirty = true;
        }
    }
    if dirty {
        dialogs.dirty = true;
    }
}

fn spawn_panel(mut commands: Commands) {
    commands.spawn((
        PlayPanel,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(15.0),
            width: Val::Percent(70.0),
            bottom: Val::Px(24.0),
            padding: UiRect::all(Val::Px(16.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.82)),
        Visibility::Hidden,
    ));
}

fn refresh_panel(
    mut commands: Commands,
    mut dialogs: ResMut<Dialogs>,
    mut panel: Query<(Entity, &mut Visibility), With<PlayPanel>>,
) {
    if !dialogs.dirty {
        return;
    }
    let Ok((panel, mut vis)) = panel.single_mut() else {
        return;
    };
    dialogs.dirty = false;
    commands.entity(panel).despawn_related::<Children>();
    let Some(v) = dialogs.shown.and_then(|i| dialogs.views.get(&i)) else {
        *vis = Visibility::Hidden;
        return;
    };
    *vis = Visibility::Inherited;
    let font = |size: f32| TextFont {
        font_size: bevy::text::FontSize::Px(size),
        ..default()
    };
    let gold = TextColor(Color::srgb(0.93, 0.78, 0.45));
    let white = TextColor(Color::srgb(0.92, 0.92, 0.9));
    let grey = TextColor(Color::srgb(0.55, 0.55, 0.55));
    let blue = TextColor(Color::srgb(0.55, 0.75, 0.95));
    commands.entity(panel).with_children(|p| {
        p.spawn((Text::new(v.name.clone()), font(14.0), grey));
        for r in &v.rolls {
            p.spawn((Text::new(r.clone()), font(16.0), blue));
        }
        if v.choices.is_empty() {
            if !v.speaker.is_empty() {
                p.spawn((Text::new(v.speaker.clone()), font(18.0), gold));
            }
            p.spawn((Text::new(v.text.clone()), font(22.0), white));
        } else {
            for (i, (text, enabled)) in v.choices.iter().enumerate() {
                let color = if *enabled { white } else { grey };
                p.spawn((Text::new(format!("{}. {text}", i + 1)), font(20.0), color));
            }
        }
    });
}

/// Resolve from the live sim, including runtime story/character-creation name handles.
fn display_names(sim: &bg3_sim::Sim) -> HashMap<Guid, String> {
    sim.entities
        .iter()
        .map(|e| {
            let name = sim
                .display_name(e.id)
                .unwrap_or_else(|| e.name.trim_start_matches("S_").to_owned());
            (e.id, crate::dialog::clean_markup(&name))
        })
        .collect()
}
