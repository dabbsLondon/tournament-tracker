//! Army list model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{ArmyListId, Confidence, EntityId};

/// A unit in an army list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    /// Unit name
    pub name: String,

    /// Number of models
    pub count: u32,

    /// Points cost
    pub points: Option<u32>,

    /// Selected wargear/upgrades
    pub wargear: Vec<String>,

    /// Keywords (if known)
    pub keywords: Vec<String>,
}

impl Unit {
    /// Create a new unit.
    pub fn new(name: String, count: u32) -> Self {
        Self {
            name,
            count,
            points: None,
            wargear: Vec::new(),
            keywords: Vec::new(),
        }
    }

    /// Builder method to set points.
    pub fn with_points(mut self, points: u32) -> Self {
        self.points = Some(points);
        self
    }

    /// Builder method to add wargear.
    pub fn with_wargear(mut self, wargear: Vec<String>) -> Self {
        self.wargear = wargear;
        self
    }

    /// Builder method to add keywords.
    pub fn with_keywords(mut self, keywords: Vec<String>) -> Self {
        self.keywords = keywords;
        self
    }
}

/// A normalized army list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArmyList {
    /// Unique identifier
    pub id: ArmyListId,

    /// Faction
    pub faction: String,

    /// Subfaction
    pub subfaction: Option<String>,

    /// Detachment name
    pub detachment: Option<String>,

    /// Total points
    pub total_points: u32,

    /// Units in the list
    pub units: Vec<Unit>,

    /// Original raw text (for audit)
    pub raw_text: String,

    /// Source URL
    pub source_url: Option<String>,

    /// When this record was created
    pub created_at: DateTime<Utc>,

    /// Confidence level of the extraction
    pub extraction_confidence: Confidence,

    /// Whether this needs manual review
    pub needs_review: bool,

    /// Path to the raw source file
    pub raw_source_path: Option<PathBuf>,
}

impl ArmyList {
    /// Create a new ArmyList with auto-generated ID.
    pub fn new(faction: String, total_points: u32, units: Vec<Unit>, raw_text: String) -> Self {
        // Generate ID from faction, detachment, sorted unit names, and total points
        let mut unit_names: Vec<_> = units.iter().map(|u| u.name.as_str()).collect();
        unit_names.sort();
        let units_str = unit_names.join(",");

        let id = EntityId::generate(&[
            &faction,
            "", // detachment placeholder
            &units_str,
            &total_points.to_string(),
        ]);

        Self {
            id,
            faction,
            subfaction: None,
            detachment: None,
            total_points,
            units,
            raw_text,
            source_url: None,
            created_at: Utc::now(),
            extraction_confidence: Confidence::default(),
            needs_review: false,
            raw_source_path: None,
        }
    }

    /// Regenerate ID with detachment included.
    pub fn with_detachment(mut self, detachment: String) -> Self {
        self.detachment = Some(detachment.clone());

        let mut unit_names: Vec<_> = self.units.iter().map(|u| u.name.as_str()).collect();
        unit_names.sort();
        let units_str = unit_names.join(",");

        self.id = EntityId::generate(&[
            &self.faction,
            &detachment,
            &units_str,
            &self.total_points.to_string(),
        ]);

        self
    }

    /// Builder method to set subfaction.
    pub fn with_subfaction(mut self, subfaction: String) -> Self {
        self.subfaction = Some(subfaction);
        self
    }

    /// Builder method to set source URL.
    pub fn with_source_url(mut self, url: String) -> Self {
        self.source_url = Some(url);
        self
    }

    /// Builder method to set confidence.
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.extraction_confidence = confidence;
        self.needs_review = confidence.needs_review();
        self
    }

    /// Builder method to set raw source path.
    pub fn with_raw_source_path(mut self, path: PathBuf) -> Self {
        self.raw_source_path = Some(path);
        self
    }

    /// Get unit names for analysis.
    pub fn unit_names(&self) -> Vec<&str> {
        self.units.iter().map(|u| u.name.as_str()).collect()
    }

    /// Check if list contains a specific unit.
    pub fn contains_unit(&self, name: &str) -> bool {
        self.units.iter().any(|u| u.name.eq_ignore_ascii_case(name))
    }

    /// Count occurrences of a unit.
    pub fn count_unit(&self, name: &str) -> u32 {
        self.units
            .iter()
            .filter(|u| u.name.eq_ignore_ascii_case(name))
            .map(|u| u.count)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_units() -> Vec<Unit> {
        vec![
            Unit::new("Yvraine".to_string(), 1).with_points(120),
            Unit::new("Wraithguard".to_string(), 5).with_points(180),
            Unit::new("Wave Serpent".to_string(), 1).with_points(120),
        ]
    }

    #[test]
    fn test_unit_creation() {
        let unit = Unit::new("Wraithguard".to_string(), 5)
            .with_points(180)
            .with_wargear(vec!["Wraithcannons".to_string()])
            .with_keywords(vec!["Infantry".to_string(), "Wraith Construct".to_string()]);

        assert_eq!(unit.name, "Wraithguard");
        assert_eq!(unit.count, 5);
        assert_eq!(unit.points, Some(180));
        assert_eq!(unit.wargear.len(), 1);
        assert_eq!(unit.keywords.len(), 2);
    }

    #[test]
    fn test_army_list_creation() {
        let units = create_test_units();
        let list = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            units,
            "Raw list text...".to_string(),
        );

        assert_eq!(list.faction, "Aeldari");
        assert_eq!(list.total_points, 2000);
        assert_eq!(list.units.len(), 3);
        assert!(!list.id.as_str().is_empty());
    }

    #[test]
    fn test_army_list_builder() {
        let units = create_test_units();
        let list = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            units,
            "Raw list text...".to_string(),
        )
        .with_subfaction("Ynnari".to_string())
        .with_detachment("Seer Council".to_string())
        .with_confidence(Confidence::High);

        assert_eq!(list.subfaction, Some("Ynnari".to_string()));
        assert_eq!(list.detachment, Some("Seer Council".to_string()));
        assert_eq!(list.extraction_confidence, Confidence::High);
    }

    #[test]
    fn test_army_list_contains_unit() {
        let units = create_test_units();
        let list = ArmyList::new("Aeldari".to_string(), 2000, units, "".to_string());

        assert!(list.contains_unit("Wraithguard"));
        assert!(list.contains_unit("wraithguard")); // Case insensitive
        assert!(!list.contains_unit("Fire Prism"));
    }

    #[test]
    fn test_army_list_count_unit() {
        let units = vec![
            Unit::new("Wraithguard".to_string(), 5),
            Unit::new("Wraithguard".to_string(), 5),
            Unit::new("Wave Serpent".to_string(), 1),
        ];
        let list = ArmyList::new("Aeldari".to_string(), 2000, units, "".to_string());

        assert_eq!(list.count_unit("Wraithguard"), 10);
        assert_eq!(list.count_unit("Wave Serpent"), 1);
        assert_eq!(list.count_unit("Fire Prism"), 0);
    }

    #[test]
    fn test_army_list_unit_names() {
        let units = create_test_units();
        let list = ArmyList::new("Aeldari".to_string(), 2000, units, "".to_string());

        let names = list.unit_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"Yvraine"));
    }

    #[test]
    fn test_army_list_id_deterministic() {
        let units1 = create_test_units();
        let units2 = create_test_units();

        let list1 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            units1,
            "Different raw text 1".to_string(),
        );

        let list2 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            units2,
            "Different raw text 2".to_string(),
        );

        // Same units should produce same ID (raw_text not in ID)
        assert_eq!(list1.id, list2.id);
    }

    #[test]
    fn test_army_list_serialization() {
        let units = create_test_units();
        let list = ArmyList::new("Aeldari".to_string(), 2000, units, "Raw text".to_string());

        let json = serde_json::to_string(&list).unwrap();
        let deserialized: ArmyList = serde_json::from_str(&json).unwrap();

        assert_eq!(list.id, deserialized.id);
        assert_eq!(list.faction, deserialized.faction);
        assert_eq!(list.units.len(), deserialized.units.len());
    }
}
