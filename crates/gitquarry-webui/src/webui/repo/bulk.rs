//! The form behind every bulk close/reopen list (issues, milestones, merge requests).

use crate::webui::error::AppError;

/// Upper bound on the rows one bulk post may act on; each row is a request to the tracker or a storage write.
pub(crate) const MAX_BULK_ITEMS: usize = 100;

/// Read by hand from the raw body rather than through axum's `Form`:
/// `serde_urlencoded` can't collect a key repeated once per checked row into
/// a `Vec`, so every submission with a selection was a 422.
#[derive(Default)]
pub(crate) struct BulkForm {
    pub(crate) repo: String,
    pub(crate) repo_uid: String,
    /// Checkbox values (`<input type="checkbox" name="numbers" value=...>`),
    /// one per selected row. A value that isn't a number names no item, so it
    /// is dropped rather than failing the whole batch.
    pub(crate) numbers: Vec<u64>,
    /// `"close"` or `"reopen"` — anything else is rejected up front, before
    /// touching any item.
    pub(crate) action: String,
    pub(crate) csrf: String,
}

impl BulkForm {
    pub(crate) fn parse(body: &[u8]) -> Self {
        let mut form = Self::default();
        for (key, value) in form_urlencoded::parse(body) {
            match key.as_ref() {
                "repo" => form.repo = value.into_owned(),
                "repo_uid" => form.repo_uid = value.into_owned(),
                "numbers" => form.numbers.extend(value.parse::<u64>().ok()),
                "action" => form.action = value.into_owned(),
                "csrf" => form.csrf = value.into_owned(),
                _ => {}
            }
        }
        form
    }

    /// Checked after the CSRF and permission checks so an unauthorised caller learns nothing from it.
    pub(crate) fn require_within_cap(&self) -> Result<(), AppError> {
        if self.numbers.len() > MAX_BULK_ITEMS {
            return Err(AppError::bad_request(anyhow::anyhow!(
                "at most {MAX_BULK_ITEMS} items can be selected at once"
            )));
        }
        Ok(())
    }
}
