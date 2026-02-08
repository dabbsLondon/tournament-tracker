//! Confidence levels for AI-extracted data.

use serde::{Deserialize, Serialize};

/// Confidence level of AI extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// AI confident, fact-checker verified
    High,
    /// AI somewhat confident, minor discrepancies
    #[default]
    Medium,
    /// AI uncertain, fact-checker flagged issues
    Low,
}

impl Confidence {
    /// Returns true if confidence is high enough for automatic storage.
    pub fn is_acceptable(&self) -> bool {
        matches!(self, Confidence::High | Confidence::Medium)
    }

    /// Returns true if this confidence level requires review.
    pub fn needs_review(&self) -> bool {
        matches!(self, Confidence::Low)
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Confidence::High => write!(f, "high"),
            Confidence::Medium => write!(f, "medium"),
            Confidence::Low => write!(f, "low"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confidence_acceptable() {
        assert!(Confidence::High.is_acceptable());
        assert!(Confidence::Medium.is_acceptable());
        assert!(!Confidence::Low.is_acceptable());
    }

    #[test]
    fn test_confidence_needs_review() {
        assert!(!Confidence::High.needs_review());
        assert!(!Confidence::Medium.needs_review());
        assert!(Confidence::Low.needs_review());
    }

    #[test]
    fn test_confidence_serialization() {
        let high = Confidence::High;
        let json = serde_json::to_string(&high).unwrap();
        assert_eq!(json, "\"high\"");

        let deserialized: Confidence = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, Confidence::High);
    }

    #[test]
    fn test_confidence_display() {
        assert_eq!(format!("{}", Confidence::High), "high");
        assert_eq!(format!("{}", Confidence::Medium), "medium");
        assert_eq!(format!("{}", Confidence::Low), "low");
    }
}
