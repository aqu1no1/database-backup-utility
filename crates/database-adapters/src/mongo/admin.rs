use database_backup_core::{MongoConnectionConfig, Password};
use mongodb::Client;
use mongodb::bson::doc;
use mongodb::error::{Error as DriverError, ErrorKind};
use thiserror::Error;

use super::archive::DatabaseContents;
use super::connection::{close, connect};
use crate::connection::{ConnectionError, Timeouts};

const UNAUTHORIZED: i32 = 13;
const SYSTEM_PREFIX: &str = "system.";
const VIEW_TYPE: &str = "view";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseState {
    Missing,
    Exists { collections: usize },
}

#[derive(Debug, Error)]
pub enum AdminError {
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error("sem permissão para {action}: {detail}")]
    PermissionDenied {
        action: String,
        detail: String,
        #[source]
        source: Box<DriverError>,
    },
    #[error("falha ao {action}: {detail}")]
    Query {
        action: String,
        detail: String,
        #[source]
        source: Box<DriverError>,
    },
}

pub struct Admin {
    server: MongoConnectionConfig,
    password: Password,
    timeouts: Timeouts,
}

impl Admin {
    #[must_use]
    pub fn new(server: &MongoConnectionConfig, password: &Password, timeouts: Timeouts) -> Self {
        Self {
            server: server.clone(),
            password: Password::new(password.expose().to_owned()),
            timeouts,
        }
    }

    pub async fn database_state(&self, name: &str) -> Result<DatabaseState, AdminError> {
        let client = self.client()?;
        let action = format!("verificar o banco {name}");
        let names = client
            .database(name)
            .list_collection_names()
            .authorized_collections(true)
            .await
            .map_err(|error| query_error(&action, error));
        close(client).await;
        let names = names?;
        if names.is_empty() {
            return Ok(DatabaseState::Missing);
        }
        Ok(DatabaseState::Exists {
            collections: names
                .iter()
                .filter(|collection| !collection.starts_with(SYSTEM_PREFIX))
                .count(),
        })
    }

    pub async fn contents(&self, name: &str) -> Result<DatabaseContents, AdminError> {
        let client = self.client()?;
        let contents = read_contents(&client, name).await;
        close(client).await;
        contents
    }

    pub async fn drop_database(&self, name: &str) -> Result<(), AdminError> {
        let client = self.client()?;
        let result = client
            .database(name)
            .drop()
            .await
            .map_err(|error| query_error(&format!("apagar o banco {name}"), error));
        close(client).await;
        result
    }

    fn client(&self) -> Result<Client, AdminError> {
        Ok(connect(&self.server, &self.password, self.timeouts)?)
    }
}

async fn read_contents(client: &Client, name: &str) -> Result<DatabaseContents, AdminError> {
    let database = client.database(name);
    let action = format!("listar o conteúdo do banco {name}");
    let mut cursor = database
        .list_collections()
        .await
        .map_err(|error| query_error(&action, error))?;
    let mut listed = Vec::new();
    while cursor
        .advance()
        .await
        .map_err(|error| query_error(&action, error))?
    {
        let entry = cursor.current();
        let Ok(collection) = entry.get_str("name") else {
            continue;
        };
        if collection.starts_with(SYSTEM_PREFIX) {
            continue;
        }
        let view = entry.get_str("type").is_ok_and(|kind| kind == VIEW_TYPE);
        listed.push((collection.to_owned(), view));
    }

    let mut contents = DatabaseContents::default();
    for (collection, view) in listed {
        if view {
            contents.views.insert(collection);
            continue;
        }
        let count = database
            .collection::<mongodb::bson::Document>(&collection)
            .count_documents(doc! {})
            .await
            .map_err(|error| {
                query_error(&format!("contar os documentos de {collection}"), error)
            })?;
        contents.documents.insert(collection, count);
    }
    Ok(contents)
}

fn query_error(action: &str, error: DriverError) -> AdminError {
    let detail = match &*error.kind {
        ErrorKind::Command(command) => command.message.clone(),
        other => other.to_string(),
    };
    if let ErrorKind::Command(command) = &*error.kind
        && command.code == UNAUTHORIZED
    {
        return AdminError::PermissionDenied {
            action: action.to_owned(),
            detail,
            source: Box::new(error),
        };
    }
    AdminError::Query {
        action: action.to_owned(),
        detail,
        source: Box::new(error),
    }
}
