# Segurança do MVP (v0.1.0 — PostgreSQL CLI)

Revisão do DB-17, ampliada no DB-35 (cadeia de build). Para cada controle: a evidência (código, teste
ou documento) e o status. As **regras** que guiam estas decisões estão em [`seguranca-rust.md`](seguranca-rust.md).

- **ok:** já atendido antes desta revisão.
- **corrigido:** ajustado no DB-17.
- **aceito:** limitação conhecida e documentada.

Todos os testes citados rodam na CI (`cargo test --workspace --tests`, job
`Rust / PostgreSQL Integration`) com os runners `docker` e `native`. **Só o Linux é testado.**

## Credenciais

| Controle | Evidência | Status |
| --- | --- | --- |
| Senha nunca em argumentos de processo | `.pgpass` temporário via `PGPASSFILE`, `env_remove("PGPASSWORD")` (`crates/database-adapters/src/postgres/tools.rs`); CLI sem flag de senha | ok |
| `.pgpass` privado e removido | teste `pgpass_is_private_and_removed_on_drop` | ok |
| `Debug` não revela a senha | `Password` com `Debug` = `Password(***)` e zerado no `Drop` (`crates/core/src/connection.rs`); teste `password_debug_is_redacted` | ok |
| Senha fora de stdout, stderr e logs (inclusive `--log-level debug`) | helper `Session` do E2E falha em qualquer saída com a senha (`apps/cli/tests/e2e.rs`); `failed_dump_reports_stderr_without_secrets`; `sanitize_removes_every_secret` | ok |
| Senha fora do histórico SQLite e dos manifestos | `schema_has_no_password_columns`; varredura de `DBU_DATA_DIR` e do diretório de saída no E2E (`full_cycle_…`, `scheduled_style_…`); manifesto com lista de campos permitidos | ok |
| Fontes de senha explícitas, sem ambiguidade | `SecretProvider` (`crates/core/src/secret.rs`); mais de uma fonte → exit 2; teste `password_sources_are_validated_without_leaking_contents` | ok (DB-18) |
| Arquivo de senha com permissão estrita | `read_password_file`; testes `rejects_files_readable_or_writable_by_others`, `rejects_symlinks_directories_and_bad_content` | ok (DB-18) |
| Prompt sem eco | `rpassword` em `apps/cli/src/credentials.rs` | ok |

## Conexões remotas (TLS)

| Controle | Evidência | Status |
| --- | --- | --- |
| Host remoto exige TLS com certificado e host validados | `TlsMode::Auto` → `VerifyFull` fora de loopback; testes `remote_host_requires_tls`, `local_host_can_force_tls` | ok |
| Mesma regra nos utilitários | `PGSSLMODE=verify-full` e `PGSSLROOTCERT=system` (runner Docker monta `/etc/ssl/certs:ro`) | ok |
| Sem TLS só em uso local controlado | `accepts_local_config_without_tls`; `--tls disable` recusado para host remoto | ok |

## Execução dos utilitários

| Controle | Evidência | Status |
| --- | --- | --- |
| `Command` com argumentos estruturados, sem shell | `tools.rs` usa `Command::args`; nenhum `sh -c` | ok |
| Nome do banco não vira string de conexão | `--dbname dbname='…'`; teste `dbname_is_always_a_quoted_conninfo_value` e E2E `hostile_identifiers_are_not_executed` | ok (DB-13) |
| Versão e compatibilidade conferidas antes de executar | `PgTools::detect` (`check_compatible`); exit 7 quando incompatível | ok |
| Cancelamento não deixa processo nem container | `cancel_stops_dump_and_container`, `cancel_right_after_spawn_leaves_no_container`, E2E `interrupted_operations_leave_no_partial_results` | ok (DB-13) |

## Backups

| Controle | Evidência | Status |
| --- | --- | --- |
| Diretório criado `0700`, arquivos `0600` | `created_directory_and_partial_are_private`; E2E `full_cycle_…` confere os modos | ok |
| Diretório de saída já existente e aberto | aviso no stderr (`backup run`), sem alterar o modo do diretório do usuário; testes `reports_output_directory_shared_with_others`, `shared_output_directory_triggers_a_warning` | corrigido |
| Sem sobrescrita de backup existente | `refuses_to_overwrite_existing_backup` | ok |
| Arquivo parcial nunca vira backup | `.partial` + `fsync` + `rename`; `dropping_without_commit_removes_partial_file`; E2E de interrupção | ok |
| SHA-256 e manifesto versionado | [`manifesto.md`](manifesto.md); `metadata_and_checksum_round_trip`, `manifest_without_or_with_unknown_version_is_rejected` | ok (DB-19) |

## Restore

| Controle | Evidência | Status |
| --- | --- | --- |
| Integridade antes de restaurar | checksum + manifesto + `pg_restore --list`; `unverified_archive_requires_explicit_flag_and_is_recorded`, E2E `corrupted_and_partial_archives_are_rejected` e `verify_and_restore_without_history` | ok |
| Origem segura | sem symlink e sem permissão aberta no dump ou no manifesto; `rejects_unsafe_source_files`, `restore_source_rejects_symlink_and_directory` | ok |
| Destino explícito e confirmação para operação destrutiva | `--target-database` obrigatório; `--overwrite` + nome; E2E `destructive_restore_requires_confirmation` | ok |
| Falha não deixa dados parciais | `--single-transaction`; `failed_restore_leaves_target_without_partial_data` | ok |
| Erros sanitizados | mensagens via `describe` sem senha; E2E `Session` em todos os passos | ok |

## SQLite (DB-22)

| Controle | Evidência | Status |
| --- | --- | --- |
| Origem aberta só para leitura, arquivo regular com cabeçalho SQLite | `check_database_file`; `unsafe_or_foreign_sources_are_rejected_before_any_copy`, `symlinked_source_is_rejected` | ok (DB-22) |
| Histórico interno nunca é origem nem destino | `same_file` em `apps/cli/src/sqlite.rs`; `invalid_sources_and_mixed_engine_arguments_are_rejected` | ok (DB-22) |
| Cópia consistente com escritor ativo, sem livelock | Online Backup API com reinício detectado; `copy_of_wal_database_under_concurrent_writes_is_a_consistent_snapshot` (mutação manual: sem o fallback, o teste falha por prazo) | ok (DB-22) |
| Origem corrompida não vira backup | `integrity_check` antes do `commit`; `damaged_sources_fail_without_publishing_a_backup` (truncada e com índice inconsistente; mutação manual: sem a checagem, o teste falha) | ok (DB-22) |
| Artefato sem `-journal`/`-wal`/`-shm` | `journal_mode=OFF` na cópia e `DELETE` no fim; `inspect_backup` recusa artefato em WAL; mutação manual: sem o `DELETE`, 3 testes falham | ok (DB-22) |
| Destino nunca criado pelo adaptador | destino aberto sem `CREATE`; `copy_never_creates_the_destination_file` | ok (DB-22) |
| Sem sobrescrita silenciosa no restore | `--overwrite` + `--confirm <caminho>`, anterior mantido como `.previous-<data>`, publicação por `hard_link` (sem clobber) ou `rename`; `restore_never_replaces_an_existing_file_without_confirmation` | ok (DB-22) |
| Destino em uso não é sobrescrito | `check_not_in_use`: `BEGIN EXCLUSIVE` sem espera (transação aberta) e, no Linux, varredura de `/proc/<pid>/fd` (até conexão parada); roda antes da confirmação e de novo antes da troca; `database_with_open_transaction_in_another_connection_is_in_use`, `database_held_open_by_another_process_is_in_use`, `restore_refuses_a_target_held_open_by_another_process` (mutação manual: sem cada checagem, o teste correspondente falha) | ok (DB-22) |
| `.partial` órfão removido sem tocar em backup em andamento | lock exclusivo (`flock`) no `.partial` enquanto ele existe; o `backup run` só remove o que aceita o lock e está parado há 10 min; `only_unlocked_and_old_partials_are_removed`, `backup_removes_only_old_orphan_partials`, `orphan_partial_from_an_interrupted_restore_does_not_block_a_new_one` (mutação manual: sem o lock ou sem a idade mínima, os testes falham) | ok (DB-22) |
| Backups da v0.1.x continuam restauráveis | leitor aceita manifesto v1; E2E `verify_and_restore_without_history` reescreve o manifesto de um dump real como v1 e roda verify e restore | ok (DB-22) |
| Histórico de restore sem host/porta inventados | migration `0004_restore_targets.sql` (`target_host`/`target_port` nulos juntos, coluna `engine`); `upgrading_keeps_postgres_restores_and_accepts_file_targets` | ok (DB-22) |

## MySQL (DB-20)

| Controle | Evidência | Status |
| --- | --- | --- |
| Senha nunca em argumentos nem no ambiente do processo | arquivo de opções `[client]` temporário (`0600`, apagado com o processo) via `--defaults-extra-file`, `env_remove("MYSQL_PWD")` (`crates/database-adapters/src/mysql/tools.rs`); testes `options_file_is_private_and_removed_on_drop`, `options_file_is_the_first_argument_and_password_never_is_an_argument`, `password_never_reaches_container_arguments_environment_or_errors` (inspeciona `Args`/`Env` do container durante o dump) | ok (DB-20) |
| Senha fora de stdout, stderr, erros, manifesto e histórico | `sanitize` com senha e caminho do arquivo de opções; `backup_verify_and_restore_round_trip_preserves_data` varre todas as saídas e o manifesto; erros de conexão sem a senha em `Display`/`Debug` (`connection_outcomes_are_classified_without_leaking_secrets`) | ok (DB-20) |
| Variáveis de senha separadas por SGBD | `DBU_MYSQL_PASSWORD`/`DBU_MYSQL_PASSWORD_FILE` (`apps/cli/src/credentials.rs`); uma senha do PostgreSQL exportada no shell não é enviada a um MySQL | ok (DB-20) |
| Host remoto exige TLS validado, sem fallback | mesma regra do `TlsMode`; `mysql_async` com rustls e os certificados do sistema (raízes embutidas desligadas); `mysql`/`mysqldump` com `--ssl-mode=VERIFY_IDENTITY --ssl-ca=<pacote do sistema>`; teste `connection_outcomes_…` recusa o certificado autoassinado do container | ok (DB-20) |
| Dump não executa comandos no cliente | `mysql --binary-mode` (recusa `\!`, `system` e demais comandos do cliente) e `--local-infile=0` (o arquivo não consegue ler arquivos locais com `LOAD DATA LOCAL`); teste `restore_refuses_client_commands_embedded_in_the_dump` (mutação manual: sem `--binary-mode`, o comando de shell roda e o teste falha) | ok (DB-20) |
| Dump não troca de banco | `inspect_dump` recusa `USE`, `CREATE DATABASE` e `DROP DATABASE` no início de linha; o dump é gerado sem `--databases`; teste `dumps_that_switch_or_create_databases_are_rejected` | ok (DB-20) |
| Dump truncado ou de outra ferramenta não vira backup nem é restaurado | cabeçalho do `mysqldump` e linha final `-- Dump completed` conferidos antes do `commit` e antes do restore; testes `truncated_dump_without_footer_is_incomplete`, `damaged_dumps_are_rejected_and_failed_restore_leaves_no_database` | ok (DB-20) |
| Leitura do dump com memória limitada | `dump.rs` guarda só 256 bytes por linha; `huge_lines_are_scanned_without_being_kept` | ok (DB-20) |
| Nome do banco não vira opção nem caminho | `--` antes do nome no `mysqldump`, `--database=<nome>` no `mysql`; nomes com `/`, `\`, `.` ou espaço final recusados (`mysql_config_applies_mysql_name_limits_and_tls_rules`) | ok (DB-20) |
| Sem sobrescrita: restore só em banco novo | banco existente (mesmo vazio) e `--overwrite` recusados com código 2; `existing_database_and_overwrite_are_refused_without_changes` | ok (DB-20) |
| Restore com falha não deixa banco incompleto | o restore apaga só o banco que ele criou, em falha, cancelamento ou contagem de tabelas divergente; `damaged_dumps_…` (mutação manual: sem o `DROP`, o teste falha) | ok (DB-20) |
| Backup que o MySQL não restauraria não é publicado | trigger ou evento com o corpo terminado em `;` (criado por cliente que envia vários comandos de uma vez) é detectado na leitura do dump; `backup run` falha com código 8 e `verify` com 10, citando o objeto; testes `trigger_or_event_body_ending_in_semicolon_is_unrestorable`, `long_trigger_body_is_judged_by_its_end`, `trigger_saved_with_trailing_semicolon_is_flagged_as_unrestorable` (confirma que o próprio `mysql` falha nesse dump), `risky_mysql_objects_…` (mutação manual: sem a detecção, os testes falham) | ok (DB-20) |
| `DEFINER` sem privilégio detectado antes de restaurar dados | o dump informa os `DEFINER` por tipo de objeto; logo depois de criar o banco, o restore cria e apaga um objeto de cada tipo com cada `DEFINER` (sonda real, que cobre roles, binlog e `SYSTEM_USER`, o que uma consulta a `USER_PRIVILEGES` não cobriria); se falhar, apaga o banco e sai com código 2; testes `definer_probe_refuses_a_user_that_cannot_recreate_the_objects` (também confere que a sonda não deixa objetos), `probe_quotes_definer_and_names_from_the_dump`, `risky_mysql_objects_…` (mutação manual: sem a sonda, o teste falha) | ok (DB-20) |
| Tabelas fora do InnoDB visíveis para scripts | lista `non_transactional_tables` no JSON do `backup run` e no manifesto (vazia quando todas são InnoDB), além do aviso no log; `risky_mysql_objects_…`, `backup_verify_and_restore_round_trip_preserves_data` | ok (DB-20) |
| MariaDB não é tratado como MySQL | `MysqlVersion::parse` recusa versões MariaDB (servidor e cliente); `parses_client_and_server_versions`; suporte planejado no DB-37 | ok (DB-20) |

## MongoDB (DB-21)

| Controle | Evidência | Status |
| --- | --- | --- |
| Senha nunca em argumentos nem no ambiente do processo | arquivo YAML temporário (`0600`, apagado com o processo) com `password: "..."`, passado com `--config=` como primeiro argumento do `mongodump`/`mongorestore` (`crates/database-adapters/src/mongo/tools.rs`); nenhuma variável de ambiente de senha é definida; testes `config_file_is_private_escaped_and_removed_on_drop`, `connection_args_start_with_the_config_file_and_never_carry_the_password`, `password_never_reaches_container_arguments_environment_or_errors` (inspeciona `Args`/`Env`/`Cmd` do container durante o dump) | ok (DB-21) |
| Senha com aspas, barra invertida e outros caracteres chega íntegra às ferramentas | `yaml_quote` escapa `\`, `"`, controles e as quebras de linha do YAML; testes `yaml_values_are_quoted_and_escaped` e `special_characters_in_the_password_reach_the_tools_intact` (usuário real, banco de autenticação diferente de `admin`, nos dois runners; mutação manual: sem escapar `"` e `\`, o teste falha) | ok (DB-21) |
| Nenhum comando de shell | `Command::args` com `--opção=valor`; o nome do usuário `x;touch <marcador>$(touch <marcador>)` não cria o marcador (`password_never_reaches_…`) | ok (DB-21) |
| Senha fora de stdout, stderr, erros, manifesto, histórico e temporários | `sanitize` com a senha e o caminho do arquivo YAML; `backup_verify_and_restore_round_trip_preserves_data_on_each_topology` varre saídas (inclusive com `--log-level debug`), manifesto e `history.sqlite3` e confere que o `TMPDIR` ficou vazio; erros de conexão sem a senha em `Display`/`Debug` (`connection_outcomes_are_classified_without_leaking_secrets`) | ok (DB-21) |
| Variáveis de senha separadas por SGBD | `DBU_MONGO_PASSWORD`/`DBU_MONGO_PASSWORD_FILE` (`apps/cli/src/credentials.rs`); `mongo_password_never_comes_from_postgres_variables` | ok (DB-21) |
| Host remoto exige TLS validado, sem fallback | mesma regra do `TlsMode`; driver com `ca_file_path` apontando para o pacote de certificados do sistema (com ele o `webpki-roots` não entra no `RootCertStore`, conferido na fonte do `mongodb` 3.9.1) e ferramentas com `--tls --tlsCAFile=<pacote>`, nunca `--tlsInsecure`; `connection_outcomes_…` e `existing_database_and_overwrite_…` recusam `--tls verify-full` contra um servidor sem TLS (código 6) | ok (DB-21) |
| Um host só, sem descoberta de membros | `MongoConnectionConfig` recusa lista de hosts e nome de replica set; o driver conecta com `directConnection` e as ferramentas recebem `--host`/`--port`; spike com servidor "isca" na porta local | ok (DB-21) |
| Nome do banco não vira opção nem padrão | `--db=<nome>` e `--nsInclude/--nsFrom/--nsTo=<nome>.*`, com nomes sem `/`, `\`, `.`, espaço, `"`, `$`, `*`, `<`, `>`, `:`, `\|` ou `?`, sem nulo e sem `admin`, `local` ou `config` (`mongodb_config_validates_names_hosts_and_tls`); o nome de origem vem do archive e é validado antes de qualquer conexão (`damaged_archives_…`: nome com `*` sai com código 10) | ok (DB-21) |
| Archive de outro formato ou do servidor inteiro não é aceito | número mágico, um único banco; `foreign_files_are_not_archives`, `archives_with_more_than_one_database_are_rejected` | ok (DB-21) |
| Archive truncado ou corrompido não vira backup nem é restaurado | todo corte do arquivo é recusado como incompleto (`truncated_archive_is_incomplete` testa todos os tamanhos); `backup run` lê o archive antes do `commit`; `damaged_archives_…` (byte trocado, truncado, de outro formato) | ok (DB-21) |
| Leitura do archive com memória limitada | `BufReader` de 64 KiB, documentos contados e pulados, tamanho declarado entre 5 B e 16 MiB + 16 KiB, no máximo 100 mil coleções; `oversized_length_is_rejected_without_allocating` | ok (DB-21) |
| Sem sobrescrita: restore só em banco novo | banco com qualquer coleção ou view (mesmo só uma) e `--overwrite` recusados com código 2; o `mongorestore` roda sem `--drop`; `existing_database_and_overwrite_are_refused_without_changes` e, no adaptador, um segundo restore no mesmo destino falha sem alterá-lo (`dump_and_restore_preserve_…`) | ok (DB-21) |
| Restore com falha não deixa banco incompleto | o restore apaga só o banco que ele mesmo criou, em falha, cancelamento ou conteúdo divergente; `damaged_archives_…` cobre `_id` duplicado e um valor alterado (o `mongorestore` falha no CRC depois de gravar os documentos); mutação manual: sem o `dropDatabase`, o teste falha | ok (DB-21) |
| Dados conferidos, não só o código de saída | depois do `mongorestore`, a contagem de documentos de cada coleção e a lista de views do destino são comparadas com as do archive (`differences` nomeia as divergências); `dump_and_restore_preserve_…` compara documentos (com os tipos BSON), índices, validador e view | ok (DB-21) |
| Só topologias testadas | `Sharded` e secundário recusados com código 2 em backup e restore (`topology_comes_from_the_hello_reply`, `only_standalone_and_primary_are_supported`); `replica_set_primary_is_detected` | ok (DB-21) |
| Coleções time series recusadas | o `mongodump` grava os dados em `system.buckets.<nome>`, fora do prelúdio; `time_series_collections_are_not_supported` e o último passo de `damaged_archives_…` (código 8, nenhum arquivo publicado) | ok (DB-21) |
| Ferramentas ausentes ou incompatíveis geram erro claro | `missing_tools_are_reported`, `old_or_legacy_native_tools_are_refused` (scripts que imprimem `100.9.4` e `r4.2.24`), `missing_tools_fail_with_code_7_and_a_hint` (código 7, cita `--runner docker`) | ok (DB-21) |
| Backup sem instantâneo de um único instante avisado | `warn` e linha no resultado de todo `backup run`; ver "Riscos aceitos" e [`arquitetura.md`](arquitetura.md#mongodb-topologia-e-consistência) | aceito (DB-21) |

## GitHub Actions e release

| Controle | Evidência | Status |
| --- | --- | --- |
| Token com privilégio mínimo | `permissions: contents: read` no topo de `ci.yml`, `audit.yml` e `release.yml`; `contents: write` só no job de release | ok |
| Sem `pull_request_target` | `ci.yml` usa `pull_request` | ok |
| Release só por `workflow_dispatch` | `release.yml`; validação de versão/changelog (`.github/scripts/release_notes.py`) | ok |
| `Cargo.lock` versionado e usado com `--locked` | `release.yml` falha sem lockfile; `ci.yml` e `release.yml` rodam clippy, testes e build com `--locked` | corrigido (DB-35: CI passou a usar `--locked`) |
| Actions fixadas por commit SHA | `uses: <action>@<sha> # <tag>` em todos os workflows; atualização descrita em [`CI.md`](CI.md) | corrigido |
| Shell dos workflows sem avisos | `actionlint` sem avisos (corrigido o `SC2035` do `release.yml`) | corrigido |
| CI vermelha bloqueia merge | checks obrigatórios na `main` (ver [`CI.md`](CI.md)) | **pendente**: será aplicado no fim do DB-35, com confirmação do mantenedor (até lá a `main` não tem proteção) |
| Proibição de `unwrap`/`expect` em produção | `[workspace.lints.clippy]` com `deny` + `clippy.toml` liberando só em testes; CI roda clippy com `-D warnings` | corrigido |
| Proibição de `unsafe` | `[workspace.lints.rust] unsafe_code = "forbid"`; um bloco `unsafe` temporário faz o build falhar | corrigido (DB-35) |
| Toolchain fixada e fonte única | `rust-toolchain.toml` (1.99.1); workflows usam `rustup toolchain install` (saiu a action `dtolnay/rust-toolchain`); atualização em [`seguranca-rust.md`](seguranca-rust.md#51-toolchain-fixada) | corrigido (DB-35) |

## Cadeia de dependências

| Controle | Evidência | Status |
| --- | --- | --- |
| Advisories conhecidos (RustSec) em todo PR | job `Rust / Supply Chain` (`cargo deny check`); `audit.yml` semanal com `cargo deny check advisories` | corrigido (DB-35) |
| Licenças compatíveis com MIT OR Apache-2.0 | `deny.toml` com allowlist das licenças em uso; mutação: tirar `ISC` ou a exceção do `option-ext` faz o check falhar | corrigido (DB-35) |
| Só crates.io como fonte | `deny.toml` `[sources]`; git ou registry desconhecido falham | corrigido (DB-35) |
| Sem versões wildcard | `deny.toml` `wildcards = "deny"`; caminhos internos liberados só porque as crates são `publish = false` | corrigido (DB-35) |
| Avisos de licença de terceiros no binário | `THIRD-PARTY-LICENSES.md` gerado por `cargo-about` (só dependências de runtime) e incluído no tarball | corrigido a partir da v0.1.1 (DB-35); **o tarball da v0.1.0 não tem esse arquivo** |

### Exceções ativas

Toda exceção na política de dependências fica aqui, com motivo e condição de remoção (triagem no
§4.4 de [`seguranca-rust.md`](seguranca-rust.md#44-triagem-de-achados)). Revisão a cada atualização de toolchain (card recorrente no Linear).

| Exceção | Tipo | Alcance | Motivo | Condição de remoção | Revisar em |
| --- | --- | --- | --- | --- | --- |
| `webpki-roots` — CDLA-Permissive-2.0 | licença (por crate) | runtime (`mysql_async` e `mongodb` → `webpki-roots`, puxada pela feature `rustls-tls` de cada um, sem como desligar) | Licença permissiva de dados (a lista de CAs da Mozilla); o código usa os certificados do sistema e não usa as raízes embutidas (`with_disable_built_in_roots(true)` no MySQL, `ca_file_path` no MongoDB), então a lista só entra no binário. `about.toml` aceita a licença só para esta crate | `mysql_async` e `mongodb` permitirem rustls sem `webpki-roots`, ou trocarmos de cliente | próxima atualização de Rust (DB-36) |
| `tiny-keccak` — CC0-1.0 | licença (por crate) | só compilação (`mongodb` → `mongodb-internal-macros` → `macro_magic` → `const-random-macro` → `tiny-keccak`): roda dentro do proc-macro `const-random-macro`, que gera constantes em tempo de compilação, e não é ligada ao binário | CC0-1.0 é a dedicação ao domínio público, sem restrição de uso, mas não estava na allowlist. Exceção por crate no `deny.toml` e no `about.toml` (o `cargo about` lista a crate, porque não distingue dependências só de compilação) | `mongodb` deixar de depender do `macro_magic`, ou o `const-random-macro` deixar de usar `tiny-keccak`, ou trocarmos de cliente | próxima atualização de Rust (DB-36) |
| `option-ext` — MPL-2.0 | licença (por crate) | runtime (`directories` → `dirs-sys` → `option-ext`, usado pelo histórico) | MPL-2.0 é copyleft fraco por arquivo; usamos a crate sem modificação e a fonte está disponível no crates.io; o texto vai no `THIRD-PARTY-LICENSES.md` | `directories`/`dirs-sys` deixarem de depender dela, ou trocarmos `directories` por código próprio | próxima atualização de Rust (DB-36) |

Nenhum advisory RustSec ignorado.

## Docker

| Controle | Evidência | Status |
| --- | --- | --- |
| Serviços de desenvolvimento só em loopback | `infra/docker/compose.yml` publica em `127.0.0.1` | ok |
| Sem segredos reais versionados | `.env` no `.gitignore`; `.env.example` com placeholders | ok |
| Testes isolados, sem volumes reais | Testcontainers com senha aleatória e portas dinâmicas (PostgreSQL, MySQL e MongoDB); E2E verifica ausência de containers `dbu-pg-*`; `password_never_…` verifica a remoção dos containers `dbu-my-*` e `dbu-mg-*` cancelados | ok |

## Riscos aceitos e limites

- **Checksum não autentica.** Quem altera o dump e o manifesto juntos gera um par válido. A
  mitigação é a criptografia/assinatura do DB-31 (v0.4.0). Ver [`manifesto.md`](manifesto.md#limites).
- **TOCTOU no restore.** O checksum é calculado e depois o `pg_restore` relê o arquivo pelo caminho.
  As permissões exigidas da origem reduzem a janela, mas não a eliminam.
- **Runner Docker exige o socket do Docker.** Quem pode usar o Docker tem, na prática, privilégio de
  root na máquina. Para destinos locais, o container usa `--network host` para alcançar
  `localhost`. Em servidores onde isso não é aceitável, use `--runner native`.
- **`DBU_PG_PASSWORD` é visível** em `/proc/<pid>/environ` para o próprio usuário e o root.
  Prefira `--password-file`.
- **Dono do arquivo de senha não é conferido.** Só as permissões são.
- **v0.1.0 sem avisos de terceiros.** O tarball publicado em 2026-10-21 não traz as notas de licença
  das dependências embutidas. Corrigido no workflow (DB-35); a v0.1.1 já inclui o arquivo. Use a v0.1.1 em vez da v0.1.0.
- **SQLite: `-wal`/`-shm` ao lado da origem.** Em modo WAL, o próprio SQLite cria esses arquivos
  ao lado da origem, mesmo numa abertura só para leitura, e eles podem ficar depois do backup. É o
  comportamento de qualquer leitor WAL e não altera os dados. Apagá-los seria pior: se a aplicação
  abrir o banco no meio, ela perde dados. A alternativa (`immutable=1`) daria leituras erradas com
  um escritor ativo. O `backup run` avisa quando os criou.
- **SQLite: limites da detecção de destino em uso.** A varredura de `/proc` só existe no Linux e só
  enxerga processos do mesmo usuário (ou todos, como root). No macOS, no Windows ou com a aplicação
  rodando como outro usuário, só a checagem de lock vale, e ela não vê uma conexão parada em modo
  rollback. Nesses casos, feche a aplicação antes do `--overwrite`. Ainda resta uma janela pequena
  entre a última checagem e a troca do arquivo.
- **Manifesto v2 não é lido pela v0.1.x.** A v0.1.x recusa com código 10 e mensagem clara, sem
  alterar nada. Use no restore uma versão igual ou mais nova que a do backup. O contrário é
  garantido e testado: a versão atual lê manifestos v1.
- **SQLite: escritores em modo rollback esperam.** Quando a cópia é reiniciada por escrita
  concorrente, o restante é copiado num passo só e, em modo rollback (não WAL), os escritores esperam
  até ele terminar. Em WAL, não.
- **`.partial` órfão recente.** Um `.partial` deixado por `kill -9` ou queda de energia só é
  removido pelo próximo `backup run` depois de 10 minutos parado. A espera protege um `.partial` de
  uma v0.1.x, que não usa lock, rodando no mesmo diretório. Até lá ele só ocupa espaço: nunca vira
  backup. Em sistemas de arquivos de rede, o `flock` pode não funcionar entre máquinas.
- **MySQL: consistência só para InnoDB.** O `--single-transaction` dá um snapshot consistente só de
  tabelas transacionais. Tabelas MyISAM, MEMORY etc. podem ficar inconsistentes entre si se houver
  escrita durante o backup. O `backup run` lista essas tabelas no log, no JSON e no manifesto
  (`non_transactional_tables`); não usamos `--lock-all-tables` porque ele bloqueia escritas no
  servidor inteiro.
- **MySQL: restore não transacional e sem sobrescrita.** O dump SQL é aplicado comando a comando;
  uma falha no meio deixa o banco parcial, que o restore apaga em seguida. Se o `DROP` falhar (por
  exemplo, sem permissão), a mensagem avisa que o banco incompleto ficou. Restaurar sobre um banco
  existente ainda não é possível.
- **MySQL: `DEFINER` e privilégios.** Triggers, views e rotinas levam o `DEFINER` da origem. Para
  recriá-los com outro usuário, o destino exige `SET_ANY_DEFINER` (MySQL 8.2+; `SET_USER_ID` antes)
  ou `SUPER`; com o binlog ligado (padrão), triggers e functions exigem `SUPER` ou
  `log_bin_trust_function_creators=1`. A sonda recusa o restore antes de carregar dados, mas a
  solução continua sendo conceder o privilégio ou restaurar com o usuário certo. O backup de eventos
  e rotinas exige `EVENT` e `SHOW_ROUTINE`.
- **MySQL: trigger ou evento com `;` no fim do corpo.** É um defeito do MySQL: o objeto criado por um
  cliente que envia vários comandos de uma vez gera um dump que o próprio `mysql` não reaplica. O
  `backup run` agora recusa esse dump (código 8) e diz qual objeto recriar. A detecção olha o fim
  do comando (últimos 32 bytes); uma variação que o `mysqldump` não gera hoje passaria despercebida
  e cairia na falha limpa do restore (código 11, banco apagado).
- **MySQL: chave RSA sem TLS em `localhost`.** Sem TLS (permitido só em `localhost`), o
  `caching_sha2_password` precisa da chave pública do servidor, pedida com
  `--get-server-public-key`. Ela não é validada, o que só é aceitável porque o tráfego não sai da
  máquina.
- **MySQL com `--allow-unverified` executa SQL de terceiros.** Um `.sql` sem checksum conhecido roda
  com as permissões do usuário informado, dentro do banco novo. As recusas de `USE`/`CREATE DATABASE`
  e os comandos do cliente desligados reduzem o alcance, mas não substituem a confiança na origem
  do arquivo: o SQL ainda pode, por exemplo, alterar outros bancos com nome qualificado se o usuário
  tiver permissão. Use um usuário de restore com privilégios só no banco de destino.
- **Senhas de exemplo no Compose de desenvolvimento.** Os perfis opcionais `mysql` e `mongodb` têm
  valores padrão (`dev-change-me`). Exigir as variáveis quebraria o `docker compose` de quem só usa
  o PostgreSQL, porque o Compose interpola até os serviços de perfis desativados. Os serviços escutam
  só em `127.0.0.1` e o `.env.example` define valores próprios.

- **MongoDB: sem instantâneo de um único instante.** O `mongodump` de um banco lê cada coleção por
  uma consulta própria, em paralelo, e o `--oplog` (o mecanismo de instantâneo) só existe para o
  dump do servidor inteiro. Escritas durante o backup podem deixar coleções inconsistentes entre si,
  em standalone e em replica set. A CLI avisa em todo backup; veja
  [`arquitetura.md`](arquitetura.md#mongodb-topologia-e-consistência).
- **MongoDB: restore não transacional e sem sobrescrita.** O `mongorestore` insere coleção a
  coleção; uma falha no meio deixa o banco parcial, que o restore apaga em seguida (`dropDatabase`,
  que exige o privilégio `dropDatabase`, por exemplo com `dbAdmin` ou `dbOwner`). Se o `dropDatabase`
  falhar, a mensagem avisa que o banco incompleto ficou. Restaurar sobre um banco existente não é
  possível.
- **MongoDB: janela entre a checagem do destino e a primeira escrita.** O MongoDB cria o banco na
  primeira escrita e não tem um `CREATE DATABASE` que falhe quando ele já existe. Se outro cliente
  criar o mesmo banco depois da checagem e antes de o `mongorestore` gravar, uma falha do restore
  apagaria o banco inteiro, inclusive o que esse cliente gravou. A janela dura o tempo de subir o
  container do runner Docker (segundos).
- **MongoDB: usuários e roles fora do backup.** O dump é de um banco sem `--dumpDbUsersAndRoles`:
  contas e permissões não são salvas nem restauradas.
- **MongoDB: `--bypassDocumentValidation` no restore.** Documentos legados que violam o validador
  da coleção são restaurados (o validador também é restaurado). Isso exige o privilégio
  `bypassDocumentValidation` no banco de destino, que `dbAdmin`, `dbOwner` e `restore` têm.
- **MongoDB: a senha fica copiada em `String` dentro do driver.** O `Credential` do driver guarda a
  senha sem zerar a memória durante a conexão. O `Password` do projeto zera os bytes, mas a cópia do
  driver não.
- **MongoDB com `--allow-unverified` grava dados de terceiros.** Um archive sem checksum conhecido
  grava os documentos, índices, validadores e pipelines de views dele no banco novo, com as
  permissões do usuário informado. O formato carrega dados e metadados, não comandos, e a leitura do
  archive confere a estrutura; o `mongorestore` ainda confere o CRC de cada coleção. Mesmo assim,
  use um usuário de restore com privilégios só no banco de destino.
- **MongoDB: Unauthorized (código 13) na conexão vira falha de autenticação (código 3).** É o mesmo
  critério do erro 1044 do MySQL, mesmo quando o usuário autenticou e só não enxerga o banco.
- **MongoDB: time series não são suportadas.** O `mongodump` grava os dados delas em
  `system.buckets.<nome>`. O backup roda o `mongodump` até o fim e só então recusa o archive (código
  8, nada é publicado); o restore de um archive assim sai com código 10. Não há como excluir a
  coleção do backup.
- **MongoDB: limites da senha.** O SCRAM-SHA-256 aplica o SASLprep: senhas com caracteres de controle
  ou fora do Unicode 3.2 (como emoji) não autenticam, e o próprio servidor não deixa criá-las.
- **MongoDB: IPv6 e `--host` com colchetes** não foram testados.
- **Docker: o cancelamento deixa volumes anônimos vazios.** O cancelamento remove o container com
  `docker rm --force`, sem `--volumes`. Imagens que declaram `VOLUME` (`postgres`, `mysql`, `mongo`)
  deixam um ou dois volumes anônimos vazios por cancelamento. Já existia para PostgreSQL e MySQL; o
  runner compartilhado (`process.rs`) não foi alterado no DB-21. Limpe com `docker volume prune`
  depois de conferir o que será apagado.

## Diferenças por sistema operacional (não testadas)

Nada abaixo foi testado: a CI e os testes rodam só em Linux. Vale para a fase de distribuição.

| Tema | Linux (testado) | macOS | Windows |
| --- | --- | --- | --- |
| Permissões de backup e arquivo de senha | `0700`/`0600` aplicados e conferidos | mesmo modelo POSIX, esperado igual | sem modo POSIX: a CLI não confere permissões; proteger por ACL |
| Cancelamento | SIGINT (Ctrl+C) | SIGINT | Ctrl+C via console; sem `kill -INT` |
| Runner Docker com TLS | monta `/etc/ssl/certs` | Docker Desktop; caminho de certificados diferente | Docker Desktop; montagem de certificados não suportada hoje |
| Cofre de credenciais | arquivo de senha (Secret Service futuro) | Keychain (futuro) | Credential Manager (futuro) |
| `--network host` | suportado | limitado no Docker Desktop | limitado no Docker Desktop |

## Como revalidar

```bash
cargo clippy --workspace --all-targets -- -D warnings   # inclui a proibição de unwrap/expect
cargo test --workspace                                  # Docker necessário para integração e E2E
DBU_REQUIRE_NATIVE=1 cargo test -p database-backup-cli --test e2e   # com pg_dump 17 instalado
docker run --rm -v "$PWD":/repo -w /repo rhysd/actionlint:latest -no-color
gh api repos/aqu1no1/database-backup-utility/branches/main/protection   # checks obrigatórios
```
