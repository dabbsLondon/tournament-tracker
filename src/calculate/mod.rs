//! Statistics calculation engine.
//!
//! Computes derived metrics from stored tournament data:
//! - Faction win rates and tier rankings
//! - Unit frequency analysis
//! - Common combo detection
//! - Trend analysis across epochs

use crate::models::{PlacementCounts, Tier};

/// Calculate tier from win rate.
pub fn calculate_tier(win_rate: f64) -> Tier {
    Tier::from_win_rate(win_rate)
}

/// Calculate win rate from wins/losses/draws.
pub fn calculate_win_rate(wins: u32, losses: u32, draws: u32) -> f64 {
    let total = wins + losses + draws;
    if total == 0 {
        0.0
    } else {
        wins as f64 / total as f64
    }
}

/// Calculate over-representation ratio.
/// A ratio > 1.0 means the faction is over-represented in top placements.
pub fn calculate_over_representation(
    faction_top_4: u32,
    total_top_4: u32,
    faction_players: u32,
    total_players: u32,
) -> f64 {
    if total_players == 0 || total_top_4 == 0 || faction_players == 0 {
        return 0.0;
    }

    let meta_share = faction_players as f64 / total_players as f64;
    let top_4_share = faction_top_4 as f64 / total_top_4 as f64;

    top_4_share / meta_share
}

/// Calculate podium rate (top 4 finishes / total players).
pub fn calculate_podium_rate(top_4: u32, player_count: u32) -> f64 {
    if player_count == 0 {
        0.0
    } else {
        top_4 as f64 / player_count as f64
    }
}

/// Aggregate placement counts from individual placements.
pub fn aggregate_placements(ranks: &[u32], total_players_per_event: &[u32]) -> PlacementCounts {
    let mut counts = PlacementCounts::default();

    for (rank, &total) in ranks.iter().zip(total_players_per_event.iter()) {
        if *rank == 1 {
            counts.first += 1;
        }
        if *rank <= 4 {
            counts.top_4 += 1;
        }
        if *rank <= 10 {
            counts.top_10 += 1;
        }
        if total > 0 && *rank <= total / 2 {
            counts.top_half += 1;
        }
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_tier() {
        assert_eq!(calculate_tier(0.60), Tier::S);
        assert_eq!(calculate_tier(0.53), Tier::A);
        assert_eq!(calculate_tier(0.50), Tier::B);
        assert_eq!(calculate_tier(0.46), Tier::C);
        assert_eq!(calculate_tier(0.40), Tier::D);
    }

    #[test]
    fn test_calculate_win_rate() {
        assert!((calculate_win_rate(5, 1, 0) - 0.833).abs() < 0.01);
        assert_eq!(calculate_win_rate(0, 0, 0), 0.0);
        assert_eq!(calculate_win_rate(3, 3, 0), 0.5);
    }

    #[test]
    fn test_calculate_over_representation() {
        // Faction has 10% of players but 20% of top 4 finishes = 2.0 over-rep
        let over_rep = calculate_over_representation(20, 100, 100, 1000);
        assert!((over_rep - 2.0).abs() < 0.01);

        // Faction has 20% of players and 20% of top 4 = 1.0 (neutral)
        let over_rep = calculate_over_representation(20, 100, 200, 1000);
        assert!((over_rep - 1.0).abs() < 0.01);

        // Edge case: no players
        assert_eq!(calculate_over_representation(0, 100, 0, 1000), 0.0);
    }

    #[test]
    fn test_calculate_podium_rate() {
        assert!((calculate_podium_rate(10, 100) - 0.10).abs() < 0.01);
        assert_eq!(calculate_podium_rate(0, 100), 0.0);
        assert_eq!(calculate_podium_rate(10, 0), 0.0);
    }

    #[test]
    fn test_aggregate_placements() {
        let ranks = vec![1, 2, 3, 5, 8, 15, 25];
        let totals = vec![50, 50, 50, 50, 50, 50, 50];

        let counts = aggregate_placements(&ranks, &totals);

        assert_eq!(counts.first, 1);
        assert_eq!(counts.top_4, 3); // 1, 2, 3
        assert_eq!(counts.top_10, 5); // 1, 2, 3, 5, 8
        assert_eq!(counts.top_half, 7); // All including 25 (25 <= 50/2)
    }
}
