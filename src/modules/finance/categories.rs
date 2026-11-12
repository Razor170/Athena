//! Sorting wallet journal entries into the categories of the Overview ring
//! and the Types filter.
//!
//! ESI names every journal entry with a `ref_type`, such as
//! `market_escrow` or `bounty_prizes` (about 160 of them; see
//! `/characters/{character_id}/wallet/journal` in ESI's OpenAPI). Each
//! belongs to exactly one [`Category`]. A `ref_type` that ESI adds later
//! lands in [`Category::Other`] until it's listed here.

use eframe::egui::Color32;

/// The groups the Overview ring shows. The order here (`Ord`) is the order
/// of the Types filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Trade,
    Bounties,
    Missions,
    Industry,
    Planetary,
    CorpAlliance,
    Transfers,
    TaxesFees,
    Other,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Trade => "Trade",
            Category::Bounties => "Bounties",
            Category::Missions => "Missions & Rewards",
            Category::Industry => "Industry",
            Category::Planetary => "Planetary",
            Category::CorpAlliance => "Corp & Alliance",
            Category::Transfers => "Transfers",
            Category::TaxesFees => "Taxes & Fees",
            Category::Other => "Other",
        }
    }

    /// The category's color in the ring and the filter.
    pub fn color(self) -> Color32 {
        match self {
            Category::Trade => Color32::from_rgb(0x3f, 0xb8, 0xa0),
            Category::Bounties => Color32::from_rgb(0xd9, 0x53, 0x4f),
            Category::Missions => Color32::from_rgb(0xe0, 0x87, 0x3a),
            Category::Industry => Color32::from_rgb(0x9b, 0x7b, 0xe0),
            Category::Planetary => Color32::from_rgb(0x5f, 0xa8, 0xe0),
            Category::CorpAlliance => Color32::from_rgb(0x6d, 0x6f, 0xd9),
            Category::Transfers => Color32::from_rgb(0xe3, 0xc9, 0x4a),
            Category::TaxesFees => Color32::from_rgb(0xc0, 0x6f, 0xa8),
            Category::Other => Color32::from_rgb(0x8a, 0x8a, 0x8a),
        }
    }
}

/// The category of a journal `ref_type`.
pub fn category(ref_type: &str) -> Category {
    // Exact names first, then a few patterns for the rest.
    match ref_type {
        "market_transaction"
        | "market_escrow"
        | "contract_price"
        | "contract_price_payment_corp"
        | "contract_auction_bid"
        | "contract_auction_bid_corp"
        | "contract_auction_bid_refund"
        | "contract_auction_sold"
        | "contract_collateral"
        | "contract_collateral_deposited_corp"
        | "contract_collateral_payout"
        | "contract_collateral_refund"
        | "contract_deposit"
        | "contract_deposit_corp"
        | "contract_deposit_refund"
        | "contract_reversal"
        | "contract_reward"
        | "contract_reward_deposited"
        | "contract_reward_deposited_corp"
        | "contract_reward_refund"
        | "courier_mission_escrow"
        | "item_trader_payment"
        | "external_trade_delivery"
        | "external_trade_freeze"
        | "external_trade_thaw"
        | "lp_store"
        | "cosmetic_market_component_item_purchase"
        | "cosmetic_market_skin_purchase"
        | "cosmetic_market_skin_sale"
        | "cosmetic_market_skin_transaction" => Category::Trade,

        "bounty"
        | "bounty_prize"
        | "bounty_prizes"
        | "bounty_reimbursement"
        | "bounty_surcharge"
        | "ess_escrow_transfer" => Category::Bounties,

        "agent_donation"
        | "agent_location_services"
        | "agent_miscellaneous"
        | "agent_mission_collateral_paid"
        | "agent_mission_collateral_refunded"
        | "agent_mission_reward"
        | "agent_mission_time_bonus_reward"
        | "agent_security_services"
        | "agent_services_rendered"
        | "agents_preward"
        | "mission_completion"
        | "mission_cost"
        | "mission_expiration"
        | "mission_reward"
        | "achievement_category_milestone_reward"
        | "achievement_milestone_reward"
        | "air_career_program_reward"
        | "campaign_objective_isk_reward"
        | "daily_challenge_reward"
        | "daily_goal_payouts"
        | "freelance_jobs_reward"
        | "insurgency_corruption_contribution_reward"
        | "insurgency_suppression_contribution_reward"
        | "milestone_reward_payment"
        | "operation_bonus"
        | "opportunity_reward"
        | "project_discovery_reward"
        | "project_payouts"
        | "resource_wars_reward"
        | "season_challenge_reward" => Category::Missions,

        "industry_job_tax"
        | "industry_security_tax"
        | "manufacturing"
        | "copying"
        | "reaction"
        | "researching_material_productivity"
        | "researching_technology"
        | "researching_time_productivity"
        | "reverse_engineering"
        | "reprocessing_tax"
        | "datacore_fee"
        | "factory_slot_rental_fee" => Category::Industry,

        "planetary_construction"
        | "planetary_export_tax"
        | "planetary_import_tax"
        | "skyhook_claim_fee" => Category::Planetary,

        "player_donation" | "player_trading" | "gm_cash_transfer" | "inheritance" => {
            Category::Transfers
        }

        "brokers_fee"
        | "transaction_tax"
        | "contract_brokers_fee"
        | "contract_brokers_fee_corp"
        | "contract_sales_tax"
        | "contract_deposit_sales_tax" => Category::TaxesFees,

        _ if ref_type.starts_with("corporation_")
            || ref_type.starts_with("corporate_")
            || ref_type.starts_with("alliance_")
            || ref_type.starts_with("war_")
            || ref_type.starts_with("medal_")
            || matches!(
                ref_type,
                "shares"
                    | "office_rental_fee"
                    | "sovereignity_bill"
                    | "infrastructure_hub_maintenance"
            ) =>
        {
            Category::CorpAlliance
        }

        _ if ref_type.ends_with("_tax")
            || ref_type.ends_with("_fee")
            || ref_type.ends_with("_fine")
            || ref_type.ends_with("_fine_paid")
            || ref_type.ends_with("_broker_fee")
            || matches!(
                ref_type,
                "clone_activation"
                    | "clone_transfer"
                    | "repair_bill"
                    | "structure_gate_jump"
                    | "allignment_based_gate_toll"
                    | "upkeep_adjustment_fee"
            ) =>
        {
            Category::TaxesFees
        }

        _ => Category::Other,
    }
}

/// A `ref_type` as people read it: "market_escrow" -> "Market Escrow".
pub fn type_label(ref_type: &str) -> String {
    // A few that the simple rule gets wrong.
    match ref_type {
        "ess_escrow_transfer" => return "ESS Escrow Transfer".to_string(),
        "lp_store" => return "LP Store".to_string(),
        "cspa" => return "CSPA Charge".to_string(),
        "cspaofflinerefund" => return "CSPA Offline Refund".to_string(),
        "gm_cash_transfer" => return "GM Cash Transfer".to_string(),
        "gm_plex_fee_refund" => return "GM PLEX Fee Refund".to_string(),
        "dna_modification_fee" => return "DNA Modification Fee".to_string(),
        "npc_bounty_security_tax" => return "NPC Bounty Security Tax".to_string(),
        "redeemed_isk_token" => return "Redeemed ISK Token".to_string(),
        "allignment_based_gate_toll" => return "Alignment Based Gate Toll".to_string(),
        "sovereignity_bill" => return "Sovereignty Bill".to_string(),
        _ => {}
    }
    ref_type
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_types_land_where_expected() {
        assert_eq!(category("market_escrow"), Category::Trade);
        assert_eq!(category("contract_price_payment_corp"), Category::Trade);
        assert_eq!(category("bounty_prizes"), Category::Bounties);
        assert_eq!(category("ess_escrow_transfer"), Category::Bounties);
        assert_eq!(category("agent_mission_reward"), Category::Missions);
        assert_eq!(category("industry_job_tax"), Category::Industry);
        assert_eq!(category("planetary_export_tax"), Category::Planetary);
        assert_eq!(category("player_donation"), Category::Transfers);
        assert_eq!(
            category("corporation_account_withdrawal"),
            Category::CorpAlliance
        );
        assert_eq!(category("brokers_fee"), Category::TaxesFees);
        assert_eq!(category("contract_brokers_fee"), Category::TaxesFees);
        assert_eq!(category("jump_clone_activation_fee"), Category::TaxesFees);
        assert_eq!(
            category("bounty_prize_corporation_tax"),
            Category::TaxesFees
        );
        assert_eq!(category("insurance"), Category::Other);
        assert_eq!(category("something_new_from_ccp"), Category::Other);
    }

    #[test]
    fn labels_read_like_words() {
        assert_eq!(type_label("market_escrow"), "Market Escrow");
        assert_eq!(
            type_label("contract_price_payment_corp"),
            "Contract Price Payment Corp"
        );
        assert_eq!(type_label("ess_escrow_transfer"), "ESS Escrow Transfer");
        assert_eq!(type_label("bounty_prizes"), "Bounty Prizes");
    }
}
