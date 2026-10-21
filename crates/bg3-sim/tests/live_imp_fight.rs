//! A live game reaches the Nautiloid's imp fight the way the game does: the avatar enters
//! the lower deck (`S_TUT_LowerDeck_GuideDialogTrigger`), the story runs Lae'zel's dialog
//! (`TUT_LowerDeck_AfterImpFight`, the root for `REALLY_LAEZEL`), the recruiting answer
//! (`OriginAddToParty`) makes her a party member (`MakePlayer`, `RegisterAsCompanion`), the
//! story teleports the party into the imp room (`IsInTrigger`), and the imps notice the
//! party ([`bg3_sim::Game::start_combat_on_sight`]). The reference combat trace has the
//! same fight: the avatar and Lae'zel sharing the first turn, then three imps.
//! Needs the game install; skips cleanly without it.

use std::sync::Arc;

use bg3_sim::Notification;
use bg3_sim::start::{load_game, new_game};

#[test]
#[ignore = "needs BG3 install"]
fn walking_to_the_lower_deck_starts_the_imp_fight() {
    let vfs = match bg3_install::Install::locate() {
        Ok(i) => Arc::new(bg3_vfs::Vfs::from_install(&i).expect("vfs")),
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || run(vfs))
        .expect("spawn")
        .join()
        .expect("game thread");
}

fn run(vfs: Arc<bg3_vfs::Vfs>) {
    let (mut game, opts) = load_game(vfs).expect("load");
    let started = new_game(&mut game, &opts);
    let avatar = started.avatar;
    let find = |game: &bg3_sim::Game, name: &str| {
        game.sim
            .entities
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.id)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let laezel = find(&game, "S_Player_Laezel");
    let trigger = opts
        .startup_triggers
        .iter()
        .find(|t| t.name == "S_TUT_LowerDeck_GuideDialogTrigger")
        .expect("guide trigger")
        .position;
    let mut fight = None;
    let mut dialogs = Vec::new();
    for step in 0..400 {
        // The opening's dialogs first; then walk in (a teleport stands in for the walk).
        if step == 40 {
            game.sim.set_position(avatar, trigger);
        }
        game.tick_live(0.1);
        if fight.is_none() {
            fight = game.start_combat_on_sight();
        }
        for n in game.take_notifications() {
            match n {
                Notification::DialogStarted { dialog, .. } => dialogs.push(dialog),
                Notification::DialogLine { instance, .. } => {
                    game.dialog_advance(instance).ok();
                }
                // The last answer of each choice: in the guide dialog, "What do you
                // suggest?", which recruits her.
                Notification::DialogChoices { instance, choices } => {
                    let i = choices.iter().rposition(|c| c.enabled).unwrap_or(0);
                    game.dialog_choose(instance, i).ok();
                }
                _ => {}
            }
        }
        if fight.is_some() {
            break;
        }
    }
    assert!(
        dialogs
            .iter()
            .any(|d| d.starts_with("TUT_LowerDeck_AfterImpFight_")),
        "{dialogs:?}"
    );
    assert!(game.sim.party.contains(&laezel), "Lae'zel joined");
    let cid = fight.expect("the imps start the fight");
    let c = game.sim.combat_by_id(cid).expect("combat");
    let mut first = c.order[0].members.clone();
    first.sort_by_key(|g| g.0);
    let mut party = vec![avatar, laezel];
    party.sort_by_key(|g| g.0);
    assert_eq!(first, party, "the party acts first, together");
    let imps: Vec<String> = c.order[1..]
        .iter()
        .flat_map(|g| g.members.iter().map(|m| game.sim.render(*m)))
        .collect();
    assert_eq!(imps.len(), 3, "{imps:?}");
    assert!(imps.iter().all(|i| i.starts_with("S_FirstFight_Imp_")));
    assert!(
        !imps.iter().any(|i| i.contains("Ranged_02")),
        "kept off stage"
    );
}
