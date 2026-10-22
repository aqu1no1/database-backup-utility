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
| `option-ext` — MPL-2.0 | licença (por crate) | runtime (`directories` → `dirs-sys` → `option-ext`, usado pelo histórico) | MPL-2.0 é copyleft fraco por arquivo; usamos a crate sem modificação e a fonte está disponível no crates.io; o texto vai no `THIRD-PARTY-LICENSES.md` | `directories`/`dirs-sys` deixarem de depender dela, ou trocarmos `directories` por código próprio | próxima atualização de Rust (DB-36) |

Nenhum advisory RustSec ignorado.

## Docker

| Controle | Evidência | Status |
| --- | --- | --- |
| Serviços de desenvolvimento só em loopback | `infra/docker/compose.yml` publica em `127.0.0.1` | ok |
| Sem segredos reais versionados | `.env` no `.gitignore`; `.env.example` com placeholders | ok |
| Testes isolados, sem volumes reais | Testcontainers com senha aleatória e portas dinâmicas; E2E verifica ausência de containers `dbu-pg-*` | ok |

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
- **Senhas de exemplo no Compose de desenvolvimento.** Os perfis opcionais `mysql` e `mongodb` têm
  valores padrão (`dev-change-me`). Exigir as variáveis quebraria o `docker compose` de quem só usa
  o PostgreSQL, porque o Compose interpola até os serviços de perfis desativados. Os serviços escutam
  só em `127.0.0.1` e o `.env.example` define valores próprios.

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
