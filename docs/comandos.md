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
cargo run -p database-backup-cli -- info
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- test-connection --database my_database --user postgres
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- backup --database my_database --user postgres
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- restore --file <arquivo.dump> --target-database my_database_copy --user postgres
```

Sem `DBU_PG_PASSWORD` a senha é pedida sem eco. Exit codes: 0 ok, 2 parâmetro inválido,
3 autenticação, 4 banco inexistente, 5 inacessível/timeout, 6 TLS, 7 ferramentas ausentes,
8 backup falhou, 9 cancelado, 10 arquivo inválido, 11 restore falhou,
12 confirmação necessária. O `restore` recusa (código 10) symlinks e arquivos graváveis por
grupo/outros.

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
cargo test -p database-backup-adapters --test postgres_container      # só a integração PostgreSQL
cargo test -p database-backup-adapters --test postgres_connection     # teste de conexão
cargo test -p database-backup-history                                 # histórico SQLite
cargo test -p database-backup-adapters --test postgres_tools          # pg_dump/pg_restore (native e Docker)
cargo test -p database-backup-cli --test backup                       # backup ponta a ponta
cargo test -p database-backup-adapters --test postgres_restore        # pg_restore e administração de bancos
cargo test -p database-backup-cli --test restore                      # restore ponta a ponta
cargo test -p database-backup-adapters --test postgres_container -- seed_fixture_is_applied
```

Os testes de integração criam PostgreSQL temporários e não usam o banco do Compose.

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
