# Manifesto do backup

Cada backup gerado por `backup run` tem, ao lado do dump, um manifesto JSON versionado:
`<arquivo>.dump.meta.json`. Com ele dá para identificar e verificar o backup sem o histórico
SQLite interno, por exemplo depois de copiar o par de arquivos para outra máquina.

## Quando e como é gravado

- Só depois que o dump terminou, o `pg_dump` saiu com sucesso, o arquivo passou no
  `pg_restore --list` e foi renomeado do `.partial` para o nome final.
- Escrita atômica: arquivo `.partial`, `fsync` e `rename`, com permissão `0600`.
- Se o manifesto não puder ser gravado, o dump também é removido e o backup falha.

## Campos (versão 2)

PostgreSQL:

```json
{
  "manifest_version": 2,
  "tool": { "name": "database-backup", "version": "…" },
  "engine": "postgres",
  "format": "pg_dump-custom",
  "file_name": "app-20261021T120000Z.dump",
  "sha256": "…64 caracteres hex…",
  "size_bytes": 5302,
  "database": "app",
  "host": "localhost",
  "port": 5432,
  "server_version": "17.2 (Debian 17.2-1.pgdg120+1)",
  "pg_dump_version": "17.11",
  "runner": "docker",
  "created_at": "2026-10-21T12:00:00Z",
  "execution_id": "01a15ec9-…"
}
```

MySQL:

```json
{
  "manifest_version": 2,
  "tool": { "name": "database-backup", "version": "…" },
  "engine": "mysql",
  "format": "mysqldump-sql",
  "file_name": "app-20261023T120000Z.sql",
  "sha256": "…64 caracteres hex…",
  "size_bytes": 8120,
  "database": "app",
  "host": "localhost",
  "port": 3306,
  "server_version": "8.4.13",
  "mysqldump_version": "8.4",
  "non_transactional_tables": [],
  "runner": "docker",
  "created_at": "2026-10-23T12:00:00Z",
  "execution_id": "01a17…"
}
```

SQLite (sem campos de servidor):

```json
{
  "manifest_version": 2,
  "tool": { "name": "database-backup", "version": "…" },
  "engine": "sqlite",
  "format": "sqlite-backup-api",
  "file_name": "app-20261022T120000Z.sqlite3",
  "sha256": "…64 caracteres hex…",
  "size_bytes": 36864,
  "database": "app.db",
  "server_version": "3.50.4",
  "created_at": "2026-10-22T12:00:00Z",
  "execution_id": "01a16…"
}
```

| Campo | Significado |
| --- | --- |
| `manifest_version` | Versão do formato do manifesto. |
| `tool` | Nome e versão do utilitário que gerou o backup. |
| `engine` | SGBD de origem: `postgres`, `mysql` ou `sqlite` (os mesmos nomes do histórico). Na v1 era `postgresql`. |
| `format` | Formato do artefato: `pg_dump-custom`, `mysqldump-sql` ou `sqlite-backup-api`. |
| `file_name` | Nome do dump, sem diretório. Mover o par junto continua válido. |
| `sha256`, `size_bytes` | Checksum e tamanho do dump. |
| `database`, `host`, `port` | Origem do backup. Nunca usuário, senha, `secret_ref` ou URI de conexão. No SQLite, `database` é o nome do arquivo de origem e `host`/`port` não existem. |
| `server_version` | Versão do servidor PostgreSQL ou MySQL, ou da biblioteca SQLite usada na cópia. |
| `non_transactional_tables` | Só MySQL: tabelas fora do InnoDB, sem snapshot consistente no backup. Lista vazia quando todas são InnoDB; ausente se a checagem não pôde ser feita. |
| `pg_dump_version`, `mysqldump_version`, `runner` | Só PostgreSQL ou MySQL: versão do `pg_dump`/`mysqldump` (major.minor no MySQL) e se ele foi nativo ou via Docker. |
| `created_at`, `execution_id` | Momento do backup e ID da execução no histórico de origem. |

## Versões

- `manifest_version` só muda quando o formato muda de forma incompatível (campo removido,
  renomeado ou com outro significado). Campos novos opcionais não mudam a versão.
- Leitores recusam manifesto sem `manifest_version` ou com versão desconhecida, com código 10 e a
  versão encontrada na mensagem. Assim, um utilitário antigo nunca interpreta errado um formato novo.
- **v2 (DB-22):** `host`, `port`, `pg_dump_version` e `runner` ficaram opcionais (só PostgreSQL) e
  `engine` passou de `postgresql` para `postgres`. Como campo deixou de ser obrigatório, a versão mudou.
  Esta versão lê v1 e v2, então backups da v0.1.x continuam verificáveis e restauráveis. A v0.1.x
  recusa manifestos v2 com código 10.
- **v2 + `mysqldump_version` e `non_transactional_tables` (DB-20):** campos novos e opcionais, só em
  backups MySQL. A versão continua 2.
  Uma versão anterior a esta lê o manifesto, mas recusa o `engine` `mysql` (código 10).

## O que `backup verify` e `restore run` conferem

1. O dump e o manifesto não são links simbólicos nem graváveis por grupo ou outros.
2. `manifest_version` suportada.
3. `file_name` igual ao nome do dump verificado: um manifesto copiado ou renomeado para outro dump é recusado.
4. `engine` igual ao SGBD do comando (`postgres`, aceitando o legado `postgresql`, `mysql` ou `sqlite`).
5. `size_bytes` igual ao tamanho real (detecta arquivo truncado antes do hash).
6. SHA-256 do dump igual ao `sha256` do manifesto.
7. Estrutura válida com `pg_restore --list` (PostgreSQL), leitura do dump SQL (MySQL: cabeçalho do
   `mysqldump`, linha final `-- Dump completed`, sem `USE`/`CREATE DATABASE` e sem trigger ou evento
   que o MySQL não conseguiria restaurar) ou
   `PRAGMA integrity_check` (SQLite).

Sem manifesto, a referência do checksum vem do histórico SQLite. Sem nenhum dos dois, só com
`--allow-unverified`, e o restore fica registrado como não verificado.

## Limites

O SHA-256 detecta corrupção e alterações acidentais ou parciais, mas **não autentica** o backup.
Quem consegue alterar o dump e o manifesto ao mesmo tempo produz um par consistente, que passa na
verificação. Proteger contra isso exige guardar o checksum em outro lugar confiável ou assinar ou
criptografar o artefato. A criptografia com `age` está planejada no DB-31 (v0.4.0). Até lá, proteja o
diretório de backups com as permissões do sistema e, para cópias externas, guarde o `sha256` em
um local separado.
