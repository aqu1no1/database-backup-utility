# Stack tecnológica

| Área | Tecnologia | Estado |
| --- | --- | --- |
| Linguagem principal | Rust estável / Cargo Workspace | Base criada |
| CLI | Clap, Tokio, rpassword | `info` e `test-connection` |
| Conexão PostgreSQL | tokio-postgres, rustls (ring) + certificados do sistema | Teste de conexão implementado |
| Motor PostgreSQL | Ferramentas oficiais `pg_dump`, `pg_restore` (runner native ou Docker) | Executor implementado |
| Histórico | SQLite (rusqlite, bundled) + rusqlite_migration | Crate `history` com schema inicial |
| Desenvolvimento | Docker Compose com PostgreSQL 17 | Arquivo criado |
| Integração | Testcontainers para Rust (PostgreSQL efêmero) | Harness, fixtures e testes de conexão |
| Desktop | Tauri | Planejado, v0.3.0 |
| Frontend | Angular standalone, TypeScript estrito | Escolhido, ainda não criado |
| Estilos | Tailwind CSS v4 | Escolhido, ainda não configurado |
| i18n | Transloco, `pt-BR` / `en-US` | Escolhido, ainda não configurado |
| Frontend testing | Vitest + Angular TestBed | Planejado |
| Cloud storage | S3 compatível, outros provedores | Futuro |
| CI / Releases | GitHub Actions | CI validada em PR; release manual |

As versões específicas devem ser atualizadas junto com manifests e lockfiles.
