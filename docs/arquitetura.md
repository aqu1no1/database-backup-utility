# Arquitetura

```mermaid
flowchart TD
  CLI[apps/cli - Rust] --> CORE[crates/core]
  DESKTOP[apps/desktop - Angular/Tauri futuro] --> CORE
  CLI --> ADAPTERS[crates/database-adapters]
  DESKTOP --> ADAPTERS
  ADAPTERS --> DB[(PostgreSQL / MySQL / SQLite — implementados<br/>MongoDB — planejado)]
  CORE -. regras e contratos .-> ADAPTERS
```

- CLI e Tauri são interfaces do mesmo sistema; as regras de backup não ficam no frontend.
- O domínio não deve depender de Clap, Tauri, bancos específicos ou infraestrutura Docker.
- Adaptadores lidam com capacidades específicas por SGBD, sem prometer uma função universal.
- `core/`, `shared/`, `features/` e `layout/` são pastas do frontend **quando ele for criado**.
- `crates/` Rust são compartilhadas entre interfaces. A pasta `libs/` TS não será criada sem uso.
- Segredos: `crates/core/src/secret.rs` define o contrato `SecretProvider`, de onde a senha veio
  (`SecretSource`) e a leitura estrita de arquivo de senha. A CLI implementa o contrato em
  `apps/cli/src/credentials.rs` (flag, variáveis, prompt). O desktop e um futuro adaptador de keyring
  entram como outras implementações. Ver [`configuracao.md`](configuracao.md#credenciais).
- `crates/database-adapters/src/postgres/tools.rs` executa `pg_dump`/`pg_restore` em modo `native`
  ou `docker`, sempre com argumentos estruturados e a senha num `.pgpass` temporário. O banco vai
  em `--dbname dbname='<nome escapado>'`: um valor de `--dbname` com `=` seria lido pelo libpq como
  string de conexão (host, porta, banco), então o nome nunca é repassado cru. No cancelamento, o
  cliente `docker run` é encerrado primeiro e o `docker rm --force` é repetido por até 2s. Isso cobre
  o caso em que o daemon termina de criar o container depois que o cliente morreu (container em
  estado "Created" não é removido pelo `--rm`).
  Distribuição nativa das ferramentas será avaliada por SO.
- `crates/database-adapters/src/process.rs` é o runner comum de ferramentas externas (PostgreSQL e
  MySQL): `RunnerChoice`/`RunnerKind`, `ToolError`, o `ToolProcess` com stderr limitado a 8 KiB e
  sanitizado, cancelamento com `docker rm --force` repetido e o diretório temporário de credenciais
  (`0600`, apagado junto com o processo). Os adaptadores só montam os argumentos de cada ferramenta.
- `crates/database-adapters/src/mysql/` segue o mesmo desenho do PostgreSQL:
  - `connection.rs` testa a conexão com `mysql_async` (TLS via rustls com os certificados do
    sistema; as raízes embutidas da `webpki-roots` ficam desligadas) e classifica os erros nos
    mesmos `ConnectionError` do PostgreSQL (1045/1044 → autenticação, 1049 → banco inexistente);
  - `admin.rs` consulta `information_schema` (existência do banco, tabelas, tabelas fora do InnoDB)
    e cria/apaga bancos com identificadores entre crases;
  - `tools.rs` roda `mysqldump`/`mysql` em `native` ou `docker` (imagem `mysql:<major.minor>`), com
    `--defaults-extra-file` como primeiro argumento, `--protocol=TCP` (em `localhost` o cliente
    usaria o socket Unix) e o nome do banco depois de `--` no `mysqldump`, para um nome que começa
    com `-` não virar opção. O restore usa `--binary-mode` e `--local-infile=0`;
  - `dump.rs` lê o dump SQL em streaming, guardando só os primeiros 256 bytes de cada linha:
    cabeçalho do `mysqldump`, banco, versões, contagem de tabelas/views/triggers/rotinas/eventos,
    linha final `-- Dump completed` e recusa de `USE`/`CREATE DATABASE`/`DROP DATABASE`.
- `crates/storage` grava o backup localmente: arquivo `.partial`, SHA-256 em streaming,
  `fsync` e `rename` atômico, além do manifesto versionado `.meta.json` ([`manifesto.md`](manifesto.md)). Destinos em nuvem entram nessa crate.
- O comando `backup run` da CLI monta as peças: conexão → runner → storage → histórico.
- O comando `restore run` faz o caminho inverso: checksum (storage/histórico) → `pg_restore --list` →
  estado do destino (`postgres/admin.rs`) → `pg_restore` em um banco novo ou temporário → troca de nomes.
- `crates/core/src/engine.rs` define o `DatabaseEngine` (`postgres`, `mysql`, `sqlite`). Os nomes são os
  mesmos do `CHECK` da tabela `connections` do histórico e do campo `engine` do manifesto v2. O
  legado `postgresql` (manifesto v1) é lido como `postgres`. MongoDB entra com seu card.
- `crates/core/src/connection.rs` tem um tipo de configuração por servidor
  (`PostgresConnectionConfig`, `MysqlConnectionConfig`) com a mesma regra de TLS (sem TLS só em
  `localhost`) e os limites de nome de cada SGBD. O `SecretProvider` recebe só o destino exibível
  (`&dyn Display`), e a CLI escolhe as variáveis de ambiente pelo SGBD.
- `crates/database-adapters/src/sqlite.rs` faz backup e restore de SQLite com a Online Backup API do
  `rusqlite`, sem ferramenta externa:
  - a origem é aberta só para leitura e precisa ser um arquivo regular com o cabeçalho
    `SQLite format 3`;
  - o destino é sempre um `.partial` já criado (`0600`), aberto sem `CREATE`, com
    `journal_mode=OFF` durante a cópia (não gera `-journal`) e `DELETE` no fim. Sem isso, uma origem
    WAL deixaria o artefato marcado como WAL, e abrir o artefato criaria `-wal`/`-shm`;
  - a cópia anda em passos de 256 páginas, com cancelamento entre os passos. Se outra conexão gravar
    entre dois passos, o SQLite reinicia a cópia; na primeira reinicialização detectada, o restante
    vai num passo só (snapshot consistente), para um escritor contínuo não impedir o fim (livelock).
    Em WAL esse passo não bloqueia escritores; em modo rollback, eles esperam até o fim da cópia;
  - `inspect_backup` roda `PRAGMA integrity_check` e conta objetos e tabelas;
  - `check_not_in_use` recusa um destino de `--overwrite` aberto por outro processo: tenta
    `BEGIN EXCLUSIVE` sem espera e, no Linux, procura o `(dev, inode)` do arquivo em `/proc/<pid>/fd`.
- Todo `.partial` (dump, manifesto e destino de restore SQLite) fica com um lock exclusivo
  (`File::try_lock`, `flock` no Unix) enquanto está em uso. Como o SO solta o lock quando o
  processo morre, `LocalTarget::remove_orphan_partials` consegue distinguir um órfão de um backup
  em andamento sem adivinhar.
- O histórico interno (`crates/history`) também é SQLite, mas é outro arquivo: a CLI recusa usá-lo
  como origem ou destino de backup e restore.
- Agendamento invoca a CLI com mecanismo independente da janela desktop, não incluído no MVP.

## Organização da CLI

`apps/cli/src/` é só a interface. A lógica continua nas crates.

| Arquivo | Papel |
| --- | --- |
| `main.rs` | Árvore de comandos (`connection test`, `backup run/list/verify`, `restore run/list`), opções globais `--json`/`--log-level`, leitura de senha e mapeamento de erros para exit codes. |
| `output.rs` | Único ponto de saída. `success` imprime o texto ou `{"status":"ok",...}`; `fail` escreve `Erro: ...` no stderr e, com `--json`, `{"status":"error","code":N,"error":"..."}` no stdout. Também configura o `tracing`. |
| `archive.rs` | Validações de arquivo compartilhadas por `restore run` e `backup verify`: SGBD do arquivo, checksum contra `.meta.json`/histórico, leitura com `pg_restore --list` e compatibilidade de versões. |
| `backup.rs`, `restore.rs`, `verify.rs`, `listing.rs` | Um comando (ou par de `list`) por arquivo. `backup.rs` e `restore.rs` escolhem o fluxo pelo `--engine`. |
| `sqlite.rs` | Fluxos de backup e restore do SQLite: chama o adaptador em `spawn_blocking`, repassa Ctrl+C e `--timeout` pela flag de cancelamento e publica o resultado com o storage e o histórico. |

Regras da saída:

- **stdout** só recebe o resultado (texto ou uma linha JSON), para ser seguro em scripts.
- **stderr** recebe erros e logs do `tracing`. Os logs têm horário, `operation` (`backup.run`,
  `restore.run`, `backup.verify`, `connection.test`), IDs de execução, `status` e `duration_ms`, sem senha,
  `secret_ref` ou conteúdo do dump. Cores ANSI só quando o stderr é um terminal.
- **Nível dos logs:** `--log-level` > `RUST_LOG` > `warn`. O nível vale para a CLI; as dependências
  ficam em `warn`.

`backup verify` precisa escolher um `pg_restore` compatível sem conectar a um servidor. Ele usa a
versão do PostgreSQL de origem gravada no manifesto (`.meta.json`) ou, na falta dele, a versão
registrada no histórico. Sem nenhuma das duas, o arquivo é recusado (código 10). Com o manifesto,
verify e restore não dependem do histórico SQLite. As checagens estão em [`manifesto.md`](manifesto.md).
Para SQLite não há versão de ferramenta a escolher: o `verify` usa o `integrity_check` do próprio
`rusqlite`. Para MySQL também não: o dump é SQL puro e o `verify` o lê em Rust (`mysql/dump.rs`),
sem `mysql` nem Docker.

O restore MySQL não tem troca atômica: o MySQL não tem `RENAME DATABASE`, e um `RENAME TABLE` não
leva views, rotinas nem triggers. Por isso ele só cria um banco novo (`apps/cli/src/mysql.rs`):
confere que o destino não existe, cria, aplica o dump, compara a contagem de tabelas com o dump e,
em falha ou cancelamento, apaga só o banco que ele mesmo criou.

## Fluxo alvo de backup

```mermaid
flowchart LR
  CFG[Validar configuração] --> CONN[Testar conexão]
  CONN --> DUMP[Gerar dump em stream]
  DUMP --> VERIFY[Finalizar / checksum]
  VERIFY --> STORE[Storage local]
  STORE --> HIST[Registrar histórico SQLite]
```

Antes de operar em produção: implementar autenticação segura, erros sanitizados, consistência,
verificação de integridade, confirmação de restore destrutivo, limites de recursos e testes reais.
