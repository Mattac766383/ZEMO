use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }
    };
}

id_type!(ActorId);
id_type!(ArtifactId);
id_type!(ConsentGrantId);
id_type!(EntityId);
id_type!(ExecutionId);
id_type!(FileId);
id_type!(FileVersionId);
id_type!(JobId);
id_type!(LearningObservationId);
id_type!(ModelReleaseId);
id_type!(OperationStepId);
id_type!(OrganizationRevisionId);
id_type!(PlanId);
id_type!(ProposalId);
id_type!(ProposalItemId);
id_type!(ProposalNodeId);
id_type!(ProposalOverrideId);
id_type!(RootId);
id_type!(RuleId);
id_type!(RuleSuggestionId);
id_type!(ScanId);
id_type!(TaxonomyId);
id_type!(WorkspaceId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_round_trip_as_strings() {
        let original = WorkspaceId::new();
        let parsed = original.to_string().parse::<WorkspaceId>();

        assert_eq!(parsed, Ok(original));
    }
}
