//! What the Logi overlay shows: one line per shown pilot, built from the
//! same state as the cards. The drawing is in `crate::overlay::logi_lines`.

use super::config::{CicSettings, LogiCharacter};
use super::logi::{Arrow, PilotState};
use crate::overlay::logi_lines::{ChainLinks, LinkArrow, LogiLine, LogiOverlay, Tackle};

/// The overlay as it should look at `now`, or `None` while it is off.
/// `pilots` gives each shown character's card state, if there is one yet.
pub fn overlay<'a>(
    settings: &CicSettings,
    pilots: impl Fn(&str) -> Option<&'a PilotState>,
    now: f64,
    move_mode: bool,
) -> Option<LogiOverlay> {
    let overlay = &settings.overlay;
    if !overlay.enabled {
        return None;
    }
    let empty = PilotState::default();
    let lines = settings
        .shown()
        .map(|character| {
            line(
                character,
                pilots(&character.name).unwrap_or(&empty),
                now,
                settings,
            )
        })
        .filter(|line| !overlay.only_busy || line.is_busy())
        .collect();
    Some(LogiOverlay {
        lines,
        position: overlay.position,
        width_px: overlay.width_px,
        show_names: overlay.show_names,
        // Percent (0-100) to 0-255.
        background_alpha: (overlay.background_opacity.clamp(0.0, 100.0) * 2.55).round() as u8,
        move_mode,
    })
}

/// One pilot's line, the same badges as the card.
fn line(
    character: &LogiCharacter,
    pilot: &PilotState,
    now: f64,
    settings: &CicSettings,
) -> LogiLine {
    let links = |outgoing: bool| -> Vec<LinkArrow> {
        pilot
            .links(outgoing, now, settings)
            .iter()
            .map(|link| match link.arrow {
                Arrow::Up => LinkArrow::Up,
                Arrow::Down => LinkArrow::Down,
                Arrow::Unknown => LinkArrow::Both,
                Arrow::Other => LinkArrow::Unknown,
            })
            .collect()
    };
    let chain = character.ship.has_cap_chain().then(|| ChainLinks {
        incoming: links(false),
        outgoing: links(true),
    });
    // Blinks like the card's DMG badge: bright in the first half of each
    // second.
    let damage = pilot.hit(now).map(|_| (now * 2.0).floor() as i64 % 2 == 0);
    let ecm = pilot
        .ecm_left(now, settings)
        .map(|(left, _)| (left.ceil() as u32, settings.ecm_secs));
    let tackle = if pilot.tackle(true, now).is_some() {
        Some(Tackle::Scram)
    } else if pilot.tackle(false, now).is_some() {
        Some(Tackle::Point)
    } else {
        None
    };
    LogiLine {
        name: character.name.clone(),
        shield: character.ship.is_shield(),
        chain,
        damage,
        ecm,
        tackle,
    }
}

/// When the overlay next changes by itself (a blink, a countdown, a badge
/// or chain link running out), or `None`.
pub fn next_change<'a>(
    settings: &CicSettings,
    pilots: impl Fn(&str) -> Option<&'a PilotState>,
    now: f64,
) -> Option<f64> {
    if !settings.overlay.enabled {
        return None;
    }
    settings
        .shown()
        .filter_map(|c| pilots(&c.name)?.next_badge_change(now, settings))
        .reduce(f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cic::combatlog::{CombatEvent, Other, RepKind};
    use crate::modules::cic::config::Ship;

    fn settings() -> CicSettings {
        let mut s = CicSettings::default();
        s.overlay.enabled = true;
        s.add_characters(&["Aria".to_string(), "Bren".to_string(), "Hidden".to_string()]);
        s.characters[0].show = true;
        s.characters[1].show = true;
        s.characters[1].ship = Ship::Scimitar;
        s
    }

    fn cap_in(module: &str) -> CombatEvent {
        CombatEvent::Rep {
            kind: RepKind::Cap,
            outgoing: false,
            amount: 351,
            other: Other {
                name: "Cora".into(),
                ship: "Basilisk".into(),
            },
            module: module.into(),
        }
    }

    #[test]
    fn lines_follow_the_shown_characters() {
        let s = settings();
        let o = overlay(&s, |_| None, 0.0, false).unwrap();
        let names: Vec<&str> = o.lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Aria", "Bren"]);
        // The Basilisk has a cap chain (still empty), the Scimitar none.
        assert_eq!(o.lines[0].chain, Some(ChainLinks::default()));
        assert_eq!(o.lines[1].chain, None);
        assert!(o.lines[1].shield);
        assert_eq!(o.background_alpha, 140);
    }

    #[test]
    fn off_means_no_overlay() {
        let mut s = settings();
        s.overlay.enabled = false;
        assert!(overlay(&s, |_| None, 0.0, false).is_none());
        assert_eq!(next_change(&s, |_| None, 0.0), None);
    }

    #[test]
    fn badges_come_from_the_card_state() {
        let s = settings();
        let mut aria = PilotState::default();
        let module = s.basilisk_chain.down.clone();
        aria.apply(cap_in(&module), "Cora".into(), 0.0, Ship::Basilisk, &s);
        aria.apply(
            CombatEvent::Damage { amount: 120 },
            String::new(),
            1.0,
            Ship::Basilisk,
            &s,
        );
        aria.apply(
            CombatEvent::Jammed { by: "Rat".into() },
            String::new(),
            1.0,
            Ship::Basilisk,
            &s,
        );
        aria.apply(
            CombatEvent::Tackle {
                scram: true,
                from: "Rat".into(),
            },
            String::new(),
            1.0,
            Ship::Basilisk,
            &s,
        );
        let pilots = |name: &str| (name == "Aria").then_some(&aria);
        let o = overlay(&s, pilots, 1.2, false).unwrap();
        let line = &o.lines[0];
        assert_eq!(line.chain.as_ref().map(|c| c.incoming.len()), Some(1));
        assert_eq!(line.damage, Some(true));
        assert_eq!(line.ecm, Some((20, 20)));
        assert_eq!(line.tackle, Some(Tackle::Scram));
        // Half a second later the DMG blink is dark.
        let o = overlay(&s, pilots, 1.6, false).unwrap();
        assert_eq!(o.lines[0].damage, Some(false));
        // The overlay wakes up for the blink.
        let next = next_change(&s, pilots, 1.2).unwrap();
        assert!(next > 1.2 && next <= 1.7);
    }

    #[test]
    fn only_busy_pilots() {
        let mut s = settings();
        s.overlay.only_busy = true;
        let o = overlay(&s, |_| None, 0.0, false).unwrap();
        // The Basilisk has no chain yet (busy), the Scimitar nothing.
        let names: Vec<&str> = o.lines.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Aria"]);
    }
}
