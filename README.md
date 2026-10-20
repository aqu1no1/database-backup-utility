# Database Backup Utility

Ferramenta multiplataforma de backup e restauração de bancos de dados. O produto terá uma
**CLI independente em Rust** e, futuramente, um **desktop Tauri + Angular + Tailwind v4**,
ambos utilizando as mesmas crates de negócio.

> **Estado atual:** bootstrap do monorepo, testes de infraestrutura PostgreSQL,
> Docker Compose, documentação e pipelines. **Ainda não executa backups nem restaurações.**
> As entregas funcionais estão organizadas nas issues do Linear.

## Stack

- CLI: Rust + Clap; bibliotecas compartilhadas no Cargo Workspace.
- Desktop (planejado): Tauri + Angular standalone + TypeScript + Tailwind v4.
- i18n desktop: Transloco, `pt-BR` e `en-US`.
- Desenvolvimento: PostgreSQL no Docker Compose; MySQL/MongoDB opcionais; SQLite embutido futuramente.
- Testes: Rust `cargo test`, integração com Testcontainers, Vitest futuramente no Angular.
- GitHub Actions: CI em PRs/main, auditoria e releases **somente manuais** (`workflow_dispatch`).

## Requisitos locais

- Rust estável (`rustup`, Cargo), Docker Engine e Docker Compose v2.
- Python 3.11+ somente para scripts de release.
- PNPM e Node.js serão necessários quando a aplicação Angular for implementada.

## Executar a CLI inicial

```bash
cargo run -p database-backup-cli -- --help
cargo run -p database-backup-cli -- info
```

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
