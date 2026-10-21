# Segurança do MVP (v0.1.0 — PostgreSQL CLI)

Revisão do DB-17. Para cada controle: a evidência (código, teste ou documento) e o status.

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

## GitHub Actions e release

| Controle | Evidência | Status |
| --- | --- | --- |
| Token com privilégio mínimo | `permissions: contents: read` no topo de `ci.yml`, `audit.yml` e `release.yml`; `contents: write` só no job de release | ok |
| Sem `pull_request_target` | `ci.yml` usa `pull_request` | ok |
| Release só por `workflow_dispatch` | `release.yml`; validação de versão/changelog (`.github/scripts/release_notes.py`) | ok |
| `Cargo.lock` versionado e usado com `--locked` | `release.yml` falha sem lockfile | ok |
| Actions fixadas por commit SHA | `uses: <action>@<sha> # <tag>` em todos os workflows; atualização descrita em [`CI.md`](CI.md) | corrigido |
| Shell dos workflows sem avisos | `actionlint` sem avisos (corrigido o `SC2035` do `release.yml`) | corrigido |
| CI vermelha bloqueia merge | checks obrigatórios na `main` (ver [`CI.md`](CI.md)) | corrigido no fim do DB-17, com confirmação do mantenedor |
| Proibição de `unwrap`/`expect` em produção | `[workspace.lints.clippy]` com `deny` + `clippy.toml` liberando só em testes; CI roda clippy com `-D warnings` | corrigido |

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
