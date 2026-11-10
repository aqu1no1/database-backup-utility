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

MongoDB:

```json
{
  "manifest_version": 2,
  "tool": { "name": "database-backup", "version": "…" },
  "engine": "mongodb",
  "format": "mongodump-archive",
  "file_name": "app-20261024T120000Z.archive",
  "sha256": "…64 caracteres hex…",
  "size_bytes": 3450,
  "database": "app",
  "host": "localhost",
  "port": 27017,
  "server_version": "8.0.15",
  "mongodump_version": "100.10.0",
  "topology": "standalone",
  "runner": "docker",
  "created_at": "2026-10-24T12:00:00Z",
  "execution_id": "01a18…"
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
| `engine` | SGBD de origem: `postgres`, `mysql`, `mongodb` ou `sqlite` (os mesmos nomes do histórico). Na v1 era `postgresql`. |
| `format` | Formato do artefato: `pg_dump-custom`, `mysqldump-sql`, `mongodump-archive` ou `sqlite-backup-api`. |
| `file_name` | Nome do dump, sem diretório. Mover o par junto continua válido. |
| `sha256`, `size_bytes` | Checksum e tamanho do dump. |
| `database`, `host`, `port` | Origem do backup. Nunca usuário, senha, `secret_ref` ou URI de conexão. No SQLite, `database` é o nome do arquivo de origem e `host`/`port` não existem. |
| `server_version` | Versão do servidor PostgreSQL, MySQL ou MongoDB, ou da biblioteca SQLite usada na cópia. |
| `non_transactional_tables` | Só MySQL: tabelas fora do InnoDB, sem snapshot consistente no backup. Lista vazia quando todas são InnoDB; ausente se a checagem não pôde ser feita. |
| `pg_dump_version`, `mysqldump_version`, `mongodump_version`, `runner` | Só PostgreSQL, MySQL ou MongoDB: versão do `pg_dump`/`mysqldump`/`mongodump` (major.minor no MySQL; três números no MongoDB, como `100.10.0`) e se ele foi nativo ou via Docker. |
| `topology` | Só MongoDB: `standalone`, `replica_set` ou `sharded` (a CLI só aceita as duas primeiras). O backup de um banco MongoDB não é um snapshot de um único instante em nenhuma delas; ver [`arquitetura.md`](arquitetura.md#mongodb-topologia-e-consistência). |
| `created_at`, `execution_id` | Momento do backup e ID da execução no histórico de origem. |

## Versão 3: backup comprimido ou criptografado

Um backup com `--compress` (quando o formato não é comprimido) ou `--recipient` grava o manifesto na
versão 3. Os campos da versão 2 continuam com o mesmo significado, com uma diferença: `file_name`,
`sha256` e `size_bytes` descrevem o **arquivo armazenado** (comprimido e/ou criptografado), e o dump
aberto fica descrito em `content`. Assim, o checksum é conferido antes de decriptar.

```json
{
  "manifest_version": 3,
  "tool": { "name": "database-backup", "version": "…" },
  "engine": "mysql",
  "format": "mysqldump-sql",
  "file_name": "app-20261110T120000Z.sql.zst.age",
  "sha256": "…SHA-256 do arquivo .sql.zst.age…",
  "size_bytes": 2310,
  "database": "app",
  "…": "demais campos do SGBD, como na versão 2",
  "compression": "zstd",
  "encryption": "age",
  "content": {
    "file_name": "app-20261110T120000Z.sql",
    "sha256": "…SHA-256 do dump aberto…",
    "size_bytes": 8120
  }
}
```

| Campo | Significado |
| --- | --- |
| `compression` | `zstd`, ou ausente. O `format` continua sendo o do SGBD (o que fica depois de descomprimir). |
| `encryption` | `age`, ou ausente. O manifesto nunca traz chave privada nem os destinatários. |
| `content.file_name` | Nome do dump aberto. Precisa ser um nome simples, sem diretório; senão o manifesto é recusado. |
| `content.sha256`, `content.size_bytes` | Checksum e tamanho do dump aberto. A abertura para ao passar desse tamanho. |

Um backup sem compressão e sem criptografia continua gravando a versão 2, e uma versão anterior do
utilitário continua lendo esses backups. Ver [`criptografia.md`](criptografia.md).

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
- **v2 + `mongodump_version` e `topology` (DB-21):** campos novos e opcionais, só em backups MongoDB.
  A versão continua 2. Uma versão anterior a esta lê o manifesto, mas recusa o `engine` `mongodb`
  (código 10).

- **v3 (DB-31):** `compression`, `encryption` e `content`; `file_name`/`sha256`/`size_bytes` passam a
  descrever o arquivo armazenado. Como um leitor antigo trataria o arquivo criptografado como dump, a
  versão muda e a v0.2.x recusa o manifesto com código 10. Só backups comprimidos ou criptografados
  usam a v3; os outros continuam na v2. Um manifesto v1/v2 com esses campos, ou um v3 sem `content`, é
  recusado.

## O que `backup verify` e `restore run` conferem

1. O dump e o manifesto não são links simbólicos nem graváveis por grupo ou outros.
2. `manifest_version` suportada.
3. `file_name` igual ao nome do dump verificado: um manifesto copiado ou renomeado para outro dump é recusado.
4. `engine` igual ao SGBD do comando (`postgres`, aceitando o legado `postgresql`, `mysql`, `mongodb` ou `sqlite`).
5. `size_bytes` igual ao tamanho real (detecta arquivo truncado antes do hash).
6. SHA-256 do dump igual ao `sha256` do manifesto.
7. Estrutura válida com `pg_restore --list` (PostgreSQL), leitura do dump SQL (MySQL: cabeçalho do
   `mysqldump`, linha final `-- Dump completed`, sem `USE`/`CREATE DATABASE` e sem trigger ou evento
   que o MySQL não conseguiria restaurar), leitura do archive (MongoDB: número mágico, cabeçalho,
   prelúdio, `EOF` de toda coleção e view, um único banco e sem coleções time series; o CRC é
   conferido pelo `mongorestore`, não pelo `verify`) ou `PRAGMA integrity_check` (SQLite).

Em um backup v3, os passos 1 a 6 valem para o arquivo armazenado, antes de abri-lo. Depois o
conteúdo é aberto num diretório temporário privado, conferido com `content.sha256` e passa pelo passo
7 (ver [`criptografia.md`](criptografia.md#o-que-o-verify-e-o-restore-conferem)).

Sem manifesto, a referência do checksum vem do histórico SQLite. Sem nenhum dos dois, só com
`--allow-unverified`, e o restore fica registrado como não verificado.

## Limites

O SHA-256 detecta corrupção e alterações acidentais ou parciais, mas **não autentica** o backup.
Quem consegue alterar o dump e o manifesto ao mesmo tempo produz um par consistente, que passa na
verificação. Proteger contra isso exige guardar o checksum em outro lugar confiável ou assinar ou
criptografar o artefato. Com `--recipient` (DB-31), o backup é criptografado com `age`: sem a chave
privada ninguém lê nem altera o conteúdo sem que a abertura falhe. O age não assina, então quem tem a
chave pública ainda consegue gerar outro backup válido; para cópias externas, guarde o `sha256` em um
local separado. Ver [`criptografia.md`](criptografia.md).
