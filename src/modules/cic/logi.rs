//! What the Logi view knows about each pilot: the running logs on the
//! arrows, damage, ECM, tackle and the cap chain links.
//!
//! Times are seconds on the module's own clock (see `Cic::now`), passed in
//! as `now`, so this file can be tested without waiting.

use std::collections::{HashMap, VecDeque};

use super::combatlog::{CombatEvent, RepKind};
use super::config::{CicSettings, Ship};

/// The most entries kept per arrow, whatever the setting.
const MAX_ENTRIES: usize = 20;
/// The DMG badge and red border stay this long after the last hit.
pub const DMG_SECS: f64 = 3.0;
/// The POINT/SCRAM badges stay this long after the last attempt.
pub const TACKLE_SECS: f64 = 10.0;

/// The five arrows of a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Shield or armor reps we send.
    RepOut,
    CapOut,
    /// Shield or armor reps we get.
    RepIn,
    Damage,
    CapIn,
}

impl Lane {
    fn index(self) -> usize {
        match self {
            Lane::RepOut => 0,
            Lane::CapOut => 1,
            Lane::RepIn => 2,
            Lane::Damage => 3,
            Lane::CapIn => 4,
        }
    }
}

/// Which way along the cap chain the other pilot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Up,
    Down,
    /// Same module set for up and down: can't tell.
    Unknown,
    /// A module that isn't one of the two set in Settings.
    Other,
}

impl Arrow {
    /// For tooltips.
    pub fn words(self) -> &'static str {
        match self {
            Arrow::Up => "above in the chain",
            Arrow::Down => "below in the chain",
            Arrow::Unknown => "direction unknown",
            Arrow::Other => "module not set in Settings",
        }
    }

    /// Order on the chain chip: up first.
    fn order(self) -> u8 {
        match self {
            Arrow::Up => 0,
            Arrow::Unknown => 1,
            Arrow::Other => 2,
            Arrow::Down => 3,
        }
    }
}

/// One number on an arrow.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub at: f64,
    /// `None`: a miss.
    pub amount: Option<u32>,
    /// What it was, for the colour: a Basilisk can still get armor reps.
    pub kind: EntryKind,
    /// The other pilot (or structure type); empty for damage.
    pub who: String,
    /// Cap transfers only.
    pub arrow: Option<Arrow>,
    /// The module, for the tooltip; empty for damage.
    pub module: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Shield,
    Armor,
    Cap,
    Damage,
}

/// A cap chain link, from the last transfer over it.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub at: f64,
    pub who: String,
    pub arrow: Arrow,
}

/// Everything about one pilot.
#[derive(Debug, Default)]
pub struct PilotState {
    /// Newest first, one list per [`Lane`].
    lanes: [VecDeque<Entry>; 5],
    last_hit: Option<(f64, u32)>,
    jammed_at: Option<(f64, String)>,
    point: Option<(f64, String)>,
    scram: Option<(f64, String)>,
    /// Partner name -> link, for links we send over and links we get.
    links_out: HashMap<String, Link>,
    links_in: HashMap<String, Link>,
}

impl PilotState {
    /// Takes one event from this pilot's log. `ship` is what Settings says
    /// they fly, `who` the name to show for the other side.
    pub fn apply(
        &mut self,
        event: CombatEvent,
        who: String,
        now: f64,
        ship: Ship,
        settings: &CicSettings,
    ) {
        match event {
            CombatEvent::Rep {
                kind,
                outgoing,
                amount,
                module,
                ..
            } => {
                let (lane, entry_kind) = match (kind, outgoing) {
                    (RepKind::Cap, true) => (Lane::CapOut, EntryKind::Cap),
                    (RepKind::Cap, false) => (Lane::CapIn, EntryKind::Cap),
                    (RepKind::Shield, true) => (Lane::RepOut, EntryKind::Shield),
                    (RepKind::Shield, false) => (Lane::RepIn, EntryKind::Shield),
                    (RepKind::Armor, true) => (Lane::RepOut, EntryKind::Armor),
                    (RepKind::Armor, false) => (Lane::RepIn, EntryKind::Armor),
                };
                let arrow = (kind == RepKind::Cap)
                    .then(|| chain_arrow(&module, outgoing, settings.chain_modules(ship)));
                if let Some(arrow) = arrow {
                    let links = if outgoing {
                        &mut self.links_out
                    } else {
                        &mut self.links_in
                    };
                    links.insert(
                        who.clone(),
                        Link {
                            at: now,
                            who: who.clone(),
                            arrow,
                        },
                    );
                }
                self.push(
                    lane,
                    Entry {
                        at: now,
                        amount: Some(amount),
                        kind: entry_kind,
                        who,
                        arrow,
                        module,
                    },
                );
            }
            CombatEvent::Damage { amount } => {
                if amount > 0 {
                    self.last_hit = Some((now, amount));
                }
                self.push(Lane::Damage, damage_entry(now, Some(amount)));
            }
            CombatEvent::Miss => self.push(Lane::Damage, damage_entry(now, None)),
            CombatEvent::Jammed { by } => self.jammed_at = Some((now, by)),
            CombatEvent::Tackle { scram, from } => {
                if scram {
                    self.scram = Some((now, from));
                } else {
                    self.point = Some((now, from));
                }
            }
        }
    }

    fn push(&mut self, lane: Lane, entry: Entry) {
        let list = &mut self.lanes[lane.index()];
        list.push_front(entry);
        list.truncate(MAX_ENTRIES);
    }

    /// The entries to show on an arrow, newest first: at most
    /// `keep_entries`, none older than `keep_secs`.
    pub fn entries(&self, lane: Lane, now: f64, settings: &CicSettings) -> Vec<&Entry> {
        let keep = f64::from(settings.keep_secs);
        self.lanes[lane.index()]
            .iter()
            .take_while(|e| now - e.at < keep)
            .take(settings.keep_entries as usize)
            .collect()
    }

    /// Seconds since the last hit (not miss, not 0), while the DMG badge
    /// shows, and how much it was.
    pub fn hit(&self, now: f64) -> Option<(f64, u32)> {
        let (at, amount) = self.last_hit?;
        let age = now - at;
        (age < DMG_SECS).then_some((age, amount))
    }

    /// Seconds left of the ECM lock-out, and who jammed us.
    pub fn ecm_left(&self, now: f64, settings: &CicSettings) -> Option<(f64, &str)> {
        let (at, by) = self.jammed_at.as_ref()?;
        let left = f64::from(settings.ecm_secs) - (now - at);
        (left > 0.0).then_some((left, by.as_str()))
    }

    /// Who pointed (or, with `scram`, scrammed) us, while the badge shows.
    pub fn tackle(&self, scram: bool, now: f64) -> Option<&str> {
        let (at, from) = if scram { &self.scram } else { &self.point }.as_ref()?;
        (now - at <= TACKLE_SECS).then_some(from.as_str())
    }

    /// The chain links that are up, sending (`outgoing`) or receiving, up
    /// first. The chip shows at most two.
    pub fn links(&self, outgoing: bool, now: f64, settings: &CicSettings) -> Vec<&Link> {
        let links = if outgoing {
            &self.links_out
        } else {
            &self.links_in
        };
        let max_age = f64::from(settings.chain_secs);
        let mut up: Vec<&Link> = links.values().filter(|l| now - l.at <= max_age).collect();
        up.sort_by(|a, b| {
            a.arrow
                .order()
                .cmp(&b.arrow.order())
                .then_with(|| a.who.cmp(&b.who))
        });
        up
    }

    /// Forgets the chain directions, after the chain modules changed.
    pub fn clear_links(&mut self) {
        self.links_out.clear();
        self.links_in.clear();
    }

    /// When the next thing on this card changes by itself (an entry fades
    /// or goes, a badge ends, a countdown ticks), or `None` if nothing is
    /// left to change. Used to ask for one repaint then, not every frame.
    pub fn next_change(&self, now: f64, settings: &CicSettings) -> Option<f64> {
        let keep = f64::from(settings.keep_secs);
        let mut next = self.next_badge_change(now, settings);
        let mut consider = |at: f64| {
            if at > now {
                next = Some(next.map_or(at, |n: f64| n.min(at)));
            }
        };
        // Entries fade over the second half of their time: step the fade
        // once a second while one is fading, and otherwise wake up when
        // the next one starts to fade.
        let half = keep / 2.0;
        for entry in self.lanes.iter().flat_map(|lane| lane.iter()) {
            let age = now - entry.at;
            if age >= keep {
                continue;
            }
            if age >= half {
                consider(now + 1.0);
            } else {
                consider(entry.at + half);
            }
        }
        next
    }

    /// Like `next_change`, but only for the badges (DMG, chain, ECM,
    /// point/scram), not the running log: what the overlay shows.
    pub fn next_badge_change(&self, now: f64, settings: &CicSettings) -> Option<f64> {
        let mut next: Option<f64> = None;
        let mut consider = |at: f64| {
            if at > now {
                next = Some(next.map_or(at, |n: f64| n.min(at)));
            }
        };
        if self.hit(now).is_some() {
            // The DMG badge blinks every half second while it shows.
            consider(now + 0.5);
        }
        if let Some((left, _)) = self.ecm_left(now, settings) {
            // The countdown shows whole seconds (rounded up): wake up when
            // the number drops.
            let to_next = left - left.ceil() + 1.0;
            consider(now + to_next.clamp(0.01, 1.0));
        }
        for (at, _) in [&self.point, &self.scram].into_iter().flatten() {
            consider(at + TACKLE_SECS + 0.01);
        }
        let chain = f64::from(settings.chain_secs);
        for link in self.links_out.values().chain(self.links_in.values()) {
            consider(link.at + chain + 0.01);
        }
        next
    }
}

fn damage_entry(now: f64, amount: Option<u32>) -> Entry {
    Entry {
        at: now,
        amount,
        kind: EntryKind::Damage,
        who: String::new(),
        arrow: None,
        module: String::new(),
    }
}

/// Which way along the chain a cap transfer goes, as seen by the pilot
/// whose log it is.
///
/// Sending: our Up module sends to the pilot above us (↑), our Down module
/// to the one below (↓). Receiving: the log names the sender's module, so a
/// sender using the Up module is below us (↓), Down module above us (↑).
/// That assumes the sender flies the same chain modules. If Up and Down
/// are the same module, the direction can't be known (↕).
pub fn chain_arrow(
    module: &str,
    outgoing: bool,
    modules: Option<&super::config::ChainModules>,
) -> Arrow {
    let Some(modules) = modules else {
        return Arrow::Other;
    };
    if modules.up == modules.down {
        return if module == modules.up {
            Arrow::Unknown
        } else {
            Arrow::Other
        };
    }
    let up = module == modules.up;
    let down = module == modules.down;
    match (up, down, outgoing) {
        (true, _, true) | (_, true, false) => Arrow::Up,
        (_, true, true) | (true, _, false) => Arrow::Down,
        _ => Arrow::Other,
    }
}

/// Opacity of an entry: full for the first half of `keep_secs`, then
/// fading to a quarter.
pub fn entry_opacity(age: f64, keep_secs: u32) -> f32 {
    let half = f64::from(keep_secs) / 2.0;
    let fade = ((age - half).max(0.0) / half).min(1.0);
    (1.0 - fade).max(0.25) as f32
}

#[cfg(test)]
mod tests {
    use super::super::combatlog::Other;
    use super::super::config::ChainModules;
    use super::*;

    const MEDIUM: &str = "Medium Remote Capacitor Transmitter II";
    const LARGE: &str = "Large Remote Capacitor Transmitter II";

    fn settings() -> CicSettings {
        CicSettings {
            basilisk_chain: ChainModules {
                up: MEDIUM.into(),
                down: LARGE.into(),
            },
            guardian_chain: ChainModules {
                up: LARGE.into(),
                down: LARGE.into(),
            },
            ..CicSettings::default()
        }
    }

    fn cap(outgoing: bool, amount: u32, module: &str) -> CombatEvent {
        CombatEvent::Rep {
            kind: RepKind::Cap,
            outgoing,
            amount,
            other: Other {
                name: "x".into(),
                ship: "Basilisk".into(),
            },
            module: module.into(),
        }
    }

    #[test]
    fn chain_arrows_follow_the_modules() {
        let s = settings();
        let basilisk = s.chain_modules(Ship::Basilisk);
        assert_eq!(chain_arrow(MEDIUM, true, basilisk), Arrow::Up);
        assert_eq!(chain_arrow(LARGE, true, basilisk), Arrow::Down);
        assert_eq!(chain_arrow(MEDIUM, false, basilisk), Arrow::Down);
        assert_eq!(chain_arrow(LARGE, false, basilisk), Arrow::Up);
        assert_eq!(chain_arrow("Small thing", true, basilisk), Arrow::Other);
        let guardian = s.chain_modules(Ship::Guardian);
        assert_eq!(chain_arrow(LARGE, true, guardian), Arrow::Unknown);
        assert_eq!(chain_arrow(LARGE, true, None), Arrow::Other);
    }

    #[test]
    fn running_log_keeps_newest_within_limits() {
        let s = CicSettings {
            keep_entries: 3,
            keep_secs: 30,
            ..settings()
        };
        let mut pilot = PilotState::default();
        for i in 0..5 {
            pilot.apply(
                CombatEvent::Damage { amount: 100 + i },
                String::new(),
                f64::from(i),
                Ship::Basilisk,
                &s,
            );
        }
        let shown: Vec<u32> = pilot
            .entries(Lane::Damage, 5.0, &s)
            .iter()
            .filter_map(|e| e.amount)
            .collect();
        assert_eq!(shown, [104, 103, 102]);
        // Everything is older than 30 s later on.
        assert!(pilot.entries(Lane::Damage, 40.0, &s).is_empty());
    }

    #[test]
    fn misses_and_zero_damage_do_not_flash() {
        let s = settings();
        let mut pilot = PilotState::default();
        pilot.apply(CombatEvent::Miss, String::new(), 0.0, Ship::Scimitar, &s);
        pilot.apply(
            CombatEvent::Damage { amount: 0 },
            String::new(),
            0.0,
            Ship::Scimitar,
            &s,
        );
        assert!(pilot.hit(0.5).is_none());
        assert_eq!(pilot.entries(Lane::Damage, 0.5, &s).len(), 2);
        pilot.apply(
            CombatEvent::Damage { amount: 50 },
            String::new(),
            1.0,
            Ship::Scimitar,
            &s,
        );
        assert_eq!(pilot.hit(2.0), Some((1.0, 50)));
        assert!(pilot.hit(4.1).is_none());
    }

    #[test]
    fn ecm_counts_down() {
        let s = settings();
        let mut pilot = PilotState::default();
        pilot.apply(
            CombatEvent::Jammed { by: "Npc".into() },
            String::new(),
            10.0,
            Ship::Basilisk,
            &s,
        );
        assert_eq!(pilot.ecm_left(15.0, &s), Some((15.0, "Npc")));
        assert!(pilot.ecm_left(30.0, &s).is_none());
    }

    #[test]
    fn tackle_badges_last_ten_seconds() {
        let s = settings();
        let mut pilot = PilotState::default();
        pilot.apply(
            CombatEvent::Tackle {
                scram: true,
                from: "Rex (Sabre)".into(),
            },
            String::new(),
            0.0,
            Ship::Basilisk,
            &s,
        );
        assert_eq!(pilot.tackle(true, 10.0), Some("Rex (Sabre)"));
        assert_eq!(pilot.tackle(false, 1.0), None);
        assert_eq!(pilot.tackle(true, 10.5), None);
    }

    #[test]
    fn chain_links_count_zero_transfers_and_expire() {
        let s = settings();
        let mut pilot = PilotState::default();
        pilot.apply(
            cap(true, 0, MEDIUM),
            "Above".into(),
            0.0,
            Ship::Basilisk,
            &s,
        );
        pilot.apply(
            cap(true, 300, LARGE),
            "Below".into(),
            1.0,
            Ship::Basilisk,
            &s,
        );
        pilot.apply(
            cap(false, 117, MEDIUM),
            "Below".into(),
            1.0,
            Ship::Basilisk,
            &s,
        );
        let out: Vec<(&str, Arrow)> = pilot
            .links(true, 5.0, &s)
            .iter()
            .map(|l| (l.who.as_str(), l.arrow))
            .collect();
        assert_eq!(out, [("Above", Arrow::Up), ("Below", Arrow::Down)]);
        let incoming = pilot.links(false, 5.0, &s);
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].arrow, Arrow::Down);
        // 12 s without a transfer: the link to "Above" is gone.
        assert_eq!(pilot.links(true, 12.5, &s).len(), 1);
        assert!(pilot.links(true, 20.0, &s).is_empty());
        // The cap log shows the 0 too, with its arrow.
        let entries = pilot.entries(Lane::CapOut, 5.0, &s);
        assert_eq!(entries[1].amount, Some(0));
        assert_eq!(entries[1].arrow, Some(Arrow::Up));
    }

    #[test]
    fn nothing_to_repaint_when_idle() {
        let s = settings();
        let mut pilot = PilotState::default();
        assert_eq!(pilot.next_change(0.0, &s), None);
        pilot.apply(
            CombatEvent::Damage { amount: 10 },
            String::new(),
            0.0,
            Ship::Basilisk,
            &s,
        );
        assert_eq!(pilot.next_change(1.0, &s), Some(1.5));
        // After everything has faded, no more repaints.
        assert_eq!(pilot.next_change(100.0, &s), None);
    }

    #[test]
    fn repaints_only_when_something_shows_a_change() {
        let s = settings();
        let mut pilot = PilotState::default();
        pilot.apply(CombatEvent::Miss, String::new(), 0.0, Ship::Basilisk, &s);
        // A miss doesn't blink: the next change is when it starts to fade.
        assert_eq!(pilot.next_change(1.0, &s), Some(15.0));
        assert_eq!(pilot.next_change(20.0, &s), Some(21.0));
        pilot.apply(
            CombatEvent::Jammed { by: "Npc".into() },
            String::new(),
            30.0,
            Ship::Basilisk,
            &s,
        );
        // 20 s lock-out, 19.7 s left shows "20 s"; it drops to 19 in 0.7 s.
        let next = pilot.next_change(30.3, &s).unwrap();
        assert!((next - 31.0).abs() < 1e-9, "{next}");
    }

    #[test]
    fn fade_steps() {
        assert_eq!(entry_opacity(0.0, 30), 1.0);
        assert_eq!(entry_opacity(15.0, 30), 1.0);
        assert!((entry_opacity(22.5, 30) - 0.5).abs() < 0.01);
        assert_eq!(entry_opacity(29.0, 30), 0.25);
    }
}
