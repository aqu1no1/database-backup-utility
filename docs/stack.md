# Stack tecnológica

| Área | Tecnologia | Estado |
| --- | --- | --- |
| Linguagem principal | Rust estável / Cargo Workspace | Implementado (v0.1.0) |
| CLI | Clap, Tokio, rpassword, tracing, serde_json | `connection test`, `backup run/list/verify`, `restore run/list`, `--json` |
| Conexão PostgreSQL | tokio-postgres, rustls (ring) + certificados do sistema | Implementado (TLS `verify-full` para hosts remotos) |
| Motor PostgreSQL | Ferramentas oficiais `pg_dump`, `pg_restore` (runner native ou Docker) | Backup, verificação e restore implementados |
| Histórico | SQLite (rusqlite, bundled) + rusqlite_migration | Implementado: conexões, backups, artefatos e restores (3 migrations) |
| Desenvolvimento | Docker Compose com PostgreSQL 17 | Arquivo criado |
| Integração / E2E | Testcontainers para Rust (PostgreSQL efêmero) | Integração dos adaptadores e E2E da CLI com runners Docker e nativo |
| Desktop | Tauri | Planejado, v0.3.0 |
| Frontend | Angular standalone, TypeScript estrito | Escolhido, ainda não criado |
| Estilos | Tailwind CSS v4 | Escolhido, ainda não configurado |
| i18n | Transloco, `pt-BR` / `en-US` | Escolhido, ainda não configurado |
| Frontend testing | Vitest + Angular TestBed | Planejado |
| Storage local | Arquivo `.partial` + SHA-256 (sha2) + rename atômico, manifesto JSON versionado (serde) | Implementado |
| Cloud storage | S3 compatível, outros provedores | Futuro |
| CI / Releases | GitHub Actions (actions fixadas por SHA) | CI em PR/`main`; release manual por `workflow_dispatch` |
| Licença | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` |

As versões específicas devem ser atualizadas junto com manifests e lockfiles.
