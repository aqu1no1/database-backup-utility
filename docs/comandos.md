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
```

Sem `DBU_PG_PASSWORD` a senha é pedida sem eco. Exit codes: 0 ok, 2 parâmetro inválido,
3 autenticação, 4 banco inexistente, 5 inacessível/timeout, 6 TLS.

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
