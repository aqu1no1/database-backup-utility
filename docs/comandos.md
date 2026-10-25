# Comandos essenciais

Referência rápida para rodar, testar e validar o projeto. Execute tudo a partir da raiz do repositório.

## Requisitos

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust (cargo, rustfmt, clippy)
sudo apt install -y build-essential                              # linker C usado pelo Rust
docker info                                                      # Docker Engine + Compose v2 ativos
```

## CLI

```bash
cargo run -p database-backup-cli -- --help
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- connection test --database my_database --user postgres
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- backup run --database my_database --user postgres
cargo run -p database-backup-cli -- backup list --json
cargo run -p database-backup-cli -- backup verify --file <arquivo.dump>
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- restore run --file <arquivo.dump> --target-database my_database_copy --user postgres
cargo run -p database-backup-cli -- restore list
cargo run -p database-backup-cli -- backup run --engine sqlite --source ./app.db
cargo run -p database-backup-cli -- backup verify --file <arquivo.sqlite3>
cargo run -p database-backup-cli -- restore run --engine sqlite --file <arquivo.sqlite3> --target ./app_copia.db
DBU_MYSQL_PASSWORD='...' cargo run -p database-backup-cli -- connection test --engine mysql --database my_database --user backup
DBU_MYSQL_PASSWORD='...' cargo run -p database-backup-cli -- backup run --engine mysql --database my_database --user backup
cargo run -p database-backup-cli -- backup verify --file <arquivo.sql>
DBU_MYSQL_PASSWORD='...' cargo run -p database-backup-cli -- restore run --engine mysql --file <arquivo.sql> --target-database my_database_copy --user backup
DBU_MONGO_PASSWORD='...' cargo run -p database-backup-cli -- connection test --engine mongodb --database my_database --user backup --auth-database admin
DBU_MONGO_PASSWORD='...' cargo run -p database-backup-cli -- backup run --engine mongodb --database my_database --user backup
cargo run -p database-backup-cli -- backup verify --file <arquivo.archive>
DBU_MONGO_PASSWORD='...' cargo run -p database-backup-cli -- restore run --engine mongodb --file <arquivo.archive> --target-database my_database_copy --user backup
```

`--engine` vale `postgres` (padrão), `mysql`, `mongodb` ou `sqlite`. Com `sqlite` não há servidor nem senha: `--source`
(backup) e `--target` (restore) indicam os arquivos, e as opções de conexão do PostgreSQL são recusadas.
Com `mysql`, a porta padrão é 3306 e o restore só cria bancos novos (`--overwrite` é recusado).
Com `mongodb`, a porta padrão é 27017; `--auth-database` (padrão `admin`; só vale com este SGBD)
indica o banco onde o usuário foi criado; o restore só cria bancos novos (`--overwrite` é recusado);
só são aceitos um standalone ou o primário de um replica set; e o backup é de um banco, sem
snapshot de um único instante (a CLI avisa). Permissões necessárias em
[`configuracao.md`](configuracao.md#mongodb-usuário-banco-de-autenticação-e-permissões).
`backup verify` descobre o SGBD pelo manifesto ou pelo cabeçalho do arquivo, sem `--engine`.

Arquivos `.partial` que sobram no diretório de backups vêm de um backup interrompido à força
(`kill -9`, queda de energia). Ctrl+C, timeout e erros os removem na hora. Cada backup em andamento
segura um lock no próprio `.partial`, e o sistema solta esse lock quando o processo morre. O
`backup run` remove sozinho os `.partial` que aceitam o lock (ninguém está usando) e estão parados há
mais de 10 minutos, e registra isso no log. Um `<destino>.partial` órfão de um restore SQLite também
é removido no próximo restore.

Opções globais: `--json` (resultado em JSON no stdout) e `--log-level error|warn|info|debug|trace`
(logs no stderr; padrão `warn` ou `RUST_LOG`).

A senha vem de `--password-file`, `DBU_PG_PASSWORD_FILE` ou `DBU_PG_PASSWORD` (MySQL: `DBU_MYSQL_PASSWORD_FILE` ou `DBU_MYSQL_PASSWORD`; MongoDB: `DBU_MONGO_PASSWORD_FILE` ou `DBU_MONGO_PASSWORD`), só uma; sem nenhuma, é pedida sem eco. Exit codes: 0 ok, 2 parâmetro inválido,
3 autenticação, 4 banco inexistente, 5 inacessível/timeout, 6 TLS, 7 ferramentas ausentes,
8 backup falhou, 9 cancelado, 10 arquivo inválido, 11 restore falhou,
12 confirmação necessária. `restore run` e `backup verify` recusam (código 10) symlinks e
arquivos graváveis por grupo/outros.

## PostgreSQL de desenvolvimento

```bash
cp .env.example .env                                                       # uma vez; troque a senha
docker compose --env-file .env -f infra/docker/compose.yml up -d postgres
docker compose --env-file .env -f infra/docker/compose.yml ps
docker compose --env-file .env -f infra/docker/compose.yml down
```

> `down -v` apaga o volume com os dados locais. Use só quando quiser zerar o banco.

## Testes

```bash
cargo test --workspace --lib --bins                                   # unitários, sem Docker
cargo test --workspace --tests                                        # tudo, inclui Testcontainers (Docker)
cargo test -p database-backup-adapters --test postgres_connection     # teste de conexão
cargo test -p database-backup-history                                 # histórico SQLite
cargo test -p database-backup-adapters --test postgres_tools          # pg_dump/pg_restore (native e Docker)
cargo test -p database-backup-cli --test backup                       # backup ponta a ponta
cargo test -p database-backup-adapters --test postgres_restore        # pg_restore e administração de bancos
cargo test -p database-backup-cli --test restore                      # restore ponta a ponta
cargo test -p database-backup-cli --test cli                          # argumentos, ajuda e JSON (sem Docker)
cargo test -p database-backup-cli --test e2e                          # ciclos completos (Docker)
cargo test -p database-backup-adapters --test mongo_connection        # conexão MongoDB (Docker)
cargo test -p database-backup-adapters --test mongo_tools             # mongodump/mongorestore (Docker)
cargo test -p database-backup-cli --test mongo                        # backup/verify/restore MongoDB (Docker)
cargo test -p database-backup-cli --test e2e -- hostile               # um cenário pelo nome
```

Os testes de integração criam PostgreSQL, MySQL e MongoDB temporários e não usam os bancos do Compose.

## Qualidade

```bash
cargo fmt --all                                        # formata
cargo clippy --workspace --all-targets -- -D warnings  # lint
cargo build --workspace                                # compila
```

## Antes de abrir um PR

Mesmos passos da CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins
cargo test --workspace --tests
cargo build --workspace
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```
