# Estratégia de testes

As regras sobre **quais** testes escrever (relevância, nível, isolamento, tempo) estão em
[`testes-rust.md`](testes-rust.md). Este documento descreve as ferramentas e os cenários existentes.

| Tipo | Ferramentas | Requer Docker |
| --- | --- | --- |
| Unitários Rust | `cargo test --workspace --lib --bins` | Não |
| Integração PostgreSQL | Testcontainers + tokio-postgres | Sim |
| E2E backup/restore | CLI real, dois servidores PostgreSQL temporários | Sim |
| Integração e E2E MySQL | Testcontainers (`mysql:8.4`) + mysql_async (`crates/database-adapters/tests/mysql_*.rs`, `apps/cli/tests/mysql.rs`) | Sim |
| Integração e E2E SQLite | `rusqlite` + arquivos em `tempfile` (`crates/database-adapters/tests/sqlite_backup.rs`, `apps/cli/tests/sqlite.rs`) | Não |
| Angular unit/component (futuro) | Vitest + Angular TestBed | Não |
| Desktop completo (futuro) | Ferramenta Tauri compatível | Conforme cenário |

## Comandos

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins
cargo test --workspace --tests
cargo test -p database-backup-cli --test e2e
cargo build --workspace
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```

## Integração PostgreSQL

O harness `PostgresTestDb` (`crates/database-adapters/tests/support/mod.rs`) sobe um
`postgres:17` efêmero via Testcontainers e entrega um client `tokio-postgres` conectado:

- `PostgresTestDb::start()` cria um banco vazio; `start_seeded()` também aplica
  `tests/fixtures/postgres/seed.sql`.
- A porta é escolhida pelo Docker e a senha é gerada a cada execução; nada é lido do `.env`
  nem do Compose de desenvolvimento.
- O container é removido quando o valor sai de escopo, inclusive se o teste falhar.
- Erros de inicialização e conexão informam imagem, host e porta, nunca a senha.

Para usar em um novo arquivo de teste do crate:

```rust
mod support;

use support::{PostgresTestDb, TestResult};

#[tokio::test]
async fn exemplo() -> TestResult {
    let db = PostgresTestDb::start_seeded().await?;
    let total: i64 = db.client().query_one("SELECT COUNT(*) FROM customers", &[]).await?.get(0);
    assert_eq!(total, 3);
    Ok(())
}
```

O harness não tem testes próprios. Todos os testes de integração e E2E dependem dele, então uma
falha em subir o container ou aplicar a fixture derruba esses testes com a mensagem do harness.

## Integração MySQL

O harness `MysqlTestDb` (mesmo `support/mod.rs`) sobe um `mysql:8.4` efêmero com senha de root
gerada a cada execução e porta escolhida pelo Docker. `start_seeded()` aplica
`tests/fixtures/mysql/seed.sql` comando a comando, como faz o cliente `mysql` (tabelas InnoDB com
FK, view, trigger, UTF-8 e `VARBINARY`).

- `mysql_connection.rs`: senha errada, banco inexistente, `verify-full` contra o certificado
  autoassinado do container, porta fechada e servidor mudo.
- `mysql_tools.rs`: dump e restore preservam dados, view e trigger nos dois runners; a senha não
  aparece em argumentos, ambiente do container nem erros; `\!`/`system` dentro do dump não executam
  nada; cliente ausente; dump truncado.
- `apps/cli/tests/mysql.rs`: ciclo `connection test` → `backup run` → `backup verify` →
  `restore run` pela CLI, recusa de banco existente e de `--overwrite`, dump adulterado ou truncado
  (código 10), restore que falha no meio (código 11, banco criado apagado), e
  `risky_mysql_objects_are_caught_before_they_cost_a_backup_or_a_restore`: tabela MyISAM listada no
  JSON e no manifesto, usuário sem privilégio de `DEFINER` recusado antes de restaurar dados e
  trigger com `;` final recusado no backup (código 8, nenhum `.sql` publicado).

O runner nativo só roda quando o `mysqldump`/`mysql` instalado é compatível com o servidor 8.4. O
`ubuntu-latest` da CI traz o cliente 8.0, então lá os testes MySQL usam só o Docker. Para exigir o
nativo localmente, defina `DBU_REQUIRE_NATIVE_MYSQL=1` (o `DBU_REQUIRE_NATIVE` da CI vale só para o
PostgreSQL).

## E2E de backup e restauração

`apps/cli/tests/e2e.rs` executa o binário real da CLI contra servidores PostgreSQL efêmeros. A
origem recebe `tests/fixtures/postgres/e2e.sql`:
- dois schemas, FK, `UNIQUE`, `CHECK`, índice, sequence, view e função;
- tipos `jsonb`, `bytea`, `numeric`, `timestamptz` e arrays;
- texto com acentos/emoji e algumas centenas de linhas.

O destino é **outro container**.

| Cenário | O que comprova |
| --- | --- |
| `full_cycle_restores_identical_data_on_another_server` | `backup run` → `backup verify` → `restore run` deixa o destino idêntico à origem. Também confere permissões `0700`/`0600`, `backup list`/`restore list` e que nenhum segredo foi gravado no histórico ou nos `.meta.json`. Roda com cada runner disponível. |
| `connection_failures_have_distinct_exit_codes` | Servidor inacessível (5), senha errada (3) e banco inexistente (4), em `backup run` e `restore run`. |
| `corrupted_and_partial_archives_are_rejected` | Checksum divergente (10), arquivo truncado, `.partial` e destino ausente (2), sem criar bancos no destino. |
| `interrupted_operations_leave_no_partial_results` | Ctrl+C (SIGINT) no meio do dump: sai com 9, não sobra `.dump` nem `.partial` e o histórico fica `canceled`. |
| `destructive_restore_requires_confirmation` | Sem `--overwrite` ou com `--confirm` errado sai com 12. Com confirmação, o banco anterior é mantido em `__previous_*`. |
| `verify_and_restore_without_history` | O dump e o manifesto copiados para um ambiente com histórico vazio são verificados e restaurados idênticos. Dump alterado, `sha256` adulterado no manifesto e dump ausente saem com 10. |
| `hostile_identifiers_are_not_executed` | Nomes com `;`, `$(...)`, crases, aspas, `dbname=...` e prefixo `-` são tratados literalmente ou recusados; nada é executado nem escrito em outro banco. |

Como funciona:

- **Comparação por snapshot:** schema (colunas, constraints, índices, views, funções, sequences) e,
  por tabela, contagem + `md5` das linhas ordenadas. O snapshot não guarda dados brutos.
- **Segredos:** toda execução da CLI passa por um helper que falha o teste se a senha aparecer no
  stdout/stderr, inclusive com `--log-level debug`.
- **Resíduos:** ao fim de cada cenário, nenhum container `dbu-pg-*`, nenhum temporário no `TMPDIR`
  próprio do teste e nenhum `.partial`. Os cenários rodam em série dentro do arquivo, para essa
  checagem não ver containers de outro cenário.
- **Runners:** `docker` sempre; `native` quando `pg_dump` está instalado. Com `DBU_REQUIRE_NATIVE=1`
  (na CI), o runner nativo é obrigatório.
- **Diagnóstico:** se um cenário falhar, stdout/stderr de cada passo vão para
  `target/e2e-logs/<cenário>/` ou para `DBU_E2E_LOG_DIR`.
- **Timeout:** cada cenário tem limite de 300s.

## Revisão de testes (política de relevância)

Revisão feita ao adotar [`testes-rust.md`](testes-rust.md): **96 → 88 testes**, sem perder
nenhum cenário de risco. Cada cenário saído de um lugar continua coberto no teste indicado.

| Teste | Decisão | Motivo / onde o cenário continua coberto |
| --- | --- | --- |
| `core::project_name_is_not_empty` | removido (com a constante `PROJECT_NAME`) | Trivial; a constante ficou sem uso depois que o comando `info` saiu no DB-12 |
| `database-adapters::postgres_identifier_is_stable` | removido (com o enum `DatabaseEngine`) | Testava o texto de uma variante de um enum que não era usado em lugar nenhum. O enum também foi removido, e a nota para recriá-lo no multi-database está no card DB-15 |
| `postgres_container.rs` (3 testes) | removido | Testavam o harness e o Testcontainers, não o produto. Todo teste de integração e E2E já falha se o harness quebrar; isolamento entre servidores aparece nos E2E com origem e destino separados |
| `history::open_creates_database_and_applies_migrations` | removido | Contido em `reopening_is_idempotent`, que também abre, aplica todas as migrations e confere a versão do schema |
| `restore.rs::restores_backup_into_new_database_with_same_data` | consolidado no E2E | `full_cycle_restores_identical_data_on_another_server` compara snapshot completo em outro servidor; recebeu a checagem de `mode = create` e do vínculo com o artefato no histórico |
| `restore.rs::overwrite_requires_confirmation_and_keeps_previous_database` | consolidado no E2E | `destructive_restore_requires_confirmation` recebeu os cenários que faltavam: `--overwrite` sem `--confirm`, nenhum banco `__restore_*` sobrando e histórico com `mode = overwrite` e `previous_database` |
| `restore.rs::rejects_missing_corrupted_and_unverified_files` | reduzido e renomeado para `unverified_archive_requires_explicit_flag_and_is_recorded` | Ausente, corrompido e truncado já estão em `corrupted_and_partial_archives_are_rejected`. Ficou só o que é exclusivo: sem checksum conhecido exige `--allow-unverified`, avisa e grava `checksum_verified = 0` |
| `backup.rs::verify_list_and_logs_after_backup` | reduzido e renomeado para `backup_logs_operation_and_status_without_secrets` | `verify`, `list` e arquivo alterado já estão no E2E. Ficou o exclusivo: logs `tracing` com operação, status e duração, resultado JSON e senha fora da saída |
| demais 80 | mantidos | Cada um protege uma regra ou contrato com falha plausível própria |

Esperas fixas mantidas, conforme a seção "Tempo e `sleep`" da política:

- **Polling com prazo:** `backup.rs` (pico de memória), `e2e.rs` (aparecimento do `.partial`) e
  `restore.rs` (sessões no `template1`).
- **Espera fixa justificada:** `postgres_tools.rs::cancel_right_after_spawn_leaves_no_container`
  espera 1 s depois do cancelamento. O defeito procurado é justamente um container criado pelo
  daemon **depois** do cancelamento, e não há sinal observável para aguardar.

### Mutação manual (as regras de risco são detectadas?)

Cada regra foi quebrada temporariamente no código de produção, o teste correspondente rodou, e o
código foi restaurado. Todas as mutações foram detectadas:

| Regra quebrada | Teste que falhou |
| --- | --- |
| Arquivo de senha aceito com permissão para grupo/outros | `secret::rejects_files_readable_or_writable_by_others` |
| Mais de uma fonte de senha deixa de ser erro | `cli::password_sources_are_validated_without_leaking_contents` |
| Manifesto de outro arquivo aceito | `cli::verify_rejects_unsupported_or_foreign_manifest` |
| `dbname` sem escape | `tools::dbname_is_always_a_quoted_conninfo_value` |
| Dump gravável por outros aceito como origem | `storage::restore_source_rejects_writable_by_others` |
| Manifesto sem versão aceito | `storage::manifest_without_or_with_unknown_version_is_rejected` |
| Host remoto tratado como local (sem TLS) | `connection::remote_host_requires_tls` |
| Diretório de saída aberto não detectado | `storage::reports_output_directory_shared_with_others` |
| Sobrescrita sem conferir o nome confirmado | `e2e::destructive_restore_requires_confirmation` |
| Restore MySQL sem `--binary-mode` (DB-20) | `mysql_tools::restore_refuses_client_commands_embedded_in_the_dump` |
| Banco criado não apagado após restore MySQL com falha (DB-20) | `mysql::damaged_dumps_are_rejected_and_failed_restore_leaves_no_database` |
| Detecção de trigger/evento com `;` final desligada (DB-20) | `dump::trigger_or_event_body_ending_in_semicolon_is_unrestorable`, `mysql::risky_mysql_objects_…` |
| Sonda de `DEFINER` desligada (DB-20) | `mysql::risky_mysql_objects_…` |
| Restore MySQL aceita banco existente (DB-20) | `mysql::existing_database_and_overwrite_are_refused_without_changes` |

## Regras

- Nunca executar testes destrutivos no banco ou volume de desenvolvimento.
- Evitar credenciais reais, fixtures contendo dados pessoais e dump de produção.
- Testes de cada adaptador devem cobrir sucesso, falhas controladas, integridade e restore real.
- Em CI, integração requer runner com Docker acessível.
- Frontend Angular deve testar i18n `pt-BR`/`en-US` e fallback quando existir.
