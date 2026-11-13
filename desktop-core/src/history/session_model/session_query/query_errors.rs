//! Why a session search cannot run: a qualifier without a value, an unknown
//! value, or two choices that cannot both hold.
use super::LocalSessionQuery;

impl LocalSessionQuery {
    /// Two values of `field` (`is` or `upload`) no session can have at once.
    pub(super) fn opposite(field: &str, left: &str, right: &str) -> bool {
        if !matches!(field, "is" | "upload") {
            return false;
        }
        let (left, right) = (left.to_lowercase(), right.to_lowercase());
        let pair = |a: &str, b: &str| (left == a && right == b) || (left == b && right == a);
        match field {
            "is" => pair("active", "inactive") || pair("on-relay", "off-relay"),
            "upload" => {
                pair("local", "in-progress")
                    || (left != right
                        && (matches!(left.as_str(), "none" | "discovering")
                            || matches!(right.as_str(), "none" | "discovering")))
            }
            _ => false,
        }
    }

    pub(super) fn opposite_selection_error(&self) -> Option<String> {
        for term in self.terms.iter().filter(|term| !term.excluded) {
            if let Some(field) = term.field.as_deref() {
                if !matches!(field, "is" | "upload") {
                    continue;
                }
                if self.terms.iter().any(|other| {
                    !other.excluded
                        && other.field == term.field
                        && Self::opposite(field, &term.value, &other.value)
                }) {
                    return Some(
                        if field == "is"
                            && ["on-relay", "off-relay"]
                                .contains(&term.value.to_lowercase().as_str())
                        {
                            "Choose either receives messages or does not receive messages."
                        } else if field == "is" {
                            "Choose either is:active or is:inactive."
                        } else {
                            "Choose compatible upload filters; opposite states cannot be combined."
                        }
                        .into(),
                    );
                }
            }
        }
        None
    }

    pub fn error(&self) -> Option<String> {
        if let Some(error) = self.opposite_selection_error() {
            return Some(error);
        }
        for term in &self.terms {
            let Some(field) = term.field.as_deref() else {
                continue;
            };
            if term.value.is_empty() {
                return Some(format!("Add a value after {field}:"));
            }
            if ["updated", "after", "before"].contains(&field) && term.dates.is_none() {
                return Some("Use a date like updated:2026-10-01, updated:>=2026-10-01, or updated:2026-10-01..2026-10-11 (UTC).".into());
            }
            if field == "is"
                && !["active", "inactive", "on-relay", "off-relay"]
                    .contains(&term.value.to_lowercase().as_str())
            {
                return Some("Use is:active, is:inactive, is:on-relay, or is:off-relay.".into());
            }
            if field == "upload"
                && ![
                    "in-progress",
                    "none",
                    "local",
                    "discovering",
                    "preparing",
                    "waiting",
                    "queued",
                    "uploading",
                    "uploaded",
                    "checking",
                    "paused",
                    "blocked",
                    "unknown",
                ]
                .contains(&term.value.to_lowercase().as_str())
            {
                return Some(
                    "Use upload:uploaded, upload:local, upload:in-progress, or upload:discovering."
                        .into(),
                );
            }
        }
        None
    }
}
