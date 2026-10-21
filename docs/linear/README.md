# Linear — planejamento do projeto

**Projeto:** https://linear.app/db-backup-utility/project/database-backup-utility-575328110532

## v0.1.0 — PostgreSQL CLI

| ID | Issue | Situação |
| --- | --- | --- |
| DB-5 | Bootstrap, pastas, templates e pipelines | Done |
| DB-6 | Testcontainers PostgreSQL | Done |
| DB-7 | Validação de conexão PostgreSQL | Done |
| DB-8 | Histórico SQLite / migrations | Done |
| DB-9 | Executores pg_dump/pg_restore native/Docker | Done |
| DB-10 | Backup full PostgreSQL com streaming | Done |
| DB-11 | Restauração PostgreSQL | Done |
| DB-12 | Comandos, mensagens e logs CLI | Done |
| DB-13 | Testes E2E backup/restore | Done |
| DB-17 | Segurança básica do MVP | Done |
| DB-18 | Provedor de credenciais | Done |
| DB-19 | Manifesto versionado e verificação | Done |
| DB-14 | Documentação e **release manual v0.1.0** | Card de release |

## Próximas versões (planejadas)

| Versão | Cards | Card de release |
| --- | --- | --- |
| v0.2.0 — Multi-Database | DB-15 (épico), DB-20 MySQL, DB-21 MongoDB, DB-22 SQLite, DB-23 matriz de capacidades | DB-32 |
| v0.3.0 — Angular + Tauri | DB-16 (épico), DB-24 bridge, DB-25 i18n, DB-26 design system, DB-27 telas | DB-33 |
| v0.4.0 — Automação e armazenamento | DB-28 agendamento, DB-29 fila de jobs, DB-30 S3, DB-31 compressão e criptografia | DB-34 |

A release de cada versão só é publicada pelo card de release, depois que todos os cards do milestone
estiverem em Done (ver `docs/RELEASE_PROCESS.md`). v1.0.0 ainda não tem milestone.

O backlog aprovado no Linear é o registro vivo das tarefas. Este arquivo é um índice para
navegação no repositório. Use os links do Linear para estados e prioridades atuais.
