use crate::{ApplicationError, ScannerApplicationService};
use domain::WorkspaceId;
use persistence::{
    ReviewAction, ReviewItemRecord, ReviewPageRecord, ReviewReasonFilter, ReviewStatusFilter,
};

impl ScannerApplicationService {
    pub fn review_items(
        &self,
        workspace_id: WorkspaceId,
        status: ReviewStatusFilter,
        reason: ReviewReasonFilter,
        limit: usize,
        offset: usize,
    ) -> Result<ReviewPageRecord, ApplicationError> {
        self.database.workspace(workspace_id)?;
        self.database
            .review_items(workspace_id, status, reason, limit, offset)
            .map_err(ApplicationError::Persistence)
    }

    pub fn update_review_item(
        &self,
        review_id: &str,
        action: ReviewAction,
    ) -> Result<ReviewItemRecord, ApplicationError> {
        validate_review_id(review_id)?;
        self.database
            .update_review_item(review_id, action)
            .map_err(ApplicationError::Persistence)
    }
}

pub(crate) fn validate_review_id(review_id: &str) -> Result<(), ApplicationError> {
    if review_id.len() > 64
        || review_id.is_empty()
        || !review_id
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
    {
        return Err(ApplicationError::NotFound);
    }
    Ok(())
}
