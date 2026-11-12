//! The Finance module's data: wallet journal entries, market trades and
//! loyalty points, as ESI sends them and as Athena keeps them.
//!
//! Also the pure calculations the tabs need (filters, sums per category),
//! kept apart from drawing so they can be tested.

use serde::Deserialize;

use super::categories::{Category, category};
use crate::modules::mining::gamelog::days_from_civil;

/// One line of a character's wallet journal.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntry {
    /// Whose wallet this line is in. The same `id` can show up in two
    /// wallets (one of your characters paying another), once in each.
    pub character_id: u64,
    pub id: i64,
    /// Unix seconds.
    pub when: i64,
    /// ESI's name for the kind of entry, e.g. "market_escrow".
    pub ref_type: String,
    /// Positive: money in. Negative: money out.
    pub amount: f64,
    /// The wallet balance right after this entry.
    pub balance: Option<f64>,
    pub first_party_id: Option<i64>,
    pub second_party_id: Option<i64>,
    /// What the entry is about, e.g. a market transaction or a station.
    pub context_id: Option<i64>,
    pub context_id_type: Option<String>,
    /// EVE's own one-line description.
    pub description: String,
    /// The text a player typed, for donations and the like.
    pub reason: Option<String>,
}

impl JournalEntry {
    pub fn category(&self) -> Category {
        category(&self.ref_type)
    }
}

/// One market buy or sell, which the journal links to by `journal_ref_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketTrade {
    pub character_id: u64,
    pub transaction_id: i64,
    pub journal_ref_id: i64,
    pub when: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub unit_price: f64,
    pub is_buy: bool,
    /// Who was on the other side.
    pub client_id: i64,
    /// A station or a player structure.
    pub location_id: i64,
}

/// What `/characters/{id}/wallet/journal` sends per line.
#[derive(Debug, Deserialize)]
pub struct EsiJournalLine {
    pub id: i64,
    pub date: String,
    pub ref_type: String,
    pub amount: Option<f64>,
    pub balance: Option<f64>,
    pub first_party_id: Option<i64>,
    pub second_party_id: Option<i64>,
    pub context_id: Option<i64>,
    pub context_id_type: Option<String>,
    #[serde(default)]
    pub description: String,
    pub reason: Option<String>,
}

impl EsiJournalLine {
    /// `None` if the date can't be read.
    pub fn into_entry(self, character_id: u64) -> Option<JournalEntry> {
        Some(JournalEntry {
            character_id,
            id: self.id,
            when: parse_esi_date(&self.date)?,
            ref_type: self.ref_type,
            amount: self.amount.unwrap_or(0.0),
            balance: self.balance,
            first_party_id: self.first_party_id,
            second_party_id: self.second_party_id,
            context_id: self.context_id,
            context_id_type: self.context_id_type,
            description: self.description,
            reason: self.reason.filter(|r| !r.trim().is_empty()),
        })
    }
}

/// What `/characters/{id}/wallet/transactions` sends per line.
#[derive(Debug, Deserialize)]
pub struct EsiTransaction {
    pub transaction_id: i64,
    pub journal_ref_id: i64,
    pub date: String,
    pub type_id: i64,
    pub quantity: i64,
    pub unit_price: f64,
    pub is_buy: bool,
    pub client_id: i64,
    pub location_id: i64,
}

impl EsiTransaction {
    pub fn into_trade(self, character_id: u64) -> Option<MarketTrade> {
        Some(MarketTrade {
            character_id,
            transaction_id: self.transaction_id,
            journal_ref_id: self.journal_ref_id,
            when: parse_esi_date(&self.date)?,
            type_id: self.type_id,
            quantity: self.quantity,
            unit_price: self.unit_price,
            is_buy: self.is_buy,
            client_id: self.client_id,
            location_id: self.location_id,
        })
    }
}

/// What `/characters/{id}/loyalty/points` sends per corporation.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LoyaltyPoints {
    pub corporation_id: i64,
    pub loyalty_points: i64,
}

/// What `/characters/{id}/location` sends (only the part we use).
#[derive(Debug, Deserialize)]
pub struct EsiLocation {
    pub solar_system_id: i64,
}

/// ESI's dates, "2026-11-12T08:43:24Z", as Unix seconds. ESI always
/// sends UTC.
pub fn parse_esi_date(text: &str) -> Option<i64> {
    let text = text.trim().trim_end_matches('Z');
    let (date, time) = text.split_once('T')?;
    let mut date_parts = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (
        date_parts.next()??,
        date_parts.next()??,
        date_parts.next()??,
    );
    // Seconds may carry a fraction ("24.123"); it's dropped.
    let time = time.split('.').next()?;
    let mut time_parts = time.split(':').map(|p| p.parse::<i64>().ok());
    let (hour, minute, second) = (
        time_parts.next()??,
        time_parts.next()??,
        time_parts.next()??,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Which way the money went, for the filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Both,
    Income,
    Expenses,
}

impl Direction {
    pub fn matches(self, amount: f64) -> bool {
        match self {
            Direction::Both => true,
            Direction::Income => amount > 0.0,
            Direction::Expenses => amount < 0.0,
        }
    }
}

/// Money in, money out, and how many entries that was.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Totals {
    pub income: f64,
    /// Negative (or zero).
    pub expenses: f64,
    pub entries: usize,
}

impl Totals {
    pub fn add(&mut self, amount: f64) {
        if amount > 0.0 {
            self.income += amount;
        } else {
            self.expenses += amount;
        }
        self.entries += 1;
    }

    pub fn net(&self) -> f64 {
        self.income + self.expenses
    }
}

/// One piece of the Overview ring.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    /// The category, or (inside a category) the ref_type.
    pub key: SliceKey,
    /// Always positive: income, or expenses without the minus.
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SliceKey {
    Category(Category),
    RefType(String),
}

/// The ring's pieces for `entries`, biggest first: per category, or per
/// ref_type inside `inside` when the user clicked into one.
pub fn slices<'a>(
    entries: impl Iterator<Item = &'a JournalEntry>,
    direction: Direction,
    inside: Option<Category>,
) -> Vec<Slice> {
    let mut sums: std::collections::HashMap<SliceKey, f64> = std::collections::HashMap::new();
    for entry in entries {
        if direction == Direction::Both || !direction.matches(entry.amount) {
            continue;
        }
        let key = match inside {
            None => SliceKey::Category(entry.category()),
            Some(cat) if entry.category() == cat => SliceKey::RefType(entry.ref_type.clone()),
            Some(_) => continue,
        };
        *sums.entry(key).or_insert(0.0) += entry.amount.abs();
    }
    let mut out: Vec<Slice> = sums
        .into_iter()
        .filter(|(_, amount)| *amount > 0.0)
        .map(|(key, amount)| Slice { key, amount })
        .collect();
    // Biggest first; ties by name so the order never flickers.
    out.sort_by(|a, b| {
        b.amount
            .total_cmp(&a.amount)
            .then_with(|| format!("{:?}", a.key).cmp(&format!("{:?}", b.key)))
    });
    out
}

/// Short ISK for cards and the ring: "9.36 b", "665 m", "54.1 k". Like the
/// Mining tab: k from 10,000, m from a million, b from a billion.
pub fn short_isk(value: f64) -> String {
    let abs = value.abs();
    let (scaled, unit) = if abs >= 1e9 {
        (value / 1e9, " b")
    } else if abs >= 1e6 {
        (value / 1e6, " m")
    } else if abs >= 1e4 {
        (value / 1e3, " k")
    } else {
        return format!("{value:.0}");
    };
    // Three significant digits: 9.36 b, 94.3 m, 665 m.
    let scaled_abs = scaled.abs();
    if scaled_abs >= 100.0 {
        format!("{scaled:.0}{unit}")
    } else if scaled_abs >= 10.0 {
        format!("{scaled:.1}{unit}")
    } else {
        format!("{scaled:.2}{unit}")
    }
}

/// Full ISK with thousands separators: "-51,036,000", "+3,160,269,611".
/// `sign` adds a "+" to positive numbers.
pub fn full_isk(value: f64, sign: bool) -> String {
    let rounded = value.round() as i64;
    let digits = rounded.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let prefix = if rounded < 0 {
        "-"
    } else if sign && rounded > 0 {
        "+"
    } else {
        ""
    };
    format!("{prefix}{grouped}")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn entry(id: i64, ref_type: &str, amount: f64) -> JournalEntry {
        JournalEntry {
            character_id: 1,
            id,
            when: 1_794_000_000 + id,
            ref_type: ref_type.to_string(),
            amount,
            balance: None,
            first_party_id: None,
            second_party_id: None,
            context_id: None,
            context_id_type: None,
            description: String::new(),
            reason: None,
        }
    }

    #[test]
    fn esi_dates() {
        assert_eq!(parse_esi_date("1970-01-01T00:00:00Z"), Some(0));
        // `date -u -d "2026-11-08 16:49:14" +%s`
        assert_eq!(parse_esi_date("2026-11-08T16:49:14Z"), Some(1_794_156_554));
        assert_eq!(
            parse_esi_date("2026-11-08T16:49:14.250Z"),
            Some(1_794_156_554)
        );
        assert_eq!(parse_esi_date("yesterday"), None);
        assert_eq!(parse_esi_date("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn journal_line_from_esi_json() {
        let json = r#"[{"amount":-51036000.0,"balance":9361614047.5,"context_id":123,
            "context_id_type":"market_transaction_id","date":"2026-11-12T07:43:24Z",
            "description":"Market escrow release","first_party_id":2112625428,
            "id":23011445566,"ref_type":"market_escrow","second_party_id":1000132}]"#;
        let lines: Vec<EsiJournalLine> = serde_json::from_str(json).unwrap();
        let entry = lines.into_iter().next().unwrap().into_entry(7).unwrap();
        assert_eq!(entry.character_id, 7);
        assert_eq!(entry.amount, -51_036_000.0);
        assert_eq!(entry.category(), Category::Trade);
        assert_eq!(entry.reason, None);
    }

    #[test]
    fn slices_per_category_then_per_type() {
        let entries = [
            entry(1, "bounty_prizes", 60.0),
            entry(2, "ess_escrow_transfer", 40.0),
            entry(3, "market_transaction", 300.0),
            entry(4, "market_escrow", -50.0),
            entry(5, "brokers_fee", -5.0),
        ];
        let top = slices(entries.iter(), Direction::Income, None);
        assert_eq!(
            top,
            [
                Slice {
                    key: SliceKey::Category(Category::Trade),
                    amount: 300.0
                },
                Slice {
                    key: SliceKey::Category(Category::Bounties),
                    amount: 100.0
                },
            ]
        );
        let bounties = slices(entries.iter(), Direction::Income, Some(Category::Bounties));
        assert_eq!(bounties.len(), 2);
        assert_eq!(
            bounties[0].key,
            SliceKey::RefType("bounty_prizes".to_string())
        );
        let spent = slices(entries.iter(), Direction::Expenses, None);
        assert_eq!(spent[0].amount, 50.0);
        assert_eq!(spent[1].amount, 5.0);
    }

    #[test]
    fn totals_split_income_and_expenses() {
        let mut totals = Totals::default();
        for amount in [100.0, -30.0, 5.0] {
            totals.add(amount);
        }
        assert_eq!(totals.income, 105.0);
        assert_eq!(totals.expenses, -30.0);
        assert_eq!(totals.net(), 75.0);
        assert_eq!(totals.entries, 3);
    }

    #[test]
    fn isk_formats() {
        assert_eq!(short_isk(9_364_000_000.0), "9.36 b");
        assert_eq!(short_isk(665_200_000.0), "665 m");
        assert_eq!(short_isk(94_300_000.0), "94.3 m");
        assert_eq!(short_isk(54_100.0), "54.1 k");
        assert_eq!(short_isk(9_999.0), "9999");
        assert_eq!(short_isk(-2_000_000_000.0), "-2.00 b");
        assert_eq!(full_isk(-51_036_000.0, true), "-51,036,000");
        assert_eq!(full_isk(3_160_269_611.0, true), "+3,160,269,611");
        assert_eq!(full_isk(999.0, false), "999");
        assert_eq!(full_isk(0.0, true), "0");
    }
}
