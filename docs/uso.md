# Uso da CLI

Manual do `dbu`: requisitos de cada SGBD, opções gerais, backup, verificação e restore
de PostgreSQL, MySQL, MongoDB e SQLite, e os exit codes. O que cada banco suporta está em
[`capacidades.md`](capacidades.md); variáveis e credenciais, em [`configuracao.md`](configuracao.md).

Os exemplos usam `cargo run -p database-backup-cli --`. Com o binário da release, troque por
`dbu`.

## Comandos curtos

| Curto | Longo equivalente |
| --- | --- |
| `dbu backup …` | `dbu backup run …` |
| `dbu verify <ARQUIVO>` | `dbu backup verify --file <ARQUIVO>` |
| `dbu restore <ARQUIVO> --to <DESTINO>` | `dbu restore run --file <ARQUIVO> --target-database <DESTINO>` (ou `--target`, no SQLite) |
| `dbu list` / `dbu list restores` | `dbu backup list` / `dbu restore list` |
| `dbu test …` | `dbu connection test …` |

Flags curtas: `-e/--engine`, `-H/--host`, `-p/--port`, `-d/--database`, `-u/--user`, `-f/--file`,
`-o/--output-dir`. Os comandos e flags longos continuam valendo, e `database-backup` (o nome
anterior do binário) vem no tarball como link para `dbu`. Os exemplos abaixo usam a forma longa,
que é a mais explícita para scripts.

## Requisitos por SGBD

Cada SGBD com servidor precisa das ferramentas oficiais instaladas (`--runner native`) **ou** do
Docker (`--runner docker`). O padrão `--runner auto` usa a ferramenta instalada se for compatível;
senão, o Docker. Quem pode usar o Docker tem, na prática, acesso de root; veja
[`seguranca.md`](seguranca.md).

- **PostgreSQL:** `pg_dump` e `pg_restore` com versão **maior ou igual** à do servidor, ou Docker
  (imagem `postgres:<versão do servidor>`).
- **MySQL:** `mysqldump` e `mysql` com versão maior ou igual à do servidor (major.minor), ou Docker
  (imagem `mysql:<major.minor>`). MariaDB ainda não é suportado (DB-37).
- **MongoDB:** MongoDB Database Tools `100.x` (`mongodump`/`mongorestore`) compatíveis com o
  servidor ([tabela em `stack.md`](stack.md#mongodb-database-tools)), ou Docker (imagem
  `mongo:<major.minor>`). Servidor 4.4 ou mais novo, standalone ou primário de replica set.
- **SQLite:** nada externo; a cópia usa o SQLite embutido no binário.

## Roteiros por SGBD

Cada roteiro cria um banco de exemplo, faz o backup, confere o arquivo e restaura em um destino
novo, num ambiente descartável: o Compose de desenvolvimento (`infra/docker/compose.yml`) para os
SGBDs com servidor e um arquivo local para o SQLite. As ferramentas de cada SGBD não precisam
estar instaladas, porque o `--runner auto` usa o Docker quando elas faltam.

Preparação, na raiz do repositório:

```bash
cp .env.example .env                    # troque as senhas
set -a; . ./.env; set +a                # só para os roteiros: a CLI não lê o .env
mkdir -p -m 700 ~/.config/dbu backups
alias dc='docker compose --env-file .env -f infra/docker/compose.yml'
```

As senhas chegam à CLI por arquivos `0600`, nunca pela linha de comando. O restore sempre cria um
banco (ou arquivo) **novo**. Substituir um destino com dados exige `--overwrite` e a confirmação do
nome, onde o SGBD suporta; detalhes nas seções seguintes.

### PostgreSQL

```bash
dc up -d --wait postgres
install -m 600 /dev/null ~/.config/dbu/pg.pass && printf '%s\n' "$POSTGRES_PASSWORD" > ~/.config/dbu/pg.pass
export DBU_PG_PASSWORD_FILE=~/.config/dbu/pg.pass
dc exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
  -c "CREATE TABLE pedidos (id int PRIMARY KEY, total numeric); INSERT INTO pedidos VALUES (1, 10.5), (2, 20);"

dbu test -d "$POSTGRES_DB" -u "$POSTGRES_USER"
dbu backup -d "$POSTGRES_DB" -u "$POSTGRES_USER" -o ./backups
BACKUP=$(ls -t backups/*.dump | head -1)
dbu verify "$BACKUP"
dbu restore "$BACKUP" --to pedidos_copia -u "$POSTGRES_USER"
dc exec postgres psql -U "$POSTGRES_USER" -d pedidos_copia -c "SELECT count(*) FROM pedidos"   # 2
```

### MySQL

O backup usa o usuário de aplicação do `.env`. O restore cria um banco, então usa o `root`.

```bash
dc --profile mysql up -d mysql
install -m 600 /dev/null ~/.config/dbu/mysql.pass && printf '%s\n' "$MYSQL_PASSWORD" > ~/.config/dbu/mysql.pass
install -m 600 /dev/null ~/.config/dbu/mysql-root.pass && printf '%s\n' "$MYSQL_ROOT_PASSWORD" > ~/.config/dbu/mysql-root.pass
export DBU_MYSQL_PASSWORD_FILE=~/.config/dbu/mysql.pass
until dbu test -e mysql -d "$MYSQL_DATABASE" -u "$MYSQL_USER" 2>/dev/null; do sleep 2; done   # primeira subida: ~20 s
dc exec mysql sh -c 'MYSQL_PWD="$MYSQL_PASSWORD" mysql -u "$MYSQL_USER" "$MYSQL_DATABASE" \
  -e "CREATE TABLE pedidos (id int PRIMARY KEY, total decimal(10,2)); INSERT INTO pedidos VALUES (1, 10.5), (2, 20);"'

dbu backup -e mysql -d "$MYSQL_DATABASE" -u "$MYSQL_USER" -o ./backups
BACKUP=$(ls -t backups/*.sql | head -1)
dbu verify "$BACKUP"
DBU_MYSQL_PASSWORD_FILE=~/.config/dbu/mysql-root.pass dbu restore -e mysql "$BACKUP" --to pedidos_copia -u root
dc exec mysql sh -c 'MYSQL_PWD="$MYSQL_ROOT_PASSWORD" mysql -u root -N -e "SELECT count(*) FROM pedidos_copia.pedidos"'   # 2
```

### MongoDB

```bash
dc --profile mongodb up -d mongodb
install -m 600 /dev/null ~/.config/dbu/mongo.pass && printf '%s\n' "$MONGO_PASSWORD" > ~/.config/dbu/mongo.pass
export DBU_MONGO_PASSWORD_FILE=~/.config/dbu/mongo.pass
until dc exec mongodb mongosh --quiet --eval 'db.runCommand({ping: 1})' >/dev/null 2>&1; do sleep 2; done
dc exec mongodb sh -c 'mongosh --quiet -u "$MONGO_INITDB_ROOT_USERNAME" -p "$MONGO_INITDB_ROOT_PASSWORD" pedidos \
  --authenticationDatabase admin --eval "db.pedidos.insertMany([{_id: 1, total: 10.5}, {_id: 2, total: 20}])"'

dbu test -e mongodb -d pedidos -u "$MONGO_USER"
dbu backup -e mongodb -d pedidos -u "$MONGO_USER" -o ./backups
BACKUP=$(ls -t backups/*.archive | head -1)
dbu verify "$BACKUP"
dbu restore -e mongodb "$BACKUP" --to pedidos_copia -u "$MONGO_USER"
dc exec mongodb sh -c 'mongosh --quiet -u "$MONGO_INITDB_ROOT_USERNAME" -p "$MONGO_INITDB_ROOT_PASSWORD" pedidos_copia \
  --authenticationDatabase admin --eval "db.pedidos.countDocuments()"'   # 2
```

O backup avisa que o MongoDB não tem snapshot de um único instante; veja
[Limites de consistência](capacidades.md#limites-de-consistência).

### SQLite

Sem servidor, usuário nem senha. O `sqlite3` só cria o banco de exemplo.

```bash
sqlite3 app.db "CREATE TABLE pedidos (id INTEGER PRIMARY KEY, total REAL); INSERT INTO pedidos VALUES (1, 10.5), (2, 20);"

dbu backup -e sqlite --source ./app.db -o ./backups
BACKUP=$(ls -t backups/*.sqlite3 | head -1)
dbu verify "$BACKUP"
dbu restore -e sqlite "$BACKUP" --to ./app_copia.db
sqlite3 app_copia.db "SELECT count(*) FROM pedidos"   # 2
```

### Histórico e limpeza

```bash
dbu list                 # backups dos quatro roteiros
dbu list restores
dc --profile mysql --profile mongodb down -v          # apaga os containers e os volumes de desenvolvimento
rm -rf backups app.db app_copia.db ~/.config/dbu/*.pass
```

Em um servidor real, troque o host (`-H`) e mantenha o TLS validado (`--tls verify-full`, padrão
fora de `localhost`).

## Opções gerais

```bash
cargo run -p database-backup-cli -- --help
cargo run -p database-backup-cli -- backup --help
```

Comandos: `connection test`, `backup run`, `backup list`, `backup verify`, `restore run`,
`restore list` e `engines` (matriz de capacidades). Cada `--help` traz exemplos.

- `--json` (em qualquer comando) imprime o resultado em uma linha JSON no stdout:
  `{"status":"ok",...}` no sucesso e `{"status":"error","code":N,"error":"..."}` na falha.
  A mensagem de erro continua indo também para o stderr.
- Os logs (`tracing`) vão para o stderr, com horário, operação, IDs, status e duração, e nunca
  incluem senhas ou conteúdo do dump. O padrão é `warn`; use `--log-level info` ou `RUST_LOG`.

Testar a conexão com um PostgreSQL (a senha nunca é passada por argumento):

```bash
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- connection test \
  --host localhost --port 5432 --database my_database --user postgres
```

A senha também pode vir de um arquivo com permissão `600` (`--password-file` ou
`DBU_PG_PASSWORD_FILE`, o indicado para agendadores). Mais de uma fonte definida é erro. Sem nenhuma,
a CLI pede a senha em um prompt sem eco. Detalhes em [`configuracao.md`](configuracao.md#credenciais). Hosts fora de `localhost`
exigem TLS com certificado validado (`--tls verify-full`, padrão em `--tls auto`).

Gerar um backup completo (`pg_dump` em formato custom, já comprimido):

```bash
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- backup run \
  --database my_database --user postgres [--output-dir DIR] [--runner auto|native|docker]
```

- O dump é gravado em streaming num arquivo `.partial`, conferido com `pg_restore --list` e só
  então renomeado para `<database>-<AAAAMMDDTHHMMSSZ>.dump`. Arquivo parcial nunca vira backup.
- Ao lado do dump fica o manifesto versionado `<arquivo>.dump.meta.json` (`0600`), com versão do
  formato, SGBD, versão do utilitário, SHA-256, tamanho, versões e runner, sem credenciais. Detalhes e
  limites em [`manifesto.md`](manifesto.md).
- Cada execução (sucesso, falha ou cancelamento) é registrada no histórico SQLite.
- Destino padrão: `backups/` na pasta de dados do aplicativo (`~/.local/share/database-backup-utility/backups/` no Linux).
- `--runner auto` usa o `pg_dump` instalado se for compatível; senão, o Docker.

Gerar um backup de um banco SQLite (Online Backup API, sem servidor, usuário nem senha):

```bash
cargo run -p database-backup-cli -- backup run --engine sqlite --source ./app.db [--output-dir DIR]
```

- O arquivo de origem é aberto só para leitura. A cópia é consistente mesmo com a aplicação
  gravando, inclusive em modo WAL, e passa por `PRAGMA integrity_check` antes de virar
  `<arquivo>-<AAAAMMDDTHHMMSSZ>.sqlite3`. O artefato fica em modo `DELETE` (sem `-wal`/`-shm`).
- Recusa links simbólicos, arquivos que não são SQLite, o próprio histórico interno do utilitário e
  origens dentro do diretório de backups.
- Em modo WAL, o próprio SQLite cria `-wal`/`-shm` ao lado da origem, como faz com qualquer leitor.
  Os dados da origem não são alterados.

Gerar um backup de um MySQL (`mysqldump`, SQL puro, um banco por arquivo):

```bash
DBU_MYSQL_PASSWORD='...' cargo run -p database-backup-cli -- backup run --engine mysql \
  --database my_database --user backup [--port 3306] [--output-dir DIR] [--runner auto|native|docker]
```

- A senha vem de `--password-file`, `DBU_MYSQL_PASSWORD_FILE` ou `DBU_MYSQL_PASSWORD` e chega ao
  `mysqldump` por um arquivo de opções temporário (`0600`), nunca por argumento. A porta padrão é 3306.
- Usa `--single-transaction`, que dá um snapshot consistente só para tabelas InnoDB. Tabelas de
  outros motores (MyISAM etc.) geram um aviso no log, porque podem ficar inconsistentes se houver
  escrita durante o backup; a lista também sai no `--json` e no manifesto
  (`non_transactional_tables`). Rotinas, triggers, eventos e views entram no dump.
- Um trigger ou evento com o corpo terminado em `;` (criado por ferramentas que enviam vários
  comandos de uma vez) gera um dump que o próprio MySQL não restaura. Nesse caso o backup falha
  (código 8) e diz qual objeto recriar, em vez de entregar um backup inútil.
- O dump é gravado em `.partial`, conferido (cabeçalho do `mysqldump`, linha final
  `-- Dump completed` e banco certo) e só então vira `<database>-<AAAAMMDDTHHMMSSZ>.sql`, com o
  manifesto `.sql.meta.json` ao lado.
- `--runner auto` usa o `mysqldump` instalado se a versão for igual ou maior que a do servidor
  (major.minor); senão, a imagem `mysql:<major.minor>` no Docker. MariaDB ainda não é suportado (DB-37).

Gerar um backup de um MongoDB (`mongodump --archive`, um banco por arquivo, sem compressão):

```bash
DBU_MONGO_PASSWORD='...' cargo run -p database-backup-cli -- backup run --engine mongodb \
  --database my_database --user backup [--auth-database admin] [--port 27017] [--output-dir DIR] [--runner auto|native|docker]
```

- A senha vem de `--password-file`, `DBU_MONGO_PASSWORD_FILE` ou `DBU_MONGO_PASSWORD` e chega ao
  `mongodump` por um arquivo YAML temporário (`0600`) passado com `--config`, nunca por argumento.
  A porta padrão é 27017 e `--auth-database` (padrão `admin`) é o banco onde o usuário foi criado.
- Só aceita um standalone ou o primário de um replica set; secundário, árbitro e cluster sharded são
  recusados (código 2). Servidores abaixo do 4.4 são recusados.
- **Não é um snapshot de um único instante:** cada coleção é lida separadamente, então escritas
  durante o backup podem deixar coleções inconsistentes entre si. A CLI avisa em todo backup. Detalhes
  em [`arquitetura.md`](arquitetura.md#mongodb-topologia-e-consistência).
- O archive é gravado em `.partial`, lido por inteiro (estrutura, banco certo, ao menos uma coleção)
  e só então vira `<database>-<AAAAMMDDTHHMMSSZ>.archive`, com o manifesto `.archive.meta.json` ao
  lado (inclui `mongodump_version` e `topology`). Usuários e roles não entram no backup.
- `--runner auto` usa o `mongodump` instalado (Database Tools `100.x`) se atender à versão do
  servidor; senão, a imagem `mongo:<major.minor>` no Docker. Matriz em [`stack.md`](stack.md).

Consultar o histórico e verificar um backup sem restaurar:

```bash
cargo run -p database-backup-cli -- backup list [--database my_database] [--limit 20]
cargo run -p database-backup-cli -- backup verify --file <arquivo.dump> [--runner auto|native|docker]
cargo run -p database-backup-cli -- restore list
```

- `backup verify` recusa symlinks e arquivos graváveis por outros, confere o manifesto (versão, nome
  do arquivo, tamanho e SHA-256) ou, sem ele, o histórico, e valida a estrutura com
  `pg_restore --list` (PostgreSQL), pela leitura do dump SQL (MySQL, sem ferramentas externas), pela
  leitura do archive (MongoDB, sem ferramentas externas) ou `PRAGMA integrity_check` (SQLite). O SGBD vem do manifesto
  ou, sem ele, do cabeçalho do arquivo. Com o manifesto funciona sem o histórico SQLite, por exemplo depois de copiar
  o par para outra máquina. Sai com 10 se o arquivo estiver ausente, alterado ou inválido.

Restaurar um backup:

```bash
DBU_PG_PASSWORD='...' cargo run -p database-backup-cli -- restore run \
  --file ~/.local/share/database-backup-utility/backups/my_database-20261020T190509Z.dump \
  --target-database my_database_copy --user postgres
```

- Antes de restaurar, confere o manifesto `.meta.json` (ou o histórico), valida o arquivo
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

Restaurar um backup SQLite em um arquivo:

```bash
cargo run -p database-backup-cli -- restore run --engine sqlite \
  --file ~/.local/share/database-backup-utility/backups/app-20261022T120000Z.sqlite3 --target ./app_copia.db
```

- O restore é gravado em `<destino>.partial` (`0600`), conferido com `integrity_check` e pela
  contagem de tabelas, e só então publicado. Um destino novo nunca substitui um arquivo criado por
  outro processo no meio do caminho.
- Arquivo de destino existente exige `--overwrite` e a confirmação do caminho (`--confirm <caminho>`
  em scripts). O atual é mantido como `<destino>.previous-<data>`. Se existir `-wal`, `-shm` ou
  `-journal` ao lado do destino, o banco parece em uso e o restore é recusado. Também é recusado
  (código 11, com o PID) se outro processo estiver com o arquivo aberto ou numa transação. A
  checagem de arquivo aberto só existe no Linux e só vê processos do mesmo usuário; fora disso,
  feche a aplicação antes de sobrescrever.

Restaurar um backup MySQL em um banco novo:

```bash
DBU_MYSQL_PASSWORD='...' cargo run -p database-backup-cli -- restore run --engine mysql \
  --file ~/.local/share/database-backup-utility/backups/my_database-20261023T120000Z.sql \
  --target-database my_database_copy --user backup
```

- Só restaura em um banco que **ainda não existe**: o restore cria o banco, aplica o dump e confere a
  contagem de tabelas. Um banco existente, mesmo vazio, é recusado (código 2) e `--overwrite` ainda
  não é suportado no MySQL.
- O restore do MySQL não é transacional. Se falhar ou for cancelado, o banco criado pelo próprio
  restore é apagado; nenhum outro banco é tocado.
- O cliente `mysql` roda com `--binary-mode` (comandos do cliente como `\!`/`system` dentro do
  arquivo são recusados) e `--local-infile=0`. Dumps que trocam ou criam bancos (`USE`,
  `CREATE DATABASE`, gerados com `--databases`) são recusados.
- Antes de carregar qualquer dado, o restore cria e apaga um objeto de teste de cada tipo com cada
  `DEFINER` do dump. Se o usuário não puder recriá-los (falta `SET_ANY_DEFINER`/`SUPER`, ou o binlog
  exige `SUPER` para triggers e functions), o restore para com código 2, apaga o banco criado e
  explica o privilégio que falta.

Restaurar um backup MongoDB em um banco novo:

```bash
DBU_MONGO_PASSWORD='...' cargo run -p database-backup-cli -- restore run --engine mongodb \
  --file ~/.local/share/database-backup-utility/backups/my_database-20261024T120000Z.archive \
  --target-database my_database_copy --user backup
```

- Só restaura em um banco que **ainda não existe** (um banco com coleções é recusado com código 2) e
  `--overwrite` não é suportado no MongoDB. Bancos reservados (`admin`, `local`, `config`) são recusados.
- Archives vindos de um servidor mais novo que o destino são recusados (código 10). O `mongorestore`
  roda sem `--drop` e só renomeia o banco de origem do archive para o destino.
- Depois do restore, a contagem de documentos de cada coleção e as views do destino são comparadas com
  as do archive. Se divergirem, se o restore falhar ou for cancelado, o banco criado pelo próprio
  restore é apagado (código 11); nenhum outro banco é tocado.
- Usa `--bypassDocumentValidation`, para restaurar documentos antigos que violam o validador atual. O
  usuário precisa de `readWrite` + `dbAdmin` (ou `dbOwner`) no destino; ver
  [`configuracao.md`](configuracao.md#mongodb-usuário-banco-de-autenticação-e-permissões).

| Exit code | Significado |
| --- | --- |
| 0 | Sucesso |
| 1 | Erro não classificado (ex.: histórico ou pasta de destino inacessível) |
| 2 | Parâmetro inválido ou senha ausente |
| 3 | Autenticação recusada |
| 4 | Banco inexistente (ou arquivo SQLite de origem inexistente) |
| 5 | Servidor inacessível ou tempo esgotado |
| 6 | Falha de TLS |
| 7 | `pg_dump`/`pg_restore`/`mysqldump`/`mysql`/`mongodump`/`mongorestore` ausente ou incompatível (e Docker indisponível); não se aplica ao SQLite |
| 8 | O backup falhou |
| 9 | Backup ou restore cancelado (Ctrl+C) |
| 10 | Arquivo de backup ausente, corrompido, sem checksum conhecido ou incompatível |
| 11 | O restore falhou (o destino não foi alterado) |
| 12 | Destino com dados sem `--overwrite`, ou confirmação ausente/divergente |
