//! Derived statistics models.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::EpochId;

/// Tier classification based on win rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    S,
    A,
    B,
    C,
    D,
}

impl Tier {
    /// Calculate tier from win rate.
    pub fn from_win_rate(win_rate: f64) -> Self {
        if win_rate >= 0.55 {
            Tier::S
        } else if win_rate >= 0.52 {
            Tier::A
        } else if win_rate >= 0.48 {
            Tier::B
        } else if win_rate >= 0.45 {
            Tier::C
        } else {
            Tier::D
        }
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tier::S => write!(f, "S"),
            Tier::A => write!(f, "A"),
            Tier::B => write!(f, "B"),
            Tier::C => write!(f, "C"),
            Tier::D => write!(f, "D"),
        }
    }
}

/// Placement count breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlacementCounts {
    pub first: u32,
    pub top_4: u32,
    pub top_10: u32,
    pub top_half: u32,
}

/// Detachment statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetachmentStats {
    pub name: String,
    pub count: u32,
    pub win_rate: f64,
}

/// Per-faction statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionStat {
    /// Faction name
    pub name: String,

    /// Tier classification
    pub tier: Tier,

    /// Number of players using this faction
    pub player_count: u32,

    /// Total games played
    pub games_played: u32,

    /// Event appearances
    pub event_appearances: u32,

    /// Wins
    pub wins: u32,

    /// Losses
    pub losses: u32,

    /// Draws
    pub draws: u32,

    /// Win rate (0.0 to 1.0)
    pub win_rate: f64,

    /// Change from previous epoch
    pub win_rate_delta: Option<f64>,

    /// Placement breakdown
    pub placement_counts: PlacementCounts,

    /// Podium rate (top_4 / player_count)
    pub podium_rate: f64,

    /// Meta share (player_count / total_players)
    pub meta_share: f64,

    /// Over-representation ratio
    pub over_representation: f64,

    /// Average placement percentile
    pub average_placement_percentile: f64,

    /// Players with 4-0 starts
    pub four_zero_starts: u32,

    /// Players with 5-0 starts
    pub five_zero_starts: u32,

    /// Top detachments
    pub top_detachments: Vec<DetachmentStats>,
}

impl FactionStat {
    /// Create a new FactionStat with calculated fields.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        player_count: u32,
        games_played: u32,
        event_appearances: u32,
        wins: u32,
        losses: u32,
        draws: u32,
        placement_counts: PlacementCounts,
        total_players: u32,
        total_top_4: u32,
    ) -> Self {
        let total_games = wins + losses + draws;
        let win_rate = if total_games > 0 {
            wins as f64 / total_games as f64
        } else {
            0.0
        };

        let tier = Tier::from_win_rate(win_rate);

        let podium_rate = if player_count > 0 {
            placement_counts.top_4 as f64 / player_count as f64
        } else {
            0.0
        };

        let meta_share = if total_players > 0 {
            player_count as f64 / total_players as f64
        } else {
            0.0
        };

        let over_representation = if meta_share > 0.0 && total_top_4 > 0 {
            let top_4_share = placement_counts.top_4 as f64 / total_top_4 as f64;
            top_4_share / meta_share
        } else {
            0.0
        };

        Self {
            name,
            tier,
            player_count,
            games_played,
            event_appearances,
            wins,
            losses,
            draws,
            win_rate,
            win_rate_delta: None,
            placement_counts,
            podium_rate,
            meta_share,
            over_representation,
            average_placement_percentile: 0.5, // Calculated separately
            four_zero_starts: 0,
            five_zero_starts: 0,
            top_detachments: Vec::new(),
        }
    }

    /// Set win rate delta from previous epoch.
    pub fn with_win_rate_delta(mut self, delta: f64) -> Self {
        self.win_rate_delta = Some(delta);
        self
    }
}

/// Date range for statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateRange {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

/// Totals for an epoch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpochTotals {
    pub events: u32,
    pub players: u32,
    pub games: u32,
}

/// Faction statistics for an epoch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionStats {
    /// Epoch ID
    pub epoch_id: EpochId,

    /// Epoch name
    pub epoch_name: String,

    /// When these stats were computed
    pub computed_at: DateTime<Utc>,

    /// Date range covered
    pub date_range: DateRange,

    /// Totals
    pub totals: EpochTotals,

    /// Per-faction statistics
    pub factions: Vec<FactionStat>,
}

impl FactionStats {
    /// Create new FactionStats.
    pub fn new(
        epoch_id: EpochId,
        epoch_name: String,
        date_range: DateRange,
        totals: EpochTotals,
        factions: Vec<FactionStat>,
    ) -> Self {
        Self {
            epoch_id,
            epoch_name,
            computed_at: Utc::now(),
            date_range,
            totals,
            factions,
        }
    }

    /// Get faction by name.
    pub fn get_faction(&self, name: &str) -> Option<&FactionStat> {
        self.factions
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// Get factions sorted by win rate (descending).
    pub fn sorted_by_win_rate(&self) -> Vec<&FactionStat> {
        let mut sorted: Vec<_> = self.factions.iter().collect();
        sorted.sort_by(|a, b| b.win_rate.partial_cmp(&a.win_rate).unwrap());
        sorted
    }

    /// Get factions in a specific tier.
    pub fn in_tier(&self, tier: Tier) -> Vec<&FactionStat> {
        self.factions.iter().filter(|f| f.tier == tier).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_from_win_rate() {
        assert_eq!(Tier::from_win_rate(0.60), Tier::S);
        assert_eq!(Tier::from_win_rate(0.55), Tier::S);
        assert_eq!(Tier::from_win_rate(0.54), Tier::A);
        assert_eq!(Tier::from_win_rate(0.52), Tier::A);
        assert_eq!(Tier::from_win_rate(0.50), Tier::B);
        assert_eq!(Tier::from_win_rate(0.48), Tier::B);
        assert_eq!(Tier::from_win_rate(0.46), Tier::C);
        assert_eq!(Tier::from_win_rate(0.45), Tier::C);
        assert_eq!(Tier::from_win_rate(0.40), Tier::D);
    }

    #[test]
    fn test_tier_display() {
        assert_eq!(format!("{}", Tier::S), "S");
        assert_eq!(format!("{}", Tier::D), "D");
    }

    #[test]
    fn test_faction_stat_creation() {
        let placements = PlacementCounts {
            first: 12,
            top_4: 38,
            top_10: 67,
            top_half: 134,
        };

        let stat = FactionStat::new(
            "Aeldari".to_string(),
            234, // player_count
            702, // games_played
            45,  // event_appearances
            379, // wins
            298, // losses
            25,  // draws
            placements,
            1856, // total_players
            152,  // total_top_4
        );

        assert_eq!(stat.name, "Aeldari");
        assert_eq!(stat.tier, Tier::A); // 379/702 = 0.539 (tier A: 52-55%)
        assert!(stat.win_rate > 0.53 && stat.win_rate < 0.55);
        assert!(stat.meta_share > 0.12 && stat.meta_share < 0.13);
        assert!(stat.over_representation > 1.0); // Over-represented in top 4
    }

    #[test]
    fn test_faction_stat_zero_games() {
        let stat = FactionStat::new(
            "Test".to_string(),
            0,
            0,
            0,
            0,
            0,
            0,
            PlacementCounts::default(),
            0,
            0,
        );

        assert_eq!(stat.win_rate, 0.0);
        assert_eq!(stat.tier, Tier::D);
        assert_eq!(stat.podium_rate, 0.0);
    }

    #[test]
    fn test_faction_stats_get_faction() {
        let stats = FactionStats::new(
            "epoch-123".into(),
            "Test Epoch".to_string(),
            DateRange {
                from: NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
                to: NaiveDate::from_ymd_opt(2025, 7, 14).unwrap(),
            },
            EpochTotals::default(),
            vec![FactionStat::new(
                "Aeldari".to_string(),
                100,
                300,
                10,
                180,
                110,
                10,
                PlacementCounts::default(),
                500,
                40,
            )],
        );

        assert!(stats.get_faction("Aeldari").is_some());
        assert!(stats.get_faction("aeldari").is_some()); // Case insensitive
        assert!(stats.get_faction("Space Marines").is_none());
    }

    #[test]
    fn test_faction_stats_sorted_by_win_rate() {
        let stats = FactionStats::new(
            "epoch-123".into(),
            "Test Epoch".to_string(),
            DateRange {
                from: NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
                to: NaiveDate::from_ymd_opt(2025, 7, 14).unwrap(),
            },
            EpochTotals::default(),
            vec![
                FactionStat::new(
                    "Low".to_string(),
                    100,
                    300,
                    10,
                    100,
                    190,
                    10, // 33% win rate
                    PlacementCounts::default(),
                    500,
                    40,
                ),
                FactionStat::new(
                    "High".to_string(),
                    100,
                    300,
                    10,
                    200,
                    90,
                    10, // 67% win rate
                    PlacementCounts::default(),
                    500,
                    40,
                ),
            ],
        );

        let sorted = stats.sorted_by_win_rate();
        assert_eq!(sorted[0].name, "High");
        assert_eq!(sorted[1].name, "Low");
    }

    #[test]
    fn test_faction_stats_serialization() {
        let stats = FactionStats::new(
            "epoch-123".into(),
            "Test Epoch".to_string(),
            DateRange {
                from: NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
                to: NaiveDate::from_ymd_opt(2025, 7, 14).unwrap(),
            },
            EpochTotals {
                events: 10,
                players: 100,
                games: 300,
            },
            vec![],
        );

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: FactionStats = serde_json::from_str(&json).unwrap();

        assert_eq!(stats.epoch_id, deserialized.epoch_id);
        assert_eq!(stats.totals.events, deserialized.totals.events);
    }
}
