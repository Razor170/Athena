//! Finding what changed about a contract, for the changelog.
//!
//! After every ESI round (and once a minute, because expiry and "soon"
//! move with the clock) the worker takes a [`Snapshot`] of each contract
//! and compares it with the one saved last time. Each difference becomes a
//! [`Change`]. Snapshots are saved in `contracts.sqlite`, so a change that
//! happened while Athena was closed is found on the next start.
//!
//! Plain functions without I/O, so they are easy to test.

use serde::{Deserialize, Serialize};

use super::data::{Contract, ContractType, Side, Stage};

/// What the changelog compares: the stage, the number of bids, and
/// whether the "expires soon" or "due soon" warning is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub stage: Stage,
    pub bids: usize,
    pub soon: bool,
}

/// The kinds of changelog entries. Saved by [`ChangeKind::key`], so never
/// rename a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// One of our characters made a new contract.
    Issued,
    /// Someone made a contract for one of our characters.
    NewForYou,
    /// One of our characters accepted someone's courier contract.
    YouAccepted,
    /// Someone accepted our contract (couriers: picked up).
    Accepted,
    /// Sold, bought, delivered.
    Finished,
    NewBid,
    Expired,
    Cancelled,
    Rejected,
    Failed,
    Deleted,
    Reversed,
    /// A courier wasn't delivered in time.
    Overdue,
    ExpiresSoon,
    DueSoon,
}

/// The groups that have their own Alert and Sound switch in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Assigned,
    Accepted,
    Finished,
    Bid,
    Ended,
    Soon,
}

impl AlertKind {
    pub const ALL: [AlertKind; 6] = [
        AlertKind::Assigned,
        AlertKind::Accepted,
        AlertKind::Finished,
        AlertKind::Bid,
        AlertKind::Ended,
        AlertKind::Soon,
    ];

    /// The line in Settings.
    pub fn label(self) -> &'static str {
        match self {
            AlertKind::Assigned => "A contract is assigned to me",
            AlertKind::Accepted => "My contract is accepted (courier picked up)",
            AlertKind::Finished => "My contract is finished (sold, bought, delivered)",
            AlertKind::Bid => "New bid on my auction",
            AlertKind::Ended => "Expired, failed, rejected, cancelled or deleted",
            AlertKind::Soon => "Expires soon, or a courier I accepted is due soon",
        }
    }
}

impl ChangeKind {
    const ALL: [ChangeKind; 15] = [
        ChangeKind::Issued,
        ChangeKind::NewForYou,
        ChangeKind::YouAccepted,
        ChangeKind::Accepted,
        ChangeKind::Finished,
        ChangeKind::NewBid,
        ChangeKind::Expired,
        ChangeKind::Cancelled,
        ChangeKind::Rejected,
        ChangeKind::Failed,
        ChangeKind::Deleted,
        ChangeKind::Reversed,
        ChangeKind::Overdue,
        ChangeKind::ExpiresSoon,
        ChangeKind::DueSoon,
    ];

    /// The text saved in `contracts.sqlite`.
    pub fn key(self) -> &'static str {
        match self {
            ChangeKind::Issued => "issued",
            ChangeKind::NewForYou => "new_for_you",
            ChangeKind::YouAccepted => "you_accepted",
            ChangeKind::Accepted => "accepted",
            ChangeKind::Finished => "finished",
            ChangeKind::NewBid => "new_bid",
            ChangeKind::Expired => "expired",
            ChangeKind::Cancelled => "cancelled",
            ChangeKind::Rejected => "rejected",
            ChangeKind::Failed => "failed",
            ChangeKind::Deleted => "deleted",
            ChangeKind::Reversed => "reversed",
            ChangeKind::Overdue => "overdue",
            ChangeKind::ExpiresSoon => "expires_soon",
            ChangeKind::DueSoon => "due_soon",
        }
    }

    pub fn from_key(key: &str) -> Option<ChangeKind> {
        ChangeKind::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// Which Settings switch covers it, for a contract where our
    /// character is on `side`. `None`: only logged, never an alert. That
    /// covers what you did yourself: issuing, accepting, a purchase or a
    /// delivery of yours finishing, rejecting a contract made for you, and
    /// cancelling or deleting your own. Bids only alert on your own
    /// auctions.
    pub fn alert_kind(self, side: Side) -> Option<AlertKind> {
        match (self, side) {
            (ChangeKind::Issued | ChangeKind::YouAccepted, _) => None,
            (ChangeKind::Finished, Side::Accepted) => None,
            (ChangeKind::Rejected, Side::AssignedToMe) => None,
            (ChangeKind::Cancelled | ChangeKind::Deleted, Side::Issued) => None,
            (ChangeKind::NewBid, side) if side != Side::Issued => None,
            (kind, _) => kind.switch(),
        }
    }

    /// The switch of each kind, before looking at who did it.
    fn switch(self) -> Option<AlertKind> {
        match self {
            ChangeKind::Issued | ChangeKind::YouAccepted => None,
            ChangeKind::NewForYou => Some(AlertKind::Assigned),
            ChangeKind::Accepted => Some(AlertKind::Accepted),
            ChangeKind::Finished => Some(AlertKind::Finished),
            ChangeKind::NewBid => Some(AlertKind::Bid),
            ChangeKind::Expired
            | ChangeKind::Cancelled
            | ChangeKind::Rejected
            | ChangeKind::Failed
            | ChangeKind::Deleted
            | ChangeKind::Reversed => Some(AlertKind::Ended),
            ChangeKind::Overdue | ChangeKind::ExpiresSoon | ChangeKind::DueSoon => {
                Some(AlertKind::Soon)
            }
        }
    }
}

/// One change, before it is saved.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub kind: ChangeKind,
    /// When it happened, as far as ESI's dates tell (Unix seconds).
    pub at: i64,
    /// The bid, for `NewBid`.
    pub amount: Option<f64>,
}

/// The contract's snapshot at `now`. `soon_secs`: how long before the
/// expiry (or a courier's due time) the "soon" warning comes on.
pub fn snapshot(contract: &Contract, mine: &[u64], now: i64, soon_secs: i64) -> Snapshot {
    let stage = contract.stage(mine, now);
    let side = contract.side(mine).0;
    let soon = match deadline(contract, stage, side) {
        Some(deadline) => deadline - now <= soon_secs,
        None => false,
    };
    Snapshot {
        stage,
        bids: contract.bids.len(),
        soon,
    }
}

/// The time the "soon" warning counts down to, if there is one for us:
/// the expiry of a contract we issued or that waits for us, or when a
/// courier we haul must be delivered.
pub fn deadline(contract: &Contract, stage: Stage, side: Side) -> Option<i64> {
    match (stage, side) {
        (
            Stage::Waiting | Stage::HasBids | Stage::AssignedToYou,
            Side::Issued | Side::AssignedToMe,
        ) => Some(contract.expires),
        (Stage::InProgress, Side::Accepted) => contract.due(),
        _ => None,
    }
}

/// The changes between the `old` snapshot (`None`: the contract is new to
/// us) and `new`, oldest first.
pub fn changes(
    contract: &Contract,
    old: Option<Snapshot>,
    new: Snapshot,
    mine: &[u64],
    now: i64,
    soon_secs: i64,
) -> Vec<Change> {
    let side = contract.side(mine).0;
    let mut out = Vec::new();
    let mut push = |kind: ChangeKind, at: Option<i64>, amount: Option<f64>| {
        out.push(Change {
            kind,
            // Never in the future, and "now" if ESI gave no date.
            at: at.unwrap_or(now).min(now),
            amount,
        });
    };

    // What it was before. A new contract counts as just made (or, for a
    // courier we accepted, as just accepted).
    let before = match old {
        Some(old) => old,
        None => {
            match side {
                Side::Issued => push(ChangeKind::Issued, Some(contract.issued), None),
                Side::AssignedToMe => push(ChangeKind::NewForYou, Some(contract.issued), None),
                Side::Accepted => push(ChangeKind::YouAccepted, contract.accepted, None),
                Side::Other => {}
            }
            let stage = if side == Side::Accepted {
                Stage::InProgress
            } else {
                Stage::Waiting
            };
            Snapshot {
                stage,
                bids: 0,
                soon: false,
            }
        }
    };

    // Waiting, "has bids" and "assigned to you" are all the same stage
    // for the changelog (bids get their own entry below).
    let both_open = before.stage.is_open() && new.stage.is_open();
    if new.stage != before.stage && !both_open {
        let completed = contract.completed;
        let stage_change = match new.stage {
            Stage::InProgress => Some((ChangeKind::Accepted, contract.accepted)),
            Stage::Overdue => Some((ChangeKind::Overdue, contract.due())),
            Stage::Expired => Some((ChangeKind::Expired, Some(contract.expires))),
            Stage::Finished => Some((ChangeKind::Finished, completed)),
            Stage::Cancelled => Some((ChangeKind::Cancelled, completed)),
            Stage::Rejected => Some((ChangeKind::Rejected, completed)),
            Stage::Failed => Some((ChangeKind::Failed, completed)),
            Stage::Deleted => Some((ChangeKind::Deleted, completed)),
            Stage::Reversed => Some((ChangeKind::Reversed, completed)),
            Stage::Waiting | Stage::HasBids | Stage::AssignedToYou | Stage::Unknown => None,
        };
        if let Some((kind, at)) = stage_change {
            push(kind, at, None);
        }
    }

    if contract.kind == ContractType::Auction && new.bids > before.bids {
        let top = contract.top_bid();
        push(
            ChangeKind::NewBid,
            top.map(|bid| bid.at),
            top.map(|bid| bid.amount),
        );
    }

    if new.soon && !before.soon && new.stage.is_active() {
        let at = deadline(contract, new.stage, side).map(|d| d - soon_secs);
        let kind = if new.stage == Stage::InProgress {
            ChangeKind::DueSoon
        } else {
            ChangeKind::ExpiresSoon
        };
        push(kind, at, None);
    }

    out.sort_by_key(|change| change.at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::contracts::data::Bid;
    use crate::modules::contracts::data::tests::esi_courier;

    const SOON: i64 = 24 * 3600;

    fn kinds(list: &[Change]) -> Vec<ChangeKind> {
        list.iter().map(|c| c.kind).collect()
    }

    #[test]
    fn own_actions_never_alert() {
        // Buying someone's item exchange: accepted and finished by you.
        let mut esi = esi_courier();
        esi.kind = "item_exchange".into();
        esi.issuer_id = 77;
        esi.acceptor_id = 1;
        esi.price = Some(5e6);
        esi.status = "finished".into();
        esi.date_accepted = Some("2026-11-13T06:00:00Z".into());
        esi.date_completed = Some("2026-11-13T06:00:00Z".into());
        let bought = Contract::from_esi(esi, 1).unwrap();
        let now = bought.completed.unwrap() + 60;
        let snap = snapshot(&bought, &[1], now, SOON);
        let list = changes(&bought, None, snap, &[1], now, SOON);
        let side = bought.side(&[1]).0;
        let alerts: Vec<_> = list
            .iter()
            .map(|c| (c.kind, c.kind.alert_kind(side)))
            .collect();
        assert_eq!(
            alerts,
            vec![
                (ChangeKind::YouAccepted, None),
                (ChangeKind::Finished, None)
            ]
        );
        // Your own contract sold by someone else does alert.
        assert_eq!(
            ChangeKind::Finished.alert_kind(Side::Issued),
            Some(AlertKind::Finished)
        );
        assert_eq!(ChangeKind::Deleted.alert_kind(Side::Issued), None);
        assert_eq!(ChangeKind::Rejected.alert_kind(Side::AssignedToMe), None);
        assert_eq!(
            ChangeKind::Rejected.alert_kind(Side::Issued),
            Some(AlertKind::Ended)
        );
        assert_eq!(ChangeKind::NewBid.alert_kind(Side::Other), None);
        assert_eq!(
            ChangeKind::NewBid.alert_kind(Side::Issued),
            Some(AlertKind::Bid)
        );
    }

    #[test]
    fn keys_round_trip() {
        for kind in ChangeKind::ALL {
            assert_eq!(ChangeKind::from_key(kind.key()), Some(kind));
        }
        assert_eq!(ChangeKind::from_key("nope"), None);
    }

    #[test]
    fn a_new_contract_is_logged_once() {
        let contract = Contract::from_esi(esi_courier(), 1).unwrap();
        let now = contract.issued + 60;
        let snap = snapshot(&contract, &[1], now, SOON);
        let list = changes(&contract, None, snap, &[1], now, SOON);
        assert_eq!(kinds(&list), vec![ChangeKind::Issued]);
        assert_eq!(list[0].at, contract.issued);
        // Nothing changed: nothing logged.
        assert!(changes(&contract, Some(snap), snap, &[1], now, SOON).is_empty());
    }

    #[test]
    fn courier_accepted_then_delivered() {
        let mut contract = Contract::from_esi(esi_courier(), 1).unwrap();
        let start = contract.issued + 60;
        let before = snapshot(&contract, &[1], start, SOON);

        contract.status = "in_progress".into();
        contract.acceptor_id = 99;
        contract.accepted = Some(start + 100);
        let now = start + 200;
        let accepted = snapshot(&contract, &[1], now, SOON);
        let list = changes(&contract, Some(before), accepted, &[1], now, SOON);
        assert_eq!(kinds(&list), vec![ChangeKind::Accepted]);
        assert_eq!(list[0].at, start + 100);
        // We issued it, so its due time isn't our "soon".
        assert!(!accepted.soon);

        contract.status = "finished".into();
        contract.completed = Some(now + 50);
        let done = snapshot(&contract, &[1], now + 100, SOON);
        let list = changes(&contract, Some(accepted), done, &[1], now + 100, SOON);
        assert_eq!(kinds(&list), vec![ChangeKind::Finished]);
    }

    #[test]
    fn expiry_and_soon_come_from_the_clock() {
        let contract = Contract::from_esi(esi_courier(), 1).unwrap();
        let early = contract.issued;
        let first = snapshot(&contract, &[1], early, SOON);
        assert!(!first.soon);

        let near = contract.expires - 3600;
        let soon = snapshot(&contract, &[1], near, SOON);
        let list = changes(&contract, Some(first), soon, &[1], near, SOON);
        assert_eq!(kinds(&list), vec![ChangeKind::ExpiresSoon]);
        assert_eq!(list[0].at, contract.expires - SOON);

        let after = contract.expires + 10;
        let expired = snapshot(&contract, &[1], after, SOON);
        let list = changes(&contract, Some(soon), expired, &[1], after, SOON);
        assert_eq!(kinds(&list), vec![ChangeKind::Expired]);
        assert_eq!(list[0].at, contract.expires);
    }

    #[test]
    fn found_late_still_gets_every_step() {
        // A contract we never saw, which is already finished (made and
        // sold while Athena was closed).
        let mut esi = esi_courier();
        esi.kind = "item_exchange".into();
        esi.price = Some(620e6);
        esi.status = "finished".into();
        esi.acceptor_id = 99;
        esi.date_completed = Some("2026-11-13T06:51:00Z".into());
        let contract = Contract::from_esi(esi, 1).unwrap();
        let now = contract.completed.unwrap() + 3600;
        let snap = snapshot(&contract, &[1], now, SOON);
        let list = changes(&contract, None, snap, &[1], now, SOON);
        assert_eq!(kinds(&list), vec![ChangeKind::Issued, ChangeKind::Finished]);
    }

    #[test]
    fn bids_and_hauling_for_others() {
        let mut esi = esi_courier();
        esi.kind = "auction".into();
        let mut auction = Contract::from_esi(esi, 1).unwrap();
        let now = auction.issued + 60;
        let before = snapshot(&auction, &[1], now, SOON);
        auction.bids.push(Bid {
            id: 1,
            bidder_id: 5,
            amount: 95e6,
            at: now - 10,
        });
        let after = snapshot(&auction, &[1], now, SOON);
        assert_eq!(after.stage, Stage::HasBids);
        let list = changes(&auction, Some(before), after, &[1], now, SOON);
        assert_eq!(kinds(&list), vec![ChangeKind::NewBid]);
        assert_eq!(list[0].amount, Some(95e6));

        // A courier character 2 accepted from a stranger, due soon.
        let mut esi = esi_courier();
        esi.issuer_id = 77;
        esi.acceptor_id = 2;
        esi.status = "in_progress".into();
        esi.date_accepted = Some("2026-11-12T13:00:00Z".into());
        let haul = Contract::from_esi(esi, 2).unwrap();
        let due = haul.due().unwrap();
        let now = due - 3600;
        let snap = snapshot(&haul, &[2], now, SOON);
        assert!(snap.soon);
        let list = changes(&haul, None, snap, &[2], now, SOON);
        assert_eq!(
            kinds(&list),
            vec![ChangeKind::YouAccepted, ChangeKind::DueSoon]
        );
    }
}
