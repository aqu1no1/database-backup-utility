# Capacidades por SGBD

O que cada banco suporta hoje, o que está planejado e o que não é suportado, e por quê. A fonte é
o tipo `DatabaseEngine::capabilities` em `crates/core/src/capability.rs`. A CLI mostra a mesma
matriz com `database-backup engines` (ou `--json`) e usa os mesmos textos nos erros de operação não
disponível (código 2). Se esta tabela e o código divergirem, vale o código, e esta página precisa
ser corrigida no mesmo PR.

## Matriz

Legenda: **sim** (implementado e testado), **planejado** (com card), **não** (não suportado).

| Operação | PostgreSQL | MySQL | MongoDB | SQLite |
| --- | --- | --- | --- | --- |
| Teste de conexão (`connection test`) | sim | sim | sim | não: arquivo local, sem servidor |
| Backup completo (`backup run`) | sim | sim | sim | sim |
| Verificação (`backup verify`) | sim | sim | sim | sim |
| Restore em destino novo (`restore run`) | sim | sim | sim | sim |
| Restore sobrescrevendo destino (`--overwrite`) | sim | não: sem `RENAME DATABASE` | não: sem `RENAME DATABASE` | sim |
| Restore seletivo | não | não | não | não |
| Backup incremental | não | não | não | não |
| Backup diferencial | não | não | não | não |
| Recuperação point-in-time | não | não | não | não |
| Compressão | planejado (DB-31) | planejado (DB-31) | planejado (DB-31) | planejado (DB-31) |
| Criptografia | planejado (DB-31) | planejado (DB-31) | planejado (DB-31) | planejado (DB-31) |
| Ferramentas via Docker (`--runner docker`) | sim | sim | sim | não: não usa ferramenta externa |

Incremental, diferencial e point-in-time não dependem só da CLI. Cada um precisa de um mecanismo
do próprio SGBD que um dump lógico de um banco não tem:

- **PostgreSQL:** arquivamento contínuo de WAL e backup físico (`pg_basebackup`).
- **MySQL:** binlog e um backup base físico.
- **MongoDB:** oplog. O `mongodump --oplog` só existe para o servidor inteiro, e o backup aqui é de
  um banco.
- **SQLite:** a Backup API copia o banco inteiro, e não existe histórico de transações para replay.

Esses itens ficam como **não suportados**, e não como planejados, até existir um card com escopo
definido.

Restore seletivo também está marcado como não suportado. O `pg_restore` filtra objetos, mas a CLI
não expõe nem testa isso.

## Formatos e ferramentas

| SGBD | Artefato | Backup | Restore | Verify sem servidor | Versões |
| --- | --- | --- | --- | --- | --- |
| PostgreSQL | `<banco>-<data>.dump` (`pg_dump` formato custom) | `pg_dump` nativo ou `postgres:<major>` | `pg_restore` | `pg_restore --list` (precisa de `pg_restore` compatível) | cliente ≥ servidor; a origem fica no manifesto |
| MySQL | `<banco>-<data>.sql` | `mysqldump --single-transaction` nativo ou `mysql:<major.minor>` | cliente `mysql` com `--binary-mode` e `--local-infile=0` | leitura do SQL em Rust (`mysql/dump.rs`) | cliente ≥ servidor (major.minor); MariaDB ainda não (DB-37) |
| MongoDB | `<banco>-<data>.archive` (`--archive`, sem compressão) | `mongodump --db` nativo ou `mongo:<major.minor>` | `mongorestore` com `--nsFrom/--nsTo`, sem `--drop` | leitura do archive em Rust (`mongo/archive.rs`) | Database Tools 100.x; servidor ≥ 4.4 ([tabela em `stack.md`](stack.md#mongodb-database-tools)) |
| SQLite | `<arquivo>-<data>.sqlite3` | Online Backup API (`rusqlite`) | Online Backup API em `.partial` + rename | `PRAGMA integrity_check` | SQLite embutido no binário |

Todo artefato tem um manifesto `.meta.json` com SHA-256, tamanho e versões, descrito em
[`manifesto.md`](manifesto.md).

## Limites de consistência

- **PostgreSQL:** o `pg_dump` lê um snapshot único do banco.
- **MySQL:** com `--single-transaction`, só as tabelas InnoDB entram em um snapshot consistente.
  Tabelas de outros engines são listadas no aviso do backup, no `--json` e no manifesto
  (`non_transactional_tables`).
- **MongoDB:** não há snapshot de um único instante. Escritas durante o backup podem deixar coleções
  inconsistentes entre si. A CLI só aceita standalone e o primário de um replica set; secundário e
  cluster sharded são recusados ([`arquitetura.md`](arquitetura.md#mongodb-topologia-e-consistência)).
- **SQLite:** a cópia é consistente mesmo com a aplicação gravando, inclusive em WAL. Em modo
  rollback, os escritores esperam até o fim da cópia.

Os riscos aceitos de cada SGBD estão em [`seguranca.md`](seguranca.md).

## Testes de contrato

Todo SGBD com backup e restore passa pelo mesmo contrato, `assert_round_trip` em
`crates/database-adapters/tests/support/mod.rs`:

1. a matriz declara backup completo, verify e restore como suportados;
2. o artefato lido sem servidor tem o mesmo número de tabelas (coleções, no MongoDB) da origem;
3. o destino restaurado tem o mesmo número de tabelas da origem;
4. os dados restaurados são iguais aos da origem;
5. a origem não muda.

Cada SGBD continua com o próprio teste de round-trip e decide o que é "dados iguais":

| SGBD | Teste | Comparação |
| --- | --- | --- |
| PostgreSQL | `postgres_restore.rs` | contagem e soma das tabelas da fixture |
| MySQL | `mysql_tools.rs` | linhas de tabelas e views, por runner |
| MongoDB | `mongo_tools.rs` | documentos, índices, opções e views, por runner |
| SQLite | `sqlite_backup.rs` | linhas da tabela de eventos |

Cada teste também mantém as checagens que só fazem sentido para o seu SGBD, como trigger
restaurado, segundo restore recusado ou cabeçalho do dump. O contrato não esconde essas diferenças:
ele só garante que nenhum SGBD passe sem cumprir o mínimo.

## Como adicionar um novo SGBD

Use o MariaDB (DB-37) como referência do próximo caso.

1. **Core:**
   - variante nova em `DatabaseEngine` (`crates/core/src/engine.rs`), com `as_str`,
     `display_name`, `FromStr` e `ALL`;
   - entradas em `crates/core/src/capability.rs`, uma por `Operation`. Os testes unitários recusam
     operação faltando ou repetida, e incremental, diferencial, PITR, seletivo, compressão ou
     criptografia marcados como suportados.
2. **Configuração:** tipo de conexão em `crates/core/src/connection.rs` com os limites de nome do
   SGBD e a mesma regra de TLS.
3. **Adaptador:** módulo em `crates/database-adapters/src/<sgbd>/` com o mesmo desenho dos atuais
   (`connection.rs`, `admin.rs`, `tools.rs` e a leitura do artefato sem servidor). Ferramentas
   externas sempre pelo runner comum (`process.rs`), com senha em arquivo `0600` e nunca em
   argumento.
4. **Histórico:** se o nome do SGBD for novo, uma migration ampliando o `CHECK (engine IN (...))` em
   `crates/history`.
5. **CLI:**
   - valor novo em `EngineArg`;
   - fluxo em `apps/cli/src/<sgbd>.rs`;
   - detecção do artefato em `archive.rs`;
   - operações fora da matriz recusadas com o helper `unsupported` de `main.rs`, para a mensagem
     sair da matriz.
6. **Testes:**
   - harness e fixture em `tests/support/mod.rs` e `tests/fixtures/<sgbd>/`;
   - round-trip chamando `assert_round_trip`;
   - testes de CLI em `apps/cli/tests/<sgbd>.rs`.

   Siga [`testes-rust.md`](testes-rust.md).
7. **Docs:**
   - esta página (matriz, formatos, consistência e a tabela de testes);
   - [`stack.md`](stack.md), [`arquitetura.md`](arquitetura.md), [`seguranca.md`](seguranca.md) e
     [`comandos.md`](comandos.md);
   - uma entrada no `CHANGELOG.md`.
