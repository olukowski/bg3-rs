//! A minimal dialog panel (`--dialog <name>`): runs a dialog with `bg3-dialog`'s runner and
//! shows the speaker, the current line and numbered choices. Number keys pick a choice;
//! Space/Enter skips a line (lines also continue on their own after a reading delay).
//! Flags live in memory for the session; rolls succeed. Osiris events the dialog raises are
//! printed to stderr.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use bevy::prelude::*;
use bg3_dialog::defs::Definitions;
use bg3_dialog::library::DialogLibrary;
use bg3_dialog::model::SPEAKER_NARRATOR;
use bg3_dialog::runner::placeholder_speakers;
use bg3_dialog::{DialogRunner, Guid, MemoryWorld, StartParams, Step};
use bg3_install::Install;
use bg3_loca::Loca;
use bg3_lsf::Value;
use bg3_vfs::Vfs;

/// Seconds a line stays up before the runner continues: a base plus a per-character reading
/// time (our presentation choice; the game waits for voice/timeline playback).
const LINE_BASE_SECS: f32 = 2.5;
const LINE_SECS_PER_CHAR: f32 = 0.05;

pub struct DialogPlugin {
    session: Mutex<Option<DialogSession>>,
}

#[derive(Resource)]
struct DialogSession {
    runner: DialogRunner,
    world: MemoryWorld,
    loca: Loca,
    defs: Definitions,
    /// Display name per speaker slot.
    speaker_names: Vec<String>,
    /// Osiris-style names for event printing.
    actor_names: HashMap<Guid, String>,
    dirty: bool,
    line_timer: f32,
}

#[derive(Component)]
struct DialogPanel;

impl DialogPlugin {
    /// Loads the dialog (by resource name or Osiris `Name_uuid`), loca and definitions.
    /// `level` is used to name the speakers from the level's characters.
    pub fn load(install: &Install, name: &str, level: &str) -> Result<Self> {
        let vfs = Arc::new(Vfs::from_install(install)?);
        let lib = Arc::new(DialogLibrary::new(vfs.clone())?);
        let resource = lib
            .find(name)
            .ok_or_else(|| anyhow!("no dialog resource named {name}"))?
            .clone();
        let dialog = lib.load_resource(&resource.id)?;
        let defs = Definitions::load(&vfs)?;
        let loca = load_loca(&vfs, "English")?;
        let characters = level_characters(&vfs, level);

        let speakers = placeholder_speakers(&dialog);
        let mut speaker_names = Vec::new();
        let mut actor_names = HashMap::new();
        for (slot, actor) in speakers.iter().enumerate() {
            let entry = dialog.speaker(slot as i32);
            let label = entry
                .and_then(|s| s.list.first())
                .map(|g| speaker_label(g, &defs, &characters, &loca))
                .unwrap_or_else(|| format!("Speaker {slot}"));
            if let Some(a) = actor {
                actor_names.insert(*a, format!("SPEAKER_{slot}"));
            }
            speaker_names.push(label);
        }
        eprintln!(
            "dialog {} ({} nodes, {} speakers)",
            resource.osiris_name(),
            dialog.nodes.len(),
            speakers.len()
        );
        let mut world = MemoryWorld::with_definitions(&defs);
        let mut runner = DialogRunner::new(
            dialog,
            lib,
            StartParams {
                instance: 1,
                speakers,
            },
        );
        runner.start(&mut world);
        Ok(DialogPlugin {
            session: Mutex::new(Some(DialogSession {
                runner,
                world,
                loca,
                defs,
                speaker_names,
                actor_names,
                dirty: true,
                line_timer: 0.0,
            })),
        })
    }
}

impl Plugin for DialogPlugin {
    fn build(&self, app: &mut App) {
        if let Some(session) = self.session.lock().unwrap().take() {
            app.insert_resource(session)
                .add_systems(Startup, spawn_panel)
                .add_systems(Update, (dialog_input, refresh_panel).chain());
        }
    }
}

fn load_loca(vfs: &Vfs, language: &str) -> Result<Loca> {
    let mut loca = Loca::new();
    let mut files: Vec<_> = vfs
        .files_under(&format!("Localization/{language}/"))
        .filter(|f| f.entry.name.to_ascii_lowercase().ends_with(".loca"))
        .collect();
    files.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    for f in files {
        loca.merge_bytes(&f.read()?)?;
    }
    Ok(loca)
}

/// Level characters and items (`Mods/*/Levels/<level>/{Characters,Items}` and every
/// `Mods/*/Globals/*/{Characters,Items}`; speakers can be items, e.g. speaker helpers) by UUID: (internal name, display-name loca handle).
fn level_characters(vfs: &Vfs, level: &str) -> HashMap<Guid, (String, Option<String>)> {
    let level = level.to_ascii_lowercase();
    let mut out = HashMap::new();
    for f in vfs.files_under("mods/") {
        let p = f.entry.name.to_ascii_lowercase();
        let wanted = (p.ends_with("/characters/_merged.lsf") || p.ends_with("/items/_merged.lsf"))
            && (p.contains("/globals/") || p.contains(&format!("/levels/{level}/")));
        if !wanted {
            continue;
        }
        let Ok(res) = f
            .read()
            .map_err(anyhow::Error::from)
            .and_then(|d| Ok(bg3_lsf::parse(&d)?))
        else {
            continue;
        };
        for region in &res.regions {
            for obj in region.node.children_named("GameObjects") {
                let Some(id) = obj.attr_str("MapKey").and_then(Guid::parse) else {
                    continue;
                };
                let name = obj.attr_str("Name").unwrap_or_default().to_owned();
                let display = match obj.attr("DisplayName") {
                    Some(Value::TranslatedString(t)) if !t.handle.is_empty() => {
                        Some(t.handle.clone())
                    }
                    _ => None,
                };
                out.insert(id, (name, display));
            }
        }
    }
    out
}

fn speaker_label(
    id: &Guid,
    defs: &Definitions,
    characters: &HashMap<Guid, (String, Option<String>)>,
    loca: &Loca,
) -> String {
    if let Some(g) = defs.speaker_groups.get(id) {
        return match g.name.as_str() {
            "GROUP_Players" => "Player".to_owned(),
            other => other.trim_start_matches("GROUP_").to_owned(),
        };
    }
    match characters.get(id) {
        Some((name, display)) => display
            .as_deref()
            .and_then(|h| loca.text(h))
            .map(str::to_owned)
            .unwrap_or_else(|| name.trim_start_matches("S_").to_owned()),
        None => id.to_string(),
    }
}

fn spawn_panel(mut commands: Commands) {
    commands.spawn((
        DialogPanel,
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
    ));
}

fn dialog_input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut session: ResMut<DialogSession>,
) {
    let s = &mut *session;
    match s.runner.step().clone() {
        Step::Line(line) => {
            let chars = line
                .text
                .as_ref()
                .and_then(|t| s.loca.text(&t.handle))
                .map_or(0, |t| t.chars().count());
            s.line_timer += time.delta_secs();
            let skip = keys.just_pressed(KeyCode::Space) || keys.just_pressed(KeyCode::Enter);
            if skip || s.line_timer > LINE_BASE_SECS + LINE_SECS_PER_CHAR * chars as f32 {
                let _ = s.runner.advance(&mut s.world);
                s.line_timer = 0.0;
                s.dirty = true;
            }
        }
        Step::Choices(options) => {
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
            if let Some(i) = DIGITS.iter().position(|k| keys.just_pressed(*k))
                && i < options.len()
                && s.runner.choose(&mut s.world, i).is_ok()
            {
                s.line_timer = 0.0;
                s.dirty = true;
            }
        }
        Step::Ended => {}
    }
    let events = s.runner.take_events();
    if !events.is_empty() {
        let names = |g: &Guid| match s.actor_names.get(g) {
            Some(n) => n.clone(),
            None => match s.defs.flags.get(g) {
                Some(f) => f.osiris_name(),
                None => g.to_string(),
            },
        };
        for e in events {
            for line in e.osiris(&names) {
                eprintln!("[dialog] {line}");
            }
        }
    }
}

fn refresh_panel(
    mut commands: Commands,
    mut session: ResMut<DialogSession>,
    panel: Query<Entity, With<DialogPanel>>,
) {
    if !session.dirty {
        return;
    }
    let Ok(panel) = panel.single() else {
        return;
    };
    session.dirty = false;
    let s = &*session;
    let text_of = |handle: Option<&str>, fallback: &str| -> String {
        handle
            .and_then(|h| s.loca.text(h))
            .map(clean_markup)
            .unwrap_or_else(|| fallback.to_owned())
    };
    let speaker_name = |speaker: Option<i32>| match speaker {
        Some(SPEAKER_NARRATOR) => "Narrator".to_owned(),
        Some(i) if i >= 0 => s
            .speaker_names
            .get(i as usize)
            .cloned()
            .unwrap_or_else(|| format!("Speaker {i}")),
        _ => String::new(),
    };
    let font = |size: f32| TextFont {
        font_size: bevy::text::FontSize::Px(size),
        ..default()
    };
    let gold = TextColor(Color::srgb(0.93, 0.78, 0.45));
    let white = TextColor(Color::srgb(0.92, 0.92, 0.9));
    let grey = TextColor(Color::srgb(0.55, 0.55, 0.55));

    commands.entity(panel).despawn_related::<Children>();
    commands
        .entity(panel)
        .with_children(|p| match s.runner.step() {
            Step::Line(line) => {
                p.spawn((Text::new(speaker_name(line.speaker)), font(18.0), gold));
                let fallback = format!("[{}]", line.constructor);
                let text = text_of(line.text.as_ref().map(|t| t.handle.as_str()), &fallback);
                p.spawn((Text::new(text), font(22.0), white));
            }
            Step::Choices(options) => {
                for (i, c) in options.iter().enumerate() {
                    let fallback = format!("[{}]", c.constructor);
                    let mut text = text_of(c.text.as_ref().map(|t| t.handle.as_str()), &fallback);
                    if let Some(r) = &c.roll {
                        text = format!("[{} {}] {text}", r.ability, r.skill);
                    }
                    let color = if c.enabled { white } else { grey };
                    p.spawn((Text::new(format!("{}. {text}", i + 1)), font(20.0), color));
                }
            }
            Step::Ended => {
                p.spawn((Text::new("(dialog ended)"), font(18.0), grey));
            }
        });
}

/// Drops loca markup the panel can't render (`<i>`, `<br>`, `<LSTag …>`).
fn clean_markup(text: &str) -> String {
    let text = text.replace("<br>", "\n");
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}
