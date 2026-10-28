# Stack tecnológica

| Área | Tecnologia | Estado |
| --- | --- | --- |
| Linguagem principal | Rust estável / Cargo Workspace | Implementado (v0.1.0) |
| CLI | Clap, Tokio, rpassword, tracing, serde_json | `connection test`, `backup run/list/verify`, `restore run/list`, `engines`, `--json` |
| Conexão PostgreSQL | tokio-postgres, rustls (ring) + certificados do sistema | Implementado (TLS `verify-full` para hosts remotos) |
| Motor PostgreSQL | Ferramentas oficiais `pg_dump`, `pg_restore` (runner native ou Docker) | Backup, verificação e restore implementados |
| Conexão MySQL | mysql_async (sem features padrão; rustls com ring, certificados do sistema, raízes embutidas desligadas) | Implementado (DB-20) |
| Motor MySQL | Ferramentas oficiais `mysqldump`, `mysql` (runner native ou Docker `mysql:<major.minor>`) | Backup, verificação e restore em banco novo implementados (DB-20) |
| Conexão MongoDB | `mongodb` 3.9 (driver oficial, `default-features = false`: `rustls-tls` com ring, `bson-3` e `compat-3-3-0`; sem `dns-resolver`, `openssl-tls` nem `rustls-tls-aws-lc`) e certificados do sistema em `ca_file_path` | Implementado (DB-21) |
| Motor MongoDB | MongoDB Database Tools `mongodump`, `mongorestore` (runner native ou Docker `mongo:<major.minor>`), formato `--archive` sem compressão | Backup, verificação e restore em banco novo implementados (DB-21) |
| Motor SQLite | Online Backup API do SQLite via `rusqlite` (feature `backup`, SQLite embutido) | Backup, verificação e restore implementados (DB-22) |
| Histórico | SQLite (rusqlite, bundled) + rusqlite_migration | Implementado: conexões, backups, artefatos e restores (4 migrations) |
| Desenvolvimento | Docker Compose com PostgreSQL 17 | Arquivo criado |
| Integração / E2E | Testcontainers para Rust (PostgreSQL 17, MySQL 8.4 e MongoDB 7.0 efêmeros) | Integração dos adaptadores e E2E da CLI com runners Docker e nativo |
| Desktop | Tauri | Planejado, v0.3.0 |
| Frontend | Angular standalone, TypeScript estrito | Escolhido, ainda não criado |
| Estilos | Tailwind CSS v4 | Escolhido, ainda não configurado |
| i18n | Transloco, `pt-BR` / `en-US` | Escolhido, ainda não configurado |
| Frontend testing | Vitest + Angular TestBed | Planejado |
| Storage local | Arquivo `.partial` + SHA-256 (sha2) + rename atômico, manifesto JSON versionado (serde) | Implementado |
| Cloud storage | S3 compatível, outros provedores | Futuro |
| CI / Releases | GitHub Actions (actions fixadas por SHA) | CI em PR/`main`; release manual por `workflow_dispatch` |
| Licença | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` |

As versões específicas devem ser atualizadas junto com manifests e lockfiles. O que cada SGBD suporta
(operações, formatos, limites de consistência) está em [`capacidades.md`](capacidades.md).

## MongoDB Database Tools

A versão mínima das ferramentas depende da versão do servidor. A tabela vem do changelog oficial do
Database Tools (<https://www.mongodb.com/docs/database-tools/release-notes/database-tools-changelog/>),
que lista a primeira versão das ferramentas para cada servidor e diz que elas são compatíveis com o
servidor listado e os anteriores. Para o 4.4 vale o piso `100.3.0`, a primeira versão com `--config`
(o arquivo YAML da senha); a oficial para o 4.4 seria a `100.1.0`.

| Servidor | Database Tools mínimo |
| --- | --- |
| 4.4 | 100.3.0 |
| 5.0 | 100.4.0 |
| 6.0 | 100.5.0 |
| 7.0 | 100.8.0 |
| 8.0 | 100.10.0 |
| 8.1 | 100.12.0 |
| 8.2 | 100.13.0 |
| 8.3 | 100.16.0 |
| 9.0 | 100.18.0 |

- Servidor abaixo do 4.4 é recusado (código 1). Servidor mais novo que o 9.0 usa a exigência do 9.0 e
  gera um aviso no log.
- A saída de `mongodump --version` é `mongodump version: 100.x.y`. As ferramentas legadas imprimem
  `r4.x.y` (por exemplo, a imagem `mongo:4.2`) e são recusadas como incompatíveis.
- As imagens `mongo:7.0` e `mongo:8.0` trazem a `100.19.1` e a `mongo:4.4` traz a `100.10.0`; por isso
  o runner Docker satisfaz a tabela para qualquer servidor.
- O driver `mongodb` 3.9 (usado no `connection test`, na checagem do destino e na conferência do
  restore) recusa servidores abaixo do 4.4 (`DRIVER_MIN_DB_VERSION` na fonte), o mesmo mínimo da CLI.
