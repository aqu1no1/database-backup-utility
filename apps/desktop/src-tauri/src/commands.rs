use std::path::Path;

use database_backup_history::History;
use database_backup_operations::desktop::{
    self, BridgeError, HISTORY_LIMIT, HistoryItem, ServerConnection, Settings,
};

async fn with_history<T, F>(task: F) -> Result<T, BridgeError>
where
    T: Send + 'static,
    F: FnOnce(&History, &Path) -> Result<T, BridgeError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let path = History::default_path()?;
        let history = History::open(&path)?;
        task(&history, &path)
    })
    .await
    .map_err(|_| BridgeError::internal())?
}

#[tauri::command]
pub async fn history_list() -> Result<Vec<HistoryItem>, BridgeError> {
    with_history(|history, _| desktop::history_items(history, HISTORY_LIMIT)).await
}

#[tauri::command]
pub async fn connections_list() -> Result<Vec<ServerConnection>, BridgeError> {
    with_history(|history, _| desktop::connections(history)).await
}

#[tauri::command]
pub async fn settings_get() -> Result<Settings, BridgeError> {
    with_history(desktop::settings).await
}

#[tauri::command]
pub async fn settings_set(settings: Settings) -> Result<Settings, BridgeError> {
    with_history(move |history, _| desktop::save_settings(history, settings)).await
}
