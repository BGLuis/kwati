//! Remote sync and network actions (owned by guinet-worker for #50, #52).

use gpui::actions;

actions!(
    remote_sync,
    [TriggerRefresh, ToggleRemoteSearch, RetryConnection,]
);
