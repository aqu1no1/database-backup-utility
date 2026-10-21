# Database Backup Utility

Ferramenta multiplataforma de backup e restauração de bancos de dados. O produto terá uma
**CLI independente em Rust** e, futuramente, um **desktop Tauri + Angular + Tailwind v4**,
ambos utilizando as mesmas crates de negócio.

> **Estado atual:** teste de conexão, backup full e restore PostgreSQL pela CLI, histórico interno
> em SQLite, testes de integração, Docker Compose e pipelines.
> As entregas funcionais estão organizadas nas issues do Linear.

## Stack

- CLI: Rust + Clap; bibliotecas compartilhadas no Cargo Workspace.
- Desktop (planejado): Tauri + Angular standalone + TypeScript + Tailwind v4.
- i18n desktop: Transloco, `pt-BR` e `en-US`.
- Desenvolvimento: PostgreSQL no Docker Compose; MySQL/MongoDB opcionais; histórico interno em SQLite embutido.
- Testes: Rust `cargo test`, integração com Testcontainers, Vitest futuramente no Angular.
- GitHub Actions: CI em PRs/main, auditoria e releases **somente manuais** (`workflow_dispatch`).

## Requisitos locais

- Rust estável (`rustup`, Cargo), Docker Engine e Docker Compose v2.
- Python 3.11+ somente para scripts de release.
- PNPM e Node.js serão necessários quando a aplicação Angular for implementada.

## Executar a CLI

```bash
cargo run -p database-backup-cli -- --help
cargo run -p database-backup-cli -- info
```

Testar a conexão com um PostgreSQL (a senha nunca é passada por argumento):

```bash
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- test-connection \
  --host localhost --port 5432 --database my_database --user postgres
```

Sem `DBU_PG_PASSWORD`, a CLI pede a senha em um prompt sem eco. Hosts fora de `localhost`
exigem TLS com certificado validado (`--tls verify-full`, padrão em `--tls auto`).

Gerar um backup completo (`pg_dump` em formato custom, já comprimido):

```bash
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- backup \
  --database my_database --user postgres [--output-dir DIR] [--runner auto|native|docker]
```

- O dump é gravado em streaming num arquivo `.partial`, conferido com `pg_restore --list` e só
  então renomeado para `<database>-<AAAAMMDDTHHMMSSZ>.dump`. Arquivo parcial nunca vira backup.
- Ao lado do dump fica `<arquivo>.dump.meta.json`, com SHA-256, tamanho, versões e runner.
- Cada execução (sucesso, falha ou cancelamento) é registrada no histórico SQLite.
- Destino padrão: `backups/` na pasta de dados do aplicativo (`~/.local/share/database-backup-utility/backups/` no Linux).
- `--runner auto` usa o `pg_dump` instalado se for compatível; senão, o Docker.

Restaurar um backup:

```bash
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- restore \
  --file ~/.local/share/database-backup-utility/backups/my_database-20261020T190509Z.dump \
  --target-database my_database_copy --user postgres
```

- Antes de restaurar, confere o SHA-256 com o `.meta.json` (ou com o histórico), valida o arquivo
  com `pg_restore --list` e recusa backups vindos de um PostgreSQL mais novo que o destino.
  Arquivos sem checksum conhecido exigem `--allow-unverified`, que emite um aviso e marca o restore
  como não verificado no histórico; estrutura e versão continuam sendo conferidas.
- Recusa como origem links simbólicos, caminhos que não são arquivos regulares e (no Linux/macOS)
  arquivos ou `.meta.json` graváveis por grupo ou outros usuários.
- Por padrão só **cria um banco novo**. Se o destino já tem dados, é preciso `--overwrite` e
  confirmar digitando o nome do banco (ou `--confirm <nome>` em scripts). O restore vai para um
  banco temporário; só se der certo o atual vira `<destino>__previous_<data>` (mantido) e o novo
  assume o nome. Se falhar, o destino não é tocado.
- O `pg_restore` roda em uma única transação: uma falha no meio desfaz tudo.

| Exit code | Significado |
| --- | --- |
| 0 | Sucesso |
| 1 | Erro não classificado (ex.: histórico ou pasta de destino inacessível) |
| 2 | Parâmetro inválido ou senha ausente |
| 3 | Autenticação recusada |
| 4 | Banco inexistente |
| 5 | Servidor inacessível ou tempo esgotado |
| 6 | Falha de TLS |
| 7 | `pg_dump`/`pg_restore` ausente ou incompatível (e Docker indisponível) |
| 8 | O backup falhou |
| 9 | Backup ou restore cancelado (Ctrl+C) |
| 10 | Arquivo de backup ausente, corrompido, sem checksum conhecido ou incompatível |
| 11 | O restore falhou (o destino não foi alterado) |
| 12 | Destino com dados sem `--overwrite`, ou confirmação ausente/divergente |

## PostgreSQL para desenvolvimento

```bash
cp .env.example .env
# Altere a senha em .env antes de iniciar.
docker compose --env-file .env -f infra/docker/compose.yml up -d postgres
docker compose --env-file .env -f infra/docker/compose.yml ps
docker compose --env-file .env -f infra/docker/compose.yml down
```

- Por padrão somente o PostgreSQL sobe; MySQL e MongoDB possuem perfis opcionais.
- **Os testes de integração não usam esse volume:** criam containers PostgreSQL efêmeros.
- Para iniciar MySQL/MongoDB, consulte `docs/configuracao.md`.
- Não use `down -v` sem intenção explícita de apagar todos os dados dos volumes locais.

## Verificações

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib --bins
docker info
cargo test --workspace --tests
cargo build --workspace
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```

A etapa de Testcontainers requer Docker funcional e pode baixar imagens na primeira execução.
O `Cargo.lock` é versionado; **a release manual exige o lockfile no repositório**.

## Documentação

| Página | Conteúdo |
| --- | --- |
| [Comandos](docs/comandos.md) | Resumo para rodar, testar e validar antes do PR |
| [Projeto](PROJECT.md) | Decisões, requisitos e roadmap completo |
| [Guia dos agentes](AGENTS.md) | Padrões obrigatórios de trabalho em pt-BR |
| [Stack](docs/stack.md) | Ferramentas escolhidas e estado de implementação |
| [Arquitetura](docs/arquitetura.md) | Limites entre CLI, core, adaptadores e desktop |
| [Configuração](docs/configuracao.md) | Variáveis, Docker e segurança |
| [Testes](docs/testes.md) | Testes unitários, integração e comandos |
| [CI e PRs](docs/CI.md) | Pipelines, proteção da main e contribuições |
| [Processo de releases](docs/RELEASE_PROCESS.md) | Publicação manual com Run workflow |
| [Releases planejadas](docs/RELEASES.md) | Milestones futuras sem confundir planos e entregas |
| [Modelo de dados](docs/diagramas/modelo-de-dados.md) | Proposta de SQLite interno |
| [Linear](docs/linear/README.md) | Links para issues e milestones |
| [Changelog](CHANGELOG.md) | Somente alterações concluídas |

## Referências

- [Desafio original](https://roadmap.sh/projects/database-backup-utility)
- [Rust Workspaces](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html)
- [Angular](https://angular.dev/)
- [Tauri](https://tauri.app/)
- [Tailwind CSS](https://tailwindcss.com/)
- [Testcontainers for Rust](https://rust.testcontainers.org/)
- [Keep a Changelog](https://keepachangelog.com/)

## Licença

Licença ainda não definida. Antes da primeira distribuição pública, escolher a licença,
adicionar `LICENSE` e declarar `license` nos manifests Cargo.
