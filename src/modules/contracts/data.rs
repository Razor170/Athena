//! What ESI sends about contracts, and the structs Athena keeps.
//!
//! A [`Contract`] is saved as JSON in `contracts.sqlite`, so new fields
//! need `#[serde(default)]` to keep older rows readable.

use serde::{Deserialize, Serialize};

use crate::modules::finance::data::parse_esi_date;

/// One line of `/characters/{id}/contracts` (only the parts we use).
#[derive(Debug, Clone, Deserialize)]
pub struct EsiContract {
    pub contract_id: i64,
    #[serde(default)]
    pub acceptor_id: i64,
    #[serde(default)]
    pub assignee_id: i64,
    #[serde(default)]
    pub availability: String,
    pub buyout: Option<f64>,
    pub collateral: Option<f64>,
    pub date_accepted: Option<String>,
    pub date_completed: Option<String>,
    pub date_expired: String,
    pub date_issued: String,
    pub days_to_complete: Option<i64>,
    pub end_location_id: Option<i64>,
    #[serde(default)]
    pub for_corporation: bool,
    #[serde(default)]
    pub issuer_corporation_id: i64,
    pub issuer_id: i64,
    pub price: Option<f64>,
    pub reward: Option<f64>,
    pub start_location_id: Option<i64>,
    pub status: String,
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub volume: Option<f64>,
}

/// One line of `/characters/{id}/contracts/{contract_id}/items`.
#[derive(Debug, Clone, Deserialize)]
pub struct EsiItem {
    pub type_id: i64,
    pub quantity: i64,
    /// False: the issuer asks for this item (a "want to buy").
    pub is_included: bool,
}

/// One line of `/characters/{id}/contracts/{contract_id}/bids`.
#[derive(Debug, Clone, Deserialize)]
pub struct EsiBid {
    pub bid_id: i64,
    pub bidder_id: i64,
    pub amount: f64,
    pub date_bid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ContractType {
    ItemExchange,
    Auction,
    Courier,
    Loan,
    #[default]
    Unknown,
}

impl ContractType {
    fn from_esi(text: &str) -> ContractType {
        match text {
            "item_exchange" => ContractType::ItemExchange,
            "auction" => ContractType::Auction,
            "courier" => ContractType::Courier,
            "loan" => ContractType::Loan,
            _ => ContractType::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContractType::ItemExchange => "Item exchange",
            ContractType::Auction => "Auction",
            ContractType::Courier => "Courier",
            ContractType::Loan => "Loan",
            ContractType::Unknown => "Contract",
        }
    }
}

/// An item in a contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub type_id: i64,
    pub quantity: i64,
    /// False: the issuer asks for it instead of giving it.
    pub included: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bid {
    pub id: i64,
    pub bidder_id: i64,
    pub amount: f64,
    /// Unix seconds.
    pub at: i64,
}

/// A contract as Athena keeps it. Times are Unix seconds (UTC).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    pub id: i64,
    pub kind: ContractType,
    /// ESI's status text: "outstanding", "in_progress", "finished", ...
    pub status: String,
    pub title: String,
    pub availability: String,
    pub issuer_id: i64,
    pub issuer_corporation_id: i64,
    pub for_corporation: bool,
    /// 0 when nobody (yet).
    pub assignee_id: i64,
    pub acceptor_id: i64,
    pub price: f64,
    pub reward: f64,
    pub collateral: f64,
    pub buyout: f64,
    pub volume: f64,
    pub start_location_id: i64,
    pub end_location_id: i64,
    pub issued: i64,
    pub expires: i64,
    pub accepted: Option<i64>,
    pub completed: Option<i64>,
    pub days_to_complete: i64,
    /// Our characters ESI listed this contract for.
    #[serde(default)]
    pub characters: Vec<u64>,
    /// `None` until fetched. Empty when EVE doesn't show them (for
    /// example to the hauler of a courier contract).
    #[serde(default)]
    pub items: Option<Vec<Item>>,
    /// Auction bids, lowest first.
    #[serde(default)]
    pub bids: Vec<Bid>,
}

impl Contract {
    /// `None` if a date ESI must send is broken.
    pub fn from_esi(esi: EsiContract, character_id: u64) -> Option<Contract> {
        Some(Contract {
            id: esi.contract_id,
            kind: ContractType::from_esi(&esi.kind),
            status: esi.status,
            title: esi.title.unwrap_or_default().trim().to_string(),
            availability: esi.availability,
            issuer_id: esi.issuer_id,
            issuer_corporation_id: esi.issuer_corporation_id,
            for_corporation: esi.for_corporation,
            assignee_id: esi.assignee_id,
            acceptor_id: esi.acceptor_id,
            price: esi.price.unwrap_or(0.0),
            reward: esi.reward.unwrap_or(0.0),
            collateral: esi.collateral.unwrap_or(0.0),
            buyout: esi.buyout.unwrap_or(0.0),
            volume: esi.volume.unwrap_or(0.0),
            start_location_id: esi.start_location_id.unwrap_or(0),
            end_location_id: esi.end_location_id.unwrap_or(0),
            issued: parse_esi_date(&esi.date_issued)?,
            expires: parse_esi_date(&esi.date_expired)?,
            accepted: esi.date_accepted.as_deref().and_then(parse_esi_date),
            completed: esi.date_completed.as_deref().and_then(parse_esi_date),
            days_to_complete: esi.days_to_complete.unwrap_or(0),
            characters: vec![character_id],
            items: None,
            bids: Vec::new(),
        })
    }

    /// Takes the new ESI data, keeping what ESI didn't send again (items,
    /// bids, the characters already known). Returns true if anything
    /// shown changed.
    pub fn merge_from(&mut self, newer: Contract) -> bool {
        let mut characters = self.characters.clone();
        for id in &newer.characters {
            if !characters.contains(id) {
                characters.push(*id);
            }
        }
        let merged = Contract {
            characters,
            items: self.items.clone(),
            bids: self.bids.clone(),
            ..newer
        };
        let changed = merged != *self;
        *self = merged;
        changed
    }

    /// Which of our characters this is about, and how.
    pub fn side(&self, mine: &[u64]) -> (Side, Option<u64>) {
        let is_mine = |id: i64| u64::try_from(id).ok().filter(|id| mine.contains(id));
        if let Some(id) = is_mine(self.issuer_id) {
            return (Side::Issued, Some(id));
        }
        if let Some(id) = is_mine(self.acceptor_id) {
            return (Side::Accepted, Some(id));
        }
        if let Some(id) = is_mine(self.assignee_id) {
            return (Side::AssignedToMe, Some(id));
        }
        (Side::Other, self.characters.first().copied())
    }

    /// When a courier must be delivered, once it was accepted.
    pub fn due(&self) -> Option<i64> {
        (self.kind == ContractType::Courier && self.days_to_complete > 0)
            .then_some(self.accepted?)
            .map(|accepted| accepted + self.days_to_complete * 86_400)
    }

    /// The highest bid, if any.
    pub fn top_bid(&self) -> Option<&Bid> {
        self.bids
            .iter()
            .max_by(|a, b| a.amount.total_cmp(&b.amount))
    }

    /// Where this contract stands at `now`. ESI keeps a contract
    /// "outstanding" after it expired, so that is worked out here.
    pub fn stage(&self, mine: &[u64], now: i64) -> Stage {
        match self.status.as_str() {
            "outstanding" if now >= self.expires => Stage::Expired,
            "outstanding" if self.kind == ContractType::Auction && !self.bids.is_empty() => {
                Stage::HasBids
            }
            "outstanding" if self.side(mine).0 == Side::AssignedToMe => Stage::AssignedToYou,
            "outstanding" => Stage::Waiting,
            "in_progress" if self.due().is_some_and(|due| now >= due) => Stage::Overdue,
            "in_progress" => Stage::InProgress,
            "finished_issuer" | "finished_contractor" | "finished" => Stage::Finished,
            "cancelled" => Stage::Cancelled,
            "rejected" => Stage::Rejected,
            "failed" => Stage::Failed,
            "deleted" => Stage::Deleted,
            "reversed" => Stage::Reversed,
            _ => Stage::Unknown,
        }
    }

    /// When it finished, expired or ended, for the "Finished" list.
    pub fn ended_at(&self, stage: Stage) -> i64 {
        match stage {
            Stage::Expired => self.expires,
            _ => self.completed.unwrap_or(self.expires),
        }
    }
}

/// How one of our characters is part of a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Issued,
    Accepted,
    AssignedToMe,
    /// Seen by ESI for one of our characters some other way (a corp
    /// contract, an auction they bid on).
    Other,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Issued => "you issued",
            Side::Accepted => "you accepted",
            Side::AssignedToMe => "to you",
            Side::Other => "",
        }
    }
}

/// Where a contract stands, as the list shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    Waiting,
    HasBids,
    AssignedToYou,
    InProgress,
    Overdue,
    Expired,
    Finished,
    Cancelled,
    Rejected,
    Failed,
    Deleted,
    Reversed,
    Unknown,
}

impl Stage {
    /// Still waiting for someone, or being hauled.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Stage::Waiting
                | Stage::HasBids
                | Stage::AssignedToYou
                | Stage::InProgress
                | Stage::Overdue
        )
    }

    /// Nobody accepted it yet.
    pub fn is_open(self) -> bool {
        matches!(self, Stage::Waiting | Stage::HasBids | Stage::AssignedToYou)
    }

    /// The status text in the list. "Finished" depends on the contract.
    pub fn label(self, contract: &Contract, side: Side) -> &'static str {
        match self {
            Stage::Waiting => "Waiting",
            Stage::HasBids => "Has bids",
            Stage::AssignedToYou => "Assigned to you",
            Stage::InProgress => "In progress",
            Stage::Overdue => "Overdue",
            Stage::Expired => "Expired",
            Stage::Finished => finished_label(contract, side),
            Stage::Cancelled => "Cancelled",
            Stage::Rejected => "Rejected",
            Stage::Failed => "Failed",
            Stage::Deleted => "Deleted",
            Stage::Reversed => "Reversed",
            Stage::Unknown => "Unknown",
        }
    }
}

/// "Delivered", "Sold", "Bought" or "Finished".
pub fn finished_label(contract: &Contract, side: Side) -> &'static str {
    match (contract.kind, side) {
        (ContractType::Courier, _) => "Delivered",
        (ContractType::Auction, Side::Issued) => "Sold",
        (ContractType::Auction, _) => "Won",
        (ContractType::ItemExchange, Side::Issued) if contract.price > 0.0 => "Sold",
        (ContractType::ItemExchange, Side::Issued) if contract.reward > 0.0 => "Bought",
        (ContractType::ItemExchange, Side::Accepted | Side::AssignedToMe)
            if contract.price > 0.0 =>
        {
            "Bought"
        }
        _ => "Finished",
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A courier contract issued by character 1, as ESI might send it.
    pub fn esi_courier() -> EsiContract {
        EsiContract {
            contract_id: 500,
            acceptor_id: 0,
            assignee_id: 0,
            availability: "public".into(),
            buyout: None,
            collateral: Some(1.2e9),
            date_accepted: None,
            date_completed: None,
            date_expired: "2026-11-20T12:00:00Z".into(),
            date_issued: "2026-11-12T12:00:00Z".into(),
            days_to_complete: Some(3),
            end_location_id: Some(60_008_494),
            for_corporation: false,
            issuer_corporation_id: 98_000_001,
            issuer_id: 1,
            price: Some(0.0),
            reward: Some(18.5e6),
            start_location_id: Some(60_003_760),
            status: "outstanding".into(),
            title: Some(String::new()),
            kind: "courier".into(),
            volume: Some(320_000.0),
        }
    }

    #[test]
    fn parses_and_finds_the_stage() {
        let mut contract = Contract::from_esi(esi_courier(), 1).unwrap();
        let issued = parse_esi_date("2026-11-12T12:00:00Z").unwrap();
        assert_eq!(contract.issued, issued);
        assert_eq!(contract.side(&[1]), (Side::Issued, Some(1)));
        assert_eq!(contract.stage(&[1], issued), Stage::Waiting);
        // Past its expiry ESI still says "outstanding".
        assert_eq!(contract.stage(&[1], contract.expires), Stage::Expired);

        contract.status = "in_progress".into();
        contract.acceptor_id = 9;
        contract.accepted = Some(issued + 3600);
        let due = contract.due().unwrap();
        assert_eq!(due, issued + 3600 + 3 * 86_400);
        assert_eq!(contract.stage(&[1], due - 1), Stage::InProgress);
        assert_eq!(contract.stage(&[1], due), Stage::Overdue);

        contract.status = "finished".into();
        assert_eq!(contract.stage(&[1], due), Stage::Finished);
        assert_eq!(finished_label(&contract, Side::Issued), "Delivered");
    }

    #[test]
    fn merge_keeps_items_bids_and_characters() {
        let mut saved = Contract::from_esi(esi_courier(), 1).unwrap();
        saved.items = Some(vec![Item {
            type_id: 34,
            quantity: 10,
            included: true,
        }]);
        let mut newer = Contract::from_esi(esi_courier(), 2).unwrap();
        assert!(saved.merge_from(newer.clone()));
        assert_eq!(saved.characters, vec![1, 2]);
        assert!(saved.items.is_some());
        // The same data again changes nothing.
        newer.characters = vec![1];
        assert!(!saved.merge_from(newer));
    }

    #[test]
    fn assigned_contracts_wait_for_you() {
        let mut esi = esi_courier();
        esi.kind = "item_exchange".into();
        esi.issuer_id = 77;
        esi.assignee_id = 2;
        let contract = Contract::from_esi(esi, 2).unwrap();
        assert_eq!(contract.side(&[1, 2]), (Side::AssignedToMe, Some(2)));
        assert_eq!(
            contract.stage(&[1, 2], contract.issued),
            Stage::AssignedToYou
        );
    }
}
