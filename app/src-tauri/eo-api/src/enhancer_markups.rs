//! The markup paid for enhancers, set per kind and tier.
//!
//! Enhancers are bought on their own and fitted to whichever weapon takes
//! them, so what was paid over their TT is a fact of the kind and tier, not
//! of a weapon. A break is priced at the markup in force for its kind and
//! tier when it broke; a new value applies from when it is set, and never
//! moves a break already kept. A tier with none is priced at its TT, and
//! the break says the markup is not set. Weapon Damage Enhancers today; the
//! other kinds join as rows of the same table.

use eo_services::enhancer_breaks::{
    heal_prices, set_tier_markup, tt_from_game_data, EnhancerKind, TierMarkups, TIERS,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::account::iso;
use crate::Nullable;
use crate::{Api, ApiError};

/// The highest markup a tier may be set to, in percent.
pub const MAX_ENHANCER_MARKUP_PERCENT: f64 = 100_000.0;

/// A kind of enhancer whose markup is set per tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EnhancerMarkupKind {
    WeaponDamage,
}

impl EnhancerMarkupKind {
    fn kind(self) -> EnhancerKind {
        match self {
            EnhancerMarkupKind::WeaponDamage => EnhancerKind::WeaponDamage,
        }
    }

    fn of(kind: EnhancerKind) -> EnhancerMarkupKind {
        match kind {
            EnhancerKind::WeaponDamage => EnhancerMarkupKind::WeaponDamage,
        }
    }

    /// The catalogue's name for one tier of the kind.
    fn catalogue_name(self, tier: u8) -> String {
        match self {
            EnhancerMarkupKind::WeaponDamage => format!("T{tier} Weapon Damage Enhancer"),
        }
    }
}

/// One tier of one kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnhancerTierMarkup {
    pub kind: EnhancerMarkupKind,
    pub tier: u8,
    /// The enhancer's TT in PED, from the game data; `null` when unlisted.
    pub tt_ped: Nullable<f64>,
    /// The markup paid, in percent of TT, in force from now on; `null`
    /// prices its breaks at TT.
    pub markup_percent: Nullable<f64>,
    /// When it was set; `null` when it never was.
    pub set_at: Nullable<String>,
}

/// Every tier of every kind whose markup is set per tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnhancerMarkups {
    /// By kind, then tier.
    pub tiers: Vec<EnhancerTierMarkup>,
}

/// One tier's markup, set from now on.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnhancerTierMarkupInput {
    pub kind: EnhancerMarkupKind,
    pub tier: u8,
    /// In percent, from 100; `null` prices the tier's breaks at TT from
    /// now on.
    pub markup_percent: Option<f64>,
}

/// Whether a markup may be set: from 100% to the highest, rounded to the
/// hundredth.
fn valid_markup(markup: Option<f64>) -> Result<Option<f64>, ApiError> {
    match markup {
        None => Ok(None),
        Some(percent)
            if percent.is_finite() && (100.0..=MAX_ENHANCER_MARKUP_PERCENT).contains(&percent) =>
        {
            Ok(Some((percent * 100.0).round() / 100.0))
        }
        Some(_) => Err(ApiError::bad_request(
            "An enhancer's markup is from 100% up",
        )),
    }
}

impl Api {
    /// The markup paid for each tier of each kind of enhancer.
    pub async fn enhancer_markups(&self) -> Result<EnhancerMarkups, ApiError> {
        let markups = self
            .db
            .with_reader(|conn| Ok(TierMarkups::read(conn)?))
            .await
            .map_err(ApiError::internal("enhancer markups"))?;
        let mut tiers = Vec::new();
        for kind in EnhancerKind::ALL {
            let wire = EnhancerMarkupKind::of(kind);
            for (tier, markup, set_at) in markups.current(kind) {
                tiers.push(EnhancerTierMarkup {
                    kind: wire,
                    tier,
                    tt_ped: tt_from_game_data(&self.game_data, &wire.catalogue_name(tier))
                        .map(|tt| tt.value())
                        .into(),
                    markup_percent: markup.into(),
                    set_at: set_at.map(iso).into(),
                });
            }
        }
        Ok(EnhancerMarkups { tiers })
    }

    /// Set the markup paid for one tier from now on, price again every
    /// kept break it moves (none broke before now, so only a break dated
    /// ahead of the clock can), and answer with every tier.
    pub async fn enhancer_markup_set(
        &self,
        input: EnhancerTierMarkupInput,
    ) -> Result<EnhancerMarkups, ApiError> {
        if !TIERS.contains(&input.tier) {
            return Err(ApiError::bad_request("An enhancer's tier is from 1 to 10"));
        }
        let markup = valid_markup(input.markup_percent)?;
        let kind = input.kind.kind();
        let tier = input.tier;
        let now = eo_services::time::naive_to_epoch(self.clock.now());
        let game_data = self.game_data.clone();
        self.db
            .with_writer(move |conn| {
                set_tier_markup(conn, kind, tier, markup, now)?;
                let tt = |name: &str| tt_from_game_data(&game_data, name);
                heal_prices(conn, &tt)
            })
            .await
            .map_err(ApiError::internal("enhancer markup"))?;
        self.enhancer_markups().await
    }
}

#[cfg(test)]
mod tests {
    use super::valid_markup;

    #[test]
    fn an_enhancer_markup_is_from_one_hundred_percent() {
        assert_eq!(valid_markup(None).unwrap(), None);
        assert_eq!(valid_markup(Some(135.555)).unwrap(), Some(135.56));
        assert!(valid_markup(Some(99.0)).is_err());
        assert!(valid_markup(Some(f64::INFINITY)).is_err());
    }
}
