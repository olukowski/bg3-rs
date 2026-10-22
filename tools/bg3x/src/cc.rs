//! `bg3x cc`: character creation headless. Lists the options, or builds a character from
//! the given choices (the rest from the data's defaults), validates it with the game's rules
//! and prints the level-1 sheet. Names are matched against the data's internal names, the
//! display names (English loca) or UUIDs. Output includes game text: keep it local.

use std::sync::Arc;

use anyhow::{Result, bail};
use bg3_install::Install;
use bg3_loca::Loca;
use bg3_rules::context::Ability;
use bg3_sim::Guid;
use bg3_sim::cc::{CcData, CharacterChoice, Pick, SelectorKind};

#[derive(clap::Args)]
pub struct Args {
    /// List races, classes, backgrounds and deities (and the choices of the character).
    #[arg(long)]
    pub list: bool,
    /// Start from a `--character` spec (as `--spec` prints it).
    #[arg(long)]
    pub choice: Option<String>,
    #[arg(long)]
    pub race: Option<String>,
    #[arg(long)]
    pub subrace: Option<String>,
    /// Body type: 0 (or `m`) masculine, 1 (or `f`) feminine.
    #[arg(long)]
    pub body: Option<String>,
    /// Body shape: 0 regular, 1 strong.
    #[arg(long)]
    pub shape: Option<u8>,
    #[arg(long)]
    pub class: Option<String>,
    #[arg(long)]
    pub subclass: Option<String>,
    #[arg(long)]
    pub background: Option<String>,
    #[arg(long)]
    pub deity: Option<String>,
    /// Point-buy scores before bonuses: Str,Dex,Con,Int,Wis,Cha.
    #[arg(long, value_delimiter = ',')]
    pub abilities: Vec<i32>,
    /// Ability bonus targets, `+2` first (e.g. `Wis,Con`).
    #[arg(long, value_delimiter = ',')]
    pub bonus: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub skills: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub expertise: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub spells: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub passives: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub equipment: Vec<String>,
    /// Appearance UUID or case-insensitive display-name substring (see --list).
    #[arg(long)]
    pub head: Option<String>,
    #[arg(long)]
    pub hair: Option<String>,
    #[arg(long)]
    pub beard: Option<String>,
    #[arg(long)]
    pub skin: Option<String>,
    #[arg(long)]
    pub hair_color: Option<String>,
    #[arg(long)]
    pub eye_color: Option<String>,
    /// Print the choice as a `bg3-viewer --character` spec.
    #[arg(long)]
    pub spec: bool,
}

struct Names<'a> {
    loca: &'a Loca,
}

impl Names<'_> {
    fn text(&self, handle: Option<&String>, fallback: &str) -> String {
        handle
            .and_then(|h| self.loca.text(h))
            .map(str::to_owned)
            .unwrap_or_else(|| fallback.to_owned())
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The first candidate whose UUID, internal name or display name matches.
fn find<'a, T>(
    what: &str,
    query: &str,
    items: impl IntoIterator<Item = (&'a T, Guid, Vec<String>)>,
) -> Result<Guid>
where
    T: 'a,
{
    if let Some(g) = Guid::parse(query.trim()) {
        return Ok(g);
    }
    let q = norm(query);
    let items: Vec<_> = items.into_iter().collect();
    for (_, id, names) in &items {
        if names.iter().any(|n| norm(n) == q) {
            return Ok(*id);
        }
    }
    let known: Vec<String> = items
        .iter()
        .filter_map(|(_, _, n)| n.last().cloned())
        .collect();
    bail!("no {what} named {query:?} (known: {})", known.join(", "))
}

/// Prefer exact display names/entry UUIDs, then resource UUIDs and display substrings.
fn find_appearance(
    what: &str,
    query: &str,
    options: impl IntoIterator<Item = (Guid, Guid, String)>,
) -> Result<Guid> {
    let options: Vec<_> = options.into_iter().collect();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        bail!("empty {what} query");
    }
    let id = Guid::parse(&q);
    options
        .iter()
        .find(|(entry, _, name)| Some(*entry) == id || name.to_lowercase() == q)
        .or_else(|| {
            options.iter().find(|(_, resource, name)| {
                Some(*resource) == id || (id.is_none() && name.to_lowercase().contains(&q))
            })
        })
        .map(|(id, _, _)| *id)
        .ok_or_else(|| anyhow::anyhow!("no allowed {what} matching {query:?}; see cc --list"))
}

fn ability(name: &str) -> Result<Ability> {
    let q = norm(name);
    Ability::ALL
        .iter()
        .find(|a| norm(a.name()).starts_with(&q) && q.len() >= 3)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("no ability {name:?}"))
}

pub fn run(a: Args) -> Result<()> {
    let install = Install::locate()?;
    let vfs = Arc::new(bg3_vfs::Vfs::from_install(&install)?);
    let loca = crate::dialog::load_loca(&vfs, "English")?;
    let stats = bg3_stats::install::load(vfs.packages(), bg3_vfs::modules::BASE_LOAD_ORDER)?;
    let cc = CcData::load(&vfs, &stats)?;
    let names = Names { loca: &loca };

    let mut choice = match &a.choice {
        Some(s) => CharacterChoice::from_spec(s).map_err(anyhow::Error::msg)?,
        None => bg3_sim::start::trace_choice(),
    };
    let race_names =
        |r: &bg3_sim::cc::CcRace| vec![r.name.clone(), names.text(r.display.as_ref(), &r.name)];
    if let Some(q) = &a.race {
        choice.race = find(
            "race",
            q,
            cc.playable_races()
                .into_iter()
                .map(|r| (r, r.id, race_names(r))),
        )?;
        choice.subrace = None;
    }
    if let Some(q) = &a.subrace {
        choice.subrace = Some(find(
            "subrace",
            q,
            cc.subraces(choice.race)
                .into_iter()
                .map(|r| (r, r.id, race_names(r))),
        )?);
    }
    if let Some(b) = &a.body {
        choice.body_type = match b.as_str() {
            "0" | "m" | "masculine" | "male" => 0,
            "1" | "f" | "feminine" | "female" => 1,
            _ => bail!("--body: 0/m or 1/f"),
        };
    }
    if let Some(s) = a.shape {
        choice.body_shape = s;
    }
    let class_names =
        |c: &bg3_sim::cc::CcClass| vec![c.name.clone(), names.text(c.display.as_ref(), &c.name)];
    if let Some(q) = &a.class {
        choice.class = find(
            "class",
            q,
            cc.playable_classes()
                .into_iter()
                .map(|c| (c, c.id, class_names(c))),
        )?;
        choice.subclass = None;
        choice.deity = None;
        choice.abilities = [0; 6];
        choice.picks.clear();
    }
    if let Some(q) = &a.subclass {
        let subs: Vec<&bg3_sim::cc::CcClass> = cc
            .level1_subclasses(choice.class)
            .into_iter()
            .filter_map(|s| cc.class(s))
            .collect();
        choice.subclass = Some(find(
            "subclass",
            q,
            subs.into_iter().map(|c| (c, c.id, class_names(c))),
        )?);
    }
    if let Some(q) = &a.background {
        choice.background = find(
            "background",
            q,
            cc.playable_backgrounds().into_iter().map(|b| {
                let passive = b.passives.first().cloned().unwrap_or_default();
                let short = passive
                    .strip_prefix("Background_")
                    .unwrap_or(&passive)
                    .to_owned();
                (
                    b,
                    b.id,
                    vec![passive, short, names.text(b.display.as_ref(), "")],
                )
            }),
        )?;
    }
    if let Some(q) = &a.deity {
        choice.deity = Some(find(
            "deity",
            q,
            cc.allowed_gods(choice.race, choice.subrace)
                .into_iter()
                .map(|g| {
                    (
                        g,
                        g.id,
                        vec![g.name.clone(), names.text(g.display.as_ref(), &g.name)],
                    )
                }),
        )?);
    }
    if a.race.is_some() && a.deity.is_none() && choice.deity.is_some() {
        // A new race may exclude the old deity; let the defaults pick again.
        let ok = cc
            .allowed_gods(choice.race, choice.subrace)
            .iter()
            .any(|g| Some(g.id) == choice.deity);
        if !ok {
            choice.deity = None;
        }
    }
    if !a.abilities.is_empty() {
        choice.abilities = a
            .abilities
            .clone()
            .try_into()
            .map_err(|_| anyhow::anyhow!("--abilities: six scores"))?;
    }
    if choice.subrace.is_none() {
        choice.subrace = cc.subraces(choice.race).first().map(|r| r.id);
    }
    if choice.subclass.is_none() {
        choice.subclass = cc.level1_subclasses(choice.class).first().copied();
    }
    for (slot, query) in [("Head", &a.head), ("Hair", &a.hair), ("Beard", &a.beard)] {
        if let Some(query) = query {
            let options = cc.appearance_options(&choice, slot);
            let id = find_appearance(
                slot,
                query,
                options.iter().map(|v| {
                    (
                        v.id,
                        v.visual,
                        names.text(v.display.as_ref(), &v.id.to_string()),
                    )
                }),
            )?;
            choice.appearance.set_visual(slot, Some(id));
        }
    }
    for (kind, query) in [
        ("Skin", &a.skin),
        ("Hair", &a.hair_color),
        ("Eye", &a.eye_color),
    ] {
        if let Some(query) = query {
            let id = find_appearance(
                kind,
                query,
                cc.appearance
                    .material_options(kind)
                    .iter()
                    .map(|m| (m.id, m.preset, names.text(m.display.as_ref(), &m.name))),
            )?;
            choice.appearance.set_material(kind, Some(id));
        }
    }
    // Picks from the flags, distributed over the selectors of each kind in order.
    let lists: [(&[SelectorKind], Vec<String>); 5] = [
        (&[SelectorKind::SelectSkills], a.skills.clone()),
        (&[SelectorKind::SelectSkillsExpertise], a.expertise.clone()),
        (&[SelectorKind::SelectSpells], a.spells.clone()),
        (&[SelectorKind::SelectPassives], a.passives.clone()),
        (&[SelectorKind::SelectEquipment], a.equipment.clone()),
    ];
    let slots = cc.slots(&choice);
    for (kinds, wanted) in lists {
        if wanted.is_empty() {
            continue;
        }
        let mut rest: Vec<String> = wanted;
        for slot in slots.iter().filter(|s| kinds.contains(&s.selector.kind)) {
            let mut items = Vec::new();
            rest.retain(|w| {
                let hit = items.len() < slot.selector.count
                    && slot
                        .options
                        .iter()
                        .find(|o| norm(o) == norm(w))
                        .is_some_and(|o| {
                            items.push(o.clone());
                            true
                        });
                !hit
            });
            set_pick(&mut choice, slot.progression, slot.index, items);
        }
        if !rest.is_empty() {
            bail!("not an option of any selector: {}", rest.join(", "));
        }
    }
    if !a.bonus.is_empty() {
        let items: Vec<String> = a
            .bonus
            .iter()
            .map(|b| ability(b).map(|x| x.name().to_owned()))
            .collect::<Result<_>>()?;
        let slot = slots
            .iter()
            .find(|s| s.selector.kind == SelectorKind::SelectAbilityBonus)
            .ok_or_else(|| anyhow::anyhow!("the class has no ability bonus selector"))?;
        set_pick(&mut choice, slot.progression, slot.index, items);
    }
    cc.complete(&mut choice);

    if a.list {
        list(&cc, &names, &choice);
    }
    if a.spec {
        println!("{}", choice.to_spec());
    }
    match cc.build(&choice) {
        Ok(c) => {
            let templates = bg3_sim::load::load_templates(&vfs)?;
            let template_stats = bg3_sim::GameData {
                templates,
                ..Default::default()
            }
            .template_chain(Some(c.template))
            .iter()
            .find_map(|t| t.stats.clone());
            let extra = Extra {
                template_stats,
                level_maps: bg3_rules::levelmap::LevelMaps::load(
                    &vfs,
                    bg3_vfs::modules::BASE_LOAD_ORDER,
                ),
                tags: bg3_sim::load::load_tags(&vfs)?,
            };
            print_sheet(&cc, &names, &stats, &extra, &c)
        }
        Err(errs) => {
            for e in &errs {
                eprintln!("invalid: {e}");
            }
            bail!("{} rule(s) broken", errs.len());
        }
    }
    Ok(())
}

fn set_pick(choice: &mut CharacterChoice, progression: Guid, index: usize, items: Vec<String>) {
    choice
        .picks
        .retain(|p| !(p.progression == progression && p.index == index));
    choice.picks.push(Pick {
        progression,
        index,
        items,
    });
}

fn list(cc: &CcData, names: &Names<'_>, choice: &CharacterChoice) {
    println!("Races:");
    for r in cc.playable_races() {
        let subs: Vec<String> = cc
            .subraces(r.id)
            .iter()
            .map(|s| format!("{} [{}]", names.text(s.display.as_ref(), &s.name), s.name))
            .collect();
        println!(
            "  {} [{}]{}",
            names.text(r.display.as_ref(), &r.name),
            r.name,
            if subs.is_empty() {
                String::new()
            } else {
                format!(": {}", subs.join(", "))
            }
        );
    }
    println!("Classes:");
    for c in cc.playable_classes() {
        let subs: Vec<String> = cc
            .level1_subclasses(c.id)
            .iter()
            .filter_map(|s| cc.class(*s))
            .map(|s| names.text(s.display.as_ref(), &s.name))
            .collect();
        println!(
            "  {} [{}] (HP {} + Con){}",
            names.text(c.display.as_ref(), &c.name),
            c.name,
            c.base_hp,
            if subs.is_empty() {
                String::new()
            } else {
                format!(", level-1 subclass: {}", subs.join(", "))
            }
        );
    }
    println!("Backgrounds:");
    for b in cc.playable_backgrounds() {
        println!(
            "  {} [{}]",
            names.text(b.display.as_ref(), ""),
            b.passives.join(", ")
        );
    }
    println!("Deities (for this race):");
    for g in cc.allowed_gods(choice.race, choice.subrace) {
        println!("  {} [{}]", names.text(g.display.as_ref(), &g.name), g.name);
    }
    println!("Appearance (for this race/body; unspecified keeps template):");
    for slot in cc.appearance_slots(choice) {
        println!("  {slot}:");
        for v in cc.appearance_options(choice, &slot) {
            println!(
                "    {} [{}] visual={}",
                names.text(v.display.as_ref(), &v.id.to_string()),
                v.id,
                v.visual
            );
        }
    }
    for kind in cc.appearance.material_kinds() {
        println!("  {kind} materials:");
        for m in cc.appearance.material_options(&kind) {
            println!(
                "    {} [{}] preset={} {}",
                names.text(m.display.as_ref(), &m.name),
                m.id,
                m.preset,
                m.ui_color.as_deref().unwrap_or("")
            );
        }
    }
    println!("Choices of this character:");
    for s in cc.slots(choice) {
        let display: Vec<String> = s
            .options
            .iter()
            .map(|o| names.text(cc.display_names.get(o), o))
            .collect();
        println!("  {}: {}", s.label(), display.join(", "));
    }
}

/// What the sheet needs besides the character-creation data.
struct Extra {
    template_stats: Option<String>,
    level_maps: bg3_rules::levelmap::LevelMaps,
    tags: bg3_sim::TagDefs,
}

fn print_sheet(
    cc: &CcData,
    names: &Names<'_>,
    stats: &bg3_stats::Stats,
    extra: &Extra,
    c: &bg3_sim::cc::CreatedCharacter,
) {
    let ch = &c.choice;
    let race = |g: Option<Guid>| {
        g.and_then(|g| cc.race(g))
            .map(|r| names.text(r.display.as_ref(), &r.name))
    };
    let class = |g: Option<Guid>| {
        g.and_then(|g| cc.class(g))
            .map(|r| names.text(r.display.as_ref(), &r.name))
    };
    let bg = cc
        .background(ch.background)
        .map(|b| names.text(b.display.as_ref(), ""))
        .unwrap_or_default();
    let god = ch
        .deity
        .and_then(|g| cc.god(g))
        .map(|g| names.text(g.display.as_ref(), &g.name));
    println!(
        "{} {}, {}{}{}, body {}/{}",
        race(ch.subrace).unwrap_or_default(),
        race(Some(ch.race)).unwrap_or_default(),
        class(Some(ch.class)).unwrap_or_default(),
        class(ch.subclass)
            .map(|s| format!(" ({s})"))
            .unwrap_or_default(),
        god.map(|g| format!(" of {g}")).unwrap_or_default(),
        ch.body_type,
        ch.body_shape,
    );
    println!(
        "background {bg}; template {}; point buy {} of {}",
        c.template,
        bg3_sim::cc::point_buy_spent(&ch.abilities).unwrap_or(0),
        bg3_sim::cc::POINT_BUY_POINTS
    );
    // The template's `Character` stats entry: proficiency bonus scaling (the sim's sheet
    // uses it).
    let def = extra
        .template_stats
        .as_deref()
        .and_then(|s| bg3_sim::character::character_defs(stats).remove(s));
    let proficiency = def
        .as_ref()
        .map_or(2, |d| d.proficiency_at(c.level, &extra.level_maps));
    println!(
        "level {}, HP {}, AC {}, proficiency +{proficiency}; stats entry {}",
        c.level,
        c.max_hp,
        c.armor_class,
        extra.template_stats.as_deref().unwrap_or("?")
    );
    let scores: Vec<String> = Ability::ALL
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let v = c.abilities[i];
            format!("{} {v} ({:+})", &a.name()[..3], (v - 10).div_euclid(2))
        })
        .collect();
    println!("{}", scores.join("  "));
    let join = |v: Vec<String>| {
        if v.is_empty() {
            "-".to_owned()
        } else {
            v.join(", ")
        }
    };
    println!(
        "saving throws: {}",
        join(c.saving_throws.iter().map(|a| a.to_string()).collect())
    );
    println!(
        "skills: {}",
        join(c.skills.iter().map(|a| a.to_string()).collect())
    );
    println!(
        "expertise: {}",
        join(c.expertise.iter().map(|a| a.to_string()).collect())
    );
    println!("proficiencies: {}", join(c.proficiencies.clone()));
    if let Some(a) = c.spellcasting_ability {
        println!("spellcasting ability: {a}");
    }
    let named = |v: &[String]| {
        join(
            v.iter()
                .map(
                    |s| match cc.display_names.get(s).and_then(|h| names.loca.text(h)) {
                        Some(t) => format!("{t} [{s}]"),
                        None => s.clone(),
                    },
                )
                .collect(),
        )
    };
    println!("spells: {}", named(&c.spells));
    println!("passives: {}", named(&c.passives));
    println!("equipment: {}", join(c.equipment.clone()));
    println!(
        "tags: {}",
        join(
            c.tags
                .iter()
                .map(|t| {
                    extra
                        .tags
                        .get(*t)
                        .map_or_else(|| t.to_string(), |d| d.name.clone())
                })
                .collect()
        )
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_names_match_substrings_and_reject_unavailable_uuids() {
        let options = || vec![(Guid(1), Guid(2), "Synthetic Hairstyle".into())];
        assert_eq!(
            find_appearance("Hair", "HaIrStY", options()).unwrap(),
            Guid(1)
        );
        assert_eq!(
            find_appearance("Hair", &Guid(2).to_string(), options()).unwrap(),
            Guid(1)
        );
        assert!(find_appearance("Hair", &Guid(3).to_string(), options()).is_err());
        assert!(find_appearance("Hair", "", options()).is_err());
    }
}
