# Roadmap de versões (planejamento)

> Este documento não substitui o `CHANGELOG.md`. Nada aqui implica publicação.

| Versão | Objetivo | Escopo planejado |
| --- | --- | --- |
| `v0.1.0` | PostgreSQL CLI — **implementada, publicação pelo DB-14** | Conexões, backup full local com streaming, verificação, restore testado, histórico SQLite, manifesto versionado, testes E2E e CI |
| `v0.1.1` | Correção da v0.1.0 | Mesmo código da v0.1.0 com Tokio 1.54.1 (RUSTSEC-2026-0384); tarball com `THIRD-PARTY-LICENSES.md` (avisos de licença das dependências) |
| `v0.2.0` | Multi-DBMS | MySQL, MongoDB, SQLite, testes por SGBD |
| `v0.3.0` | Desktop Angular | Tauri, Angular standalone, Tailwind CSS v4, Transloco `pt-BR`/`en-US` |
| `v0.4.0` | Agendamento e cloud | Scheduler independente do desktop, S3 compatível, retenção, Slack |
| `v1.0.0` | Distribuição estável (sem milestone no Linear ainda) | Compatibilidade, verificações, instaladores, documentação e suporte |
| `v2.0.0` | Capacidades avançadas | Incremental/diferencial/PITR **onde houver suporte real** |

Projetos/milestones no Linear: https://linear.app/db-backup-utility/project/database-backup-utility-575328110532
