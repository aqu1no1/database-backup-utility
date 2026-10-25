use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use mongodb::bson::RawDocument;
use thiserror::Error;

use super::connection::MongoVersion;

pub const ARCHIVE_MAGIC: [u8; 4] = [0x6d, 0xe2, 0x99, 0x81];
const TERMINATOR: u32 = 0xFFFF_FFFF;
const MIN_DOCUMENT_BYTES: u32 = 5;
const MAX_DOCUMENT_BYTES: u32 = 16 * 1024 * 1024 + 16 * 1024;
const MAX_NAMESPACES: usize = 100_000;
const READ_BUFFER_BYTES: usize = 64 * 1024;
const SYSTEM_PREFIX: &str = "system.";
const VIEW_TYPE: &str = "view";
const TIMESERIES_TYPE: &str = "timeseries";
const BUCKETS_PREFIX: &str = "system.buckets.";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DatabaseContents {
    pub documents: BTreeMap<String, u64>,
    pub views: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveInfo {
    pub database: String,
    pub server_version: Option<String>,
    pub dumped_from: Option<MongoVersion>,
    pub tool_version: Option<String>,
    pub contents: DatabaseContents,
}

impl ArchiveInfo {
    #[must_use]
    pub fn collections(&self) -> usize {
        self.contents.documents.len()
    }

    #[must_use]
    pub fn objects(&self) -> usize {
        self.contents.documents.len() + self.contents.views.len()
    }
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("não foi possível ler {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("o arquivo não é um archive do mongodump (número mágico ausente)")]
    NotMongoArchive,
    #[error("o archive está incompleto: {detail}")]
    Incomplete { detail: String },
    #[error("o archive está corrompido: {detail}")]
    Corrupt { detail: String },
    #[error(
        "o archive tem mais de um banco ({first} e {other}); só são aceitos archives de um único banco, sem o backup do servidor inteiro"
    )]
    MultipleDatabases { first: String, other: String },
    #[error("o archive contém coleções que esta versão não suporta: {detail}")]
    Unsupported { detail: String },
    #[error("o archive não contém coleções nem views")]
    Empty,
}

pub fn inspect_archive(path: &Path) -> Result<ArchiveInfo, ArchiveError> {
    let file = std::fs::File::open(path).map_err(|source| ArchiveError::Io {
        path: path.to_owned(),
        source,
    })?;
    scan(BufReader::with_capacity(READ_BUFFER_BYTES, file)).map_err(|error| match error {
        ScanError::Io(source) => ArchiveError::Io {
            path: path.to_owned(),
            source,
        },
        ScanError::Archive(error) => error,
    })
}

enum ScanError {
    Io(io::Error),
    Archive(ArchiveError),
}

impl From<ArchiveError> for ScanError {
    fn from(error: ArchiveError) -> Self {
        Self::Archive(error)
    }
}

struct Namespace {
    view: bool,
    documents: u64,
    finished: bool,
}

fn incomplete(detail: &str) -> ScanError {
    ArchiveError::Incomplete {
        detail: detail.to_owned(),
    }
    .into()
}

fn corrupt(detail: String) -> ScanError {
    ArchiveError::Corrupt { detail }.into()
}

fn unsupported_timeseries(collection: &str) -> ScanError {
    ArchiveError::Unsupported {
        detail: format!("coleções time series ({collection})"),
    }
    .into()
}

fn read_exact(reader: &mut impl Read, buffer: &mut [u8], what: &str) -> Result<(), ScanError> {
    reader.read_exact(buffer).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            incomplete(what)
        } else {
            ScanError::Io(error)
        }
    })
}

fn read_length(reader: &mut impl Read, what: &str) -> Result<Option<u32>, ScanError> {
    let mut bytes = [0_u8; 4];
    let mut filled = 0;
    while filled < bytes.len() {
        match reader.read(&mut bytes[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ScanError::Io(error)),
        }
    }
    match filled {
        0 => Ok(None),
        4 => Ok(Some(u32::from_le_bytes(bytes))),
        _ => Err(incomplete(what)),
    }
}

fn check_length(length: u32, what: &str) -> Result<(), ScanError> {
    if (MIN_DOCUMENT_BYTES..=MAX_DOCUMENT_BYTES).contains(&length) {
        Ok(())
    } else {
        Err(corrupt(format!(
            "{what} declara {length} bytes, fora do intervalo aceito"
        )))
    }
}

fn read_document(reader: &mut impl Read, length: u32, what: &str) -> Result<Vec<u8>, ScanError> {
    check_length(length, what)?;
    let mut buffer = vec![0_u8; length as usize];
    buffer[..4].copy_from_slice(&length.to_le_bytes());
    read_exact(reader, &mut buffer[4..], what)?;
    Ok(buffer)
}

fn skip(reader: &mut impl Read, bytes: u64, what: &str) -> Result<(), ScanError> {
    let copied =
        io::copy(&mut reader.by_ref().take(bytes), &mut io::sink()).map_err(ScanError::Io)?;
    if copied == bytes {
        Ok(())
    } else {
        Err(incomplete(what))
    }
}

fn text<'a>(document: &'a RawDocument, key: &str, what: &str) -> Result<&'a str, ScanError> {
    document
        .get_str(key)
        .map_err(|_| corrupt(format!("{what} sem o campo {key}")))
}

fn parse<'a>(buffer: &'a [u8], what: &str) -> Result<&'a RawDocument, ScanError> {
    RawDocument::from_bytes(buffer)
        .map_err(|_| corrupt(format!("{what} não é um documento BSON válido")))
}

fn scan(mut reader: impl Read) -> Result<ArchiveInfo, ScanError> {
    let mut magic = [0_u8; 4];
    match reader.read_exact(&mut magic) {
        Ok(()) if magic == ARCHIVE_MAGIC => {}
        Ok(()) => return Err(ArchiveError::NotMongoArchive.into()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(ArchiveError::NotMongoArchive.into());
        }
        Err(error) => return Err(ScanError::Io(error)),
    }

    let Some(length) = read_length(&mut reader, "o cabeçalho do archive foi cortado")? else {
        return Err(incomplete("falta o cabeçalho do archive"));
    };
    let buffer = read_document(&mut reader, length, "o cabeçalho")?;
    let header = parse(&buffer, "o cabeçalho")?;
    let server_version = header.get_str("server_version").ok().map(str::to_owned);
    let tool_version = header.get_str("tool_version").ok().map(str::to_owned);

    let mut database: Option<String> = None;
    let mut namespaces: BTreeMap<String, Namespace> = BTreeMap::new();
    loop {
        let Some(length) = read_length(&mut reader, "o prelúdio do archive foi cortado")? else {
            return Err(incomplete("falta o fim do prelúdio do archive"));
        };
        if length == TERMINATOR {
            break;
        }
        let buffer = read_document(&mut reader, length, "uma entrada do prelúdio")?;
        let entry = parse(&buffer, "uma entrada do prelúdio")?;
        let entry_database = text(entry, "db", "uma entrada do prelúdio")?;
        let collection = text(entry, "collection", "uma entrada do prelúdio")?;
        if collection.starts_with(BUCKETS_PREFIX)
            || entry
                .get_str("type")
                .is_ok_and(|kind| kind == TIMESERIES_TYPE)
        {
            return Err(unsupported_timeseries(collection));
        }
        match &database {
            None => database = Some(entry_database.to_owned()),
            Some(first) if first != entry_database => {
                return Err(ArchiveError::MultipleDatabases {
                    first: first.clone(),
                    other: entry_database.to_owned(),
                }
                .into());
            }
            Some(_) => {}
        }
        if namespaces.len() >= MAX_NAMESPACES {
            return Err(corrupt(format!(
                "o prelúdio lista mais de {MAX_NAMESPACES} coleções"
            )));
        }
        let view = entry.get_str("type").is_ok_and(|kind| kind == VIEW_TYPE);
        let previous = namespaces.insert(
            collection.to_owned(),
            Namespace {
                view,
                documents: 0,
                finished: false,
            },
        );
        if previous.is_some() {
            return Err(corrupt(format!(
                "a coleção {collection} aparece duas vezes no prelúdio"
            )));
        }
    }

    while let Some(length) = read_length(&mut reader, "um bloco do archive foi cortado")? {
        let buffer = read_document(&mut reader, length, "o cabeçalho de um bloco")?;
        let block = parse(&buffer, "o cabeçalho de um bloco")?;
        let block_database = text(block, "db", "o cabeçalho de um bloco")?;
        let collection = text(block, "collection", "o cabeçalho de um bloco")?;
        let finished = block.get_bool("EOF").unwrap_or(false);
        let known = database.as_deref() == Some(block_database);
        let Some(namespace) = namespaces.get_mut(collection).filter(|_| known) else {
            if collection.starts_with(BUCKETS_PREFIX) {
                return Err(unsupported_timeseries(collection));
            }
            return Err(corrupt(format!(
                "o bloco de {block_database}.{collection} não consta no prelúdio"
            )));
        };
        loop {
            let Some(length) = read_length(&mut reader, "um bloco do archive foi cortado")? else {
                return Err(incomplete("falta o terminador de um bloco"));
            };
            if length == TERMINATOR {
                break;
            }
            check_length(length, "um documento")?;
            skip(
                &mut reader,
                u64::from(length) - 4,
                "um documento foi cortado",
            )?;
            namespace.documents += 1;
        }
        namespace.finished |= finished;
    }

    let Some(database) = database else {
        return Err(ArchiveError::Empty.into());
    };
    if let Some((name, _)) = namespaces.iter().find(|(_, namespace)| !namespace.finished) {
        return Err(ArchiveError::Incomplete {
            detail: format!("a coleção {name} não recebeu o marcador de fim"),
        }
        .into());
    }

    let mut contents = DatabaseContents::default();
    for (name, namespace) in namespaces {
        if name.starts_with(SYSTEM_PREFIX) {
            continue;
        }
        if namespace.view {
            contents.views.insert(name);
        } else {
            contents.documents.insert(name, namespace.documents);
        }
    }
    if contents.documents.is_empty() && contents.views.is_empty() {
        return Err(ArchiveError::Empty.into());
    }
    Ok(ArchiveInfo {
        database,
        dumped_from: server_version.as_deref().and_then(MongoVersion::parse),
        server_version,
        tool_version,
        contents,
    })
}

#[cfg(test)]
mod tests {
    use mongodb::bson::{Document, doc};

    use super::*;

    fn bytes(document: &Document) -> Vec<u8> {
        document.to_vec().expect("documento BSON")
    }

    fn prelude(database: &str, collection: &str, kind: &str) -> Document {
        doc! { "db": database, "collection": collection, "metadata": "{}", "size": 0_i64, "type": kind }
    }

    fn block(database: &str, collection: &str, finished: bool, documents: u32) -> Vec<u8> {
        let mut out = bytes(&doc! {
            "db": database, "collection": collection, "EOF": finished, "CRC": 0_i64,
        });
        for index in 0..documents {
            out.extend(bytes(&doc! { "_id": index as i32, "nome": "Zoë 🚀" }));
        }
        out.extend(TERMINATOR.to_le_bytes());
        out
    }

    fn archive(entries: &[Document], blocks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = ARCHIVE_MAGIC.to_vec();
        out.extend(bytes(&doc! {
            "concurrent_collections": 4,
            "version": "0.1",
            "server_version": "8.0.15",
            "tool_version": "100.10.0",
        }));
        for entry in entries {
            out.extend(bytes(entry));
        }
        out.extend(TERMINATOR.to_le_bytes());
        for block in blocks {
            out.extend(block);
        }
        out
    }

    fn sample() -> Vec<u8> {
        archive(
            &[
                prelude("app", "customers", "collection"),
                prelude("app", "empty_collection", "collection"),
                prelude("app", "totals", "view"),
                prelude("app", "system.views", "collection"),
            ],
            &[
                block("app", "totals", false, 0),
                block("app", "totals", true, 0),
                block("app", "customers", false, 2),
                block("app", "system.views", false, 1),
                block("app", "customers", false, 1),
                block("app", "system.views", true, 0),
                block("app", "customers", true, 0),
                block("app", "empty_collection", false, 0),
                block("app", "empty_collection", true, 0),
            ],
        )
    }

    fn inspect(content: &[u8]) -> Result<ArchiveInfo, ArchiveError> {
        scan(content).map_err(|error| match error {
            ScanError::Io(source) => ArchiveError::Io {
                path: PathBuf::new(),
                source,
            },
            ScanError::Archive(error) => error,
        })
    }

    #[test]
    fn reads_counts_views_and_versions_of_a_complete_archive() {
        let info = inspect(&sample()).expect("archive válido");

        assert_eq!(info.database, "app");
        assert_eq!(info.server_version.as_deref(), Some("8.0.15"));
        assert_eq!(info.dumped_from, Some(MongoVersion { major: 8, minor: 0 }));
        assert_eq!(info.tool_version.as_deref(), Some("100.10.0"));
        assert_eq!(
            info.contents.documents,
            BTreeMap::from([
                ("customers".to_owned(), 3),
                ("empty_collection".to_owned(), 0),
            ])
        );
        assert_eq!(info.contents.views, BTreeSet::from(["totals".to_owned()]));
        assert_eq!((info.collections(), info.objects()), (2, 3));
    }

    #[test]
    fn truncated_archive_is_incomplete() {
        let full = sample();

        for cut in ARCHIVE_MAGIC.len()..full.len() {
            let result = inspect(&full[..cut]);
            assert!(
                matches!(result, Err(ArchiveError::Incomplete { .. })),
                "corte em {cut} de {}: {result:?}",
                full.len()
            );
        }
        assert!(inspect(&full).is_ok());
    }

    #[test]
    fn foreign_files_are_not_archives() {
        for content in [
            &b""[..],
            b"6d",
            b"PGDMP\x01\x02\x03",
            b"SQLite format 3\0",
            b"-- MySQL dump ",
        ] {
            assert!(
                matches!(inspect(content), Err(ArchiveError::NotMongoArchive)),
                "{content:?}"
            );
        }
    }

    #[test]
    fn archives_with_more_than_one_database_are_rejected() {
        let content = archive(
            &[
                prelude("app", "customers", "collection"),
                prelude("outro", "orders", "collection"),
            ],
            &[],
        );

        match inspect(&content) {
            Err(ArchiveError::MultipleDatabases { first, other }) => {
                assert_eq!((first.as_str(), other.as_str()), ("app", "outro"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn oversized_length_is_rejected_without_allocating() {
        let mut in_header = ARCHIVE_MAGIC.to_vec();
        in_header.extend(0x7fff_ffff_u32.to_le_bytes());
        let mut in_prelude = archive(&[], &[]);
        in_prelude.truncate(in_prelude.len() - 4);
        in_prelude.extend(0x7fff_ffff_u32.to_le_bytes());
        let mut in_data = archive(&[prelude("app", "customers", "collection")], &[]);
        in_data.extend(bytes(&doc! {
            "db": "app", "collection": "customers", "EOF": false, "CRC": 0_i64,
        }));
        in_data.extend(0x7fff_ffff_u32.to_le_bytes());
        let mut too_small = ARCHIVE_MAGIC.to_vec();
        too_small.extend(4_u32.to_le_bytes());

        for content in [in_header, in_prelude, in_data, too_small] {
            assert!(
                matches!(inspect(&content), Err(ArchiveError::Corrupt { .. })),
                "{:?}",
                inspect(&content)
            );
        }
    }

    #[test]
    fn namespace_missing_from_the_prelude_is_corrupt() {
        let content = archive(
            &[prelude("app", "customers", "collection")],
            &[
                block("app", "fantasma", false, 1),
                block("app", "customers", true, 0),
            ],
        );
        let other_database = archive(
            &[prelude("app", "customers", "collection")],
            &[block("outro", "customers", true, 0)],
        );

        assert!(matches!(
            inspect(&content),
            Err(ArchiveError::Corrupt { .. })
        ));
        assert!(matches!(
            inspect(&other_database),
            Err(ArchiveError::Corrupt { .. })
        ));
    }

    #[test]
    fn time_series_collections_are_not_supported() {
        let declared = archive(
            &[
                prelude("app", "customers", "collection"),
                prelude("app", "readings", "timeseries"),
            ],
            &[],
        );
        let buckets_only = archive(
            &[prelude("app", "customers", "collection")],
            &[
                block("app", "system.buckets.readings", false, 2),
                block("app", "system.buckets.readings", true, 0),
            ],
        );
        let buckets_in_prelude = archive(
            &[prelude("app", "system.buckets.readings", "collection")],
            &[],
        );

        for content in [declared, buckets_only, buckets_in_prelude] {
            assert!(
                matches!(inspect(&content), Err(ArchiveError::Unsupported { .. })),
                "{:?}",
                inspect(&content)
            );
        }
    }

    #[test]
    fn archive_without_collections_is_empty() {
        let nothing = archive(&[], &[]);
        let only_system = archive(
            &[prelude("app", "system.js", "collection")],
            &[
                block("app", "system.js", false, 1),
                block("app", "system.js", true, 0),
            ],
        );

        assert!(matches!(inspect(&nothing), Err(ArchiveError::Empty)));
        assert!(matches!(inspect(&only_system), Err(ArchiveError::Empty)));
    }
}
