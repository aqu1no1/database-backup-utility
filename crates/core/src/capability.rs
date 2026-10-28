use crate::engine::DatabaseEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    ConnectionTest,
    FullBackup,
    Verify,
    Restore,
    RestoreOverwrite,
    SelectiveRestore,
    IncrementalBackup,
    DifferentialBackup,
    PointInTimeRecovery,
    Compression,
    Encryption,
    DockerRunner,
}

impl Operation {
    pub const ALL: [Self; 12] = [
        Self::ConnectionTest,
        Self::FullBackup,
        Self::Verify,
        Self::Restore,
        Self::RestoreOverwrite,
        Self::SelectiveRestore,
        Self::IncrementalBackup,
        Self::DifferentialBackup,
        Self::PointInTimeRecovery,
        Self::Compression,
        Self::Encryption,
        Self::DockerRunner,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionTest => "connection_test",
            Self::FullBackup => "full_backup",
            Self::Verify => "verify",
            Self::Restore => "restore",
            Self::RestoreOverwrite => "restore_overwrite",
            Self::SelectiveRestore => "selective_restore",
            Self::IncrementalBackup => "incremental_backup",
            Self::DifferentialBackup => "differential_backup",
            Self::PointInTimeRecovery => "point_in_time_recovery",
            Self::Compression => "compression",
            Self::Encryption => "encryption",
            Self::DockerRunner => "docker_runner",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::ConnectionTest => "teste de conexão",
            Self::FullBackup => "backup completo",
            Self::Verify => "verificação do arquivo",
            Self::Restore => "restore em destino novo",
            Self::RestoreOverwrite => "restore sobrescrevendo destino existente",
            Self::SelectiveRestore => "restore seletivo",
            Self::IncrementalBackup => "backup incremental",
            Self::DifferentialBackup => "backup diferencial",
            Self::PointInTimeRecovery => "recuperação point-in-time",
            Self::Compression => "compressão",
            Self::Encryption => "criptografia",
            Self::DockerRunner => "ferramentas via Docker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Supported,
    Planned { issue: &'static str },
    Unsupported,
}

impl Support {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Planned { .. } => "planned",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub operation: Operation,
    pub support: Support,
    pub note: &'static str,
}

const fn entry(operation: Operation, support: Support, note: &'static str) -> Capability {
    Capability {
        operation,
        support,
        note,
    }
}

const COMPRESSION: Capability = entry(
    Operation::Compression,
    Support::Planned { issue: "DB-31" },
    "compressão opcional planejada para a v0.4.0",
);
const ENCRYPTION: Capability = entry(
    Operation::Encryption,
    Support::Planned { issue: "DB-31" },
    "criptografia age planejada para a v0.4.0",
);

const POSTGRES: [Capability; 12] = [
    entry(
        Operation::ConnectionTest,
        Support::Supported,
        "tokio-postgres, com TLS verify-full opcional",
    ),
    entry(
        Operation::FullBackup,
        Support::Supported,
        "pg_dump em formato custom, snapshot consistente de um banco",
    ),
    entry(
        Operation::Verify,
        Support::Supported,
        "checksum e pg_restore --list, com pg_restore compatível com a origem",
    ),
    entry(
        Operation::Restore,
        Support::Supported,
        "pg_restore em banco novo",
    ),
    entry(
        Operation::RestoreOverwrite,
        Support::Supported,
        "--overwrite com confirmação; o anterior fica como <destino>__previous_<data>",
    ),
    entry(
        Operation::SelectiveRestore,
        Support::Unsupported,
        "pg_restore permite filtrar objetos, mas a CLI ainda não expõe nem testa isso",
    ),
    entry(
        Operation::IncrementalBackup,
        Support::Unsupported,
        "exige arquivamento de WAL ou pg_basebackup, fora do escopo de um dump lógico",
    ),
    entry(
        Operation::DifferentialBackup,
        Support::Unsupported,
        "exige arquivamento de WAL ou pg_basebackup, fora do escopo de um dump lógico",
    ),
    entry(
        Operation::PointInTimeRecovery,
        Support::Unsupported,
        "exige backup físico com arquivamento contínuo de WAL",
    ),
    COMPRESSION,
    ENCRYPTION,
    entry(
        Operation::DockerRunner,
        Support::Supported,
        "imagem postgres:<major> quando não há cliente nativo compatível",
    ),
];

const MYSQL: [Capability; 12] = [
    entry(
        Operation::ConnectionTest,
        Support::Supported,
        "mysql_async, com TLS verify-full opcional",
    ),
    entry(
        Operation::FullBackup,
        Support::Supported,
        "mysqldump --single-transaction; consistente só para tabelas InnoDB",
    ),
    entry(
        Operation::Verify,
        Support::Supported,
        "checksum e leitura do SQL em Rust, sem ferramenta externa",
    ),
    entry(
        Operation::Restore,
        Support::Supported,
        "cliente mysql em banco novo, com --binary-mode e --local-infile=0",
    ),
    entry(
        Operation::RestoreOverwrite,
        Support::Unsupported,
        "o MySQL não tem RENAME DATABASE para uma troca atômica",
    ),
    entry(
        Operation::SelectiveRestore,
        Support::Unsupported,
        "o dump SQL é aplicado inteiro",
    ),
    entry(
        Operation::IncrementalBackup,
        Support::Unsupported,
        "exige binlog ou backup físico, fora do escopo do mysqldump",
    ),
    entry(
        Operation::DifferentialBackup,
        Support::Unsupported,
        "exige backup físico, fora do escopo do mysqldump",
    ),
    entry(
        Operation::PointInTimeRecovery,
        Support::Unsupported,
        "exige replay de binlog a partir de um backup base",
    ),
    COMPRESSION,
    ENCRYPTION,
    entry(
        Operation::DockerRunner,
        Support::Supported,
        "imagem mysql:<major.minor> quando não há cliente nativo compatível",
    ),
];

const MONGODB: [Capability; 12] = [
    entry(
        Operation::ConnectionTest,
        Support::Supported,
        "driver mongodb; recusa sharded e secundário de replica set",
    ),
    entry(
        Operation::FullBackup,
        Support::Supported,
        "mongodump --archive de um banco, sem snapshot de um único instante",
    ),
    entry(
        Operation::Verify,
        Support::Supported,
        "checksum e leitura do archive em Rust, sem ferramenta externa",
    ),
    entry(
        Operation::Restore,
        Support::Supported,
        "mongorestore em banco novo, conferindo documentos por coleção",
    ),
    entry(
        Operation::RestoreOverwrite,
        Support::Unsupported,
        "o MongoDB não tem RENAME DATABASE para uma troca atômica",
    ),
    entry(
        Operation::SelectiveRestore,
        Support::Unsupported,
        "o archive é restaurado inteiro",
    ),
    entry(
        Operation::IncrementalBackup,
        Support::Unsupported,
        "exige leitura contínua do oplog, fora do escopo do mongodump",
    ),
    entry(
        Operation::DifferentialBackup,
        Support::Unsupported,
        "exige leitura contínua do oplog, fora do escopo do mongodump",
    ),
    entry(
        Operation::PointInTimeRecovery,
        Support::Unsupported,
        "--oplog só existe para o servidor inteiro, e o backup aqui é de um banco",
    ),
    COMPRESSION,
    ENCRYPTION,
    entry(
        Operation::DockerRunner,
        Support::Supported,
        "imagem mongo:<major.minor> quando não há Database Tools nativas",
    ),
];

const SQLITE: [Capability; 12] = [
    entry(
        Operation::ConnectionTest,
        Support::Unsupported,
        "o SQLite é um arquivo local, sem servidor",
    ),
    entry(
        Operation::FullBackup,
        Support::Supported,
        "Online Backup API, consistente mesmo com escritas em WAL",
    ),
    entry(
        Operation::Verify,
        Support::Supported,
        "checksum e PRAGMA integrity_check",
    ),
    entry(
        Operation::Restore,
        Support::Supported,
        "Online Backup API em arquivo novo",
    ),
    entry(
        Operation::RestoreOverwrite,
        Support::Supported,
        "--overwrite com confirmação, se nenhum processo tiver o arquivo aberto",
    ),
    entry(
        Operation::SelectiveRestore,
        Support::Unsupported,
        "o arquivo é restaurado inteiro",
    ),
    entry(
        Operation::IncrementalBackup,
        Support::Unsupported,
        "a Backup API copia o banco inteiro",
    ),
    entry(
        Operation::DifferentialBackup,
        Support::Unsupported,
        "a Backup API copia o banco inteiro",
    ),
    entry(
        Operation::PointInTimeRecovery,
        Support::Unsupported,
        "o SQLite não mantém histórico de transações para replay",
    ),
    COMPRESSION,
    ENCRYPTION,
    entry(
        Operation::DockerRunner,
        Support::Unsupported,
        "não há ferramenta externa: a cópia usa o rusqlite embutido",
    ),
];

impl DatabaseEngine {
    #[must_use]
    pub const fn capabilities(self) -> &'static [Capability] {
        match self {
            Self::Postgres => &POSTGRES,
            Self::Mysql => &MYSQL,
            Self::Mongodb => &MONGODB,
            Self::Sqlite => &SQLITE,
        }
    }

    #[must_use]
    pub fn capability(self, operation: Operation) -> Capability {
        self.capabilities()
            .iter()
            .find(|capability| capability.operation == operation)
            .copied()
            .unwrap_or(Capability {
                operation,
                support: Support::Unsupported,
                note: "",
            })
    }

    #[must_use]
    pub fn unsupported_message(self, operation: Operation) -> Option<String> {
        let capability = self.capability(operation);
        let reason = match capability.support {
            Support::Supported => return None,
            Support::Planned { issue } => format!("planejado em {issue}"),
            Support::Unsupported => "não suportado".to_owned(),
        };
        let mut message = format!(
            "{} não está disponível para {} ({reason})",
            operation.description(),
            self.display_name()
        );
        if !capability.note.is_empty() {
            message.push_str(": ");
            message.push_str(capability.note);
        }
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_declares_every_operation_exactly_once() {
        for engine in DatabaseEngine::ALL {
            for operation in Operation::ALL {
                let declared = engine
                    .capabilities()
                    .iter()
                    .filter(|capability| capability.operation == operation)
                    .count();
                assert_eq!(
                    declared, 1,
                    "{engine} declara {operation:?} {declared} vezes"
                );
            }
        }
    }

    #[test]
    fn every_entry_explains_its_scope() {
        for engine in DatabaseEngine::ALL {
            for capability in engine.capabilities() {
                assert!(
                    !capability.note.is_empty(),
                    "{engine} {:?} sem explicação",
                    capability.operation
                );
            }
        }
    }

    #[test]
    fn nothing_beyond_full_backup_and_restore_is_claimed_without_an_implementation() {
        for engine in DatabaseEngine::ALL {
            for operation in [
                Operation::SelectiveRestore,
                Operation::IncrementalBackup,
                Operation::DifferentialBackup,
                Operation::PointInTimeRecovery,
                Operation::Compression,
                Operation::Encryption,
            ] {
                assert_ne!(
                    engine.capability(operation).support,
                    Support::Supported,
                    "{engine} declara {operation:?} sem implementação"
                );
            }
        }
    }

    #[test]
    fn unsupported_message_names_the_operation_engine_and_reason() {
        assert_eq!(
            DatabaseEngine::Postgres.unsupported_message(Operation::FullBackup),
            None
        );
        let message = DatabaseEngine::Sqlite
            .unsupported_message(Operation::ConnectionTest)
            .unwrap();
        assert_eq!(
            message,
            "teste de conexão não está disponível para SQLite (não suportado): o SQLite é um arquivo local, sem servidor"
        );
        let planned = DatabaseEngine::Mysql
            .unsupported_message(Operation::Encryption)
            .unwrap();
        assert!(planned.contains("planejado em DB-31"), "{planned}");
    }
}
