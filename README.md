# Database Backup Utility

Ferramenta multiplataforma de backup e restauração de bancos de dados. O produto terá uma
**CLI independente em Rust** e, futuramente, um **desktop Tauri + Angular + Tailwind v4**,
ambos utilizando as mesmas crates de negócio.

> **Estado atual (v0.1.1 + `Unreleased`):** CLI para PostgreSQL com teste de conexão, backup full,
> verificação e restore, histórico interno em SQLite, manifesto versionado e testes E2E. Em
> `Unreleased` (ainda sem release): backup, verificação e restore de bancos **SQLite** como origem e
> de **MySQL** (`mysqldump`, restore só em banco novo).
> **Ainda não existem:** desktop Angular/Tauri, MongoDB, sobrescrita no restore MySQL, agendamento,
> nuvem, compressão extra e criptografia. Esses itens estão planejados em [`docs/RELEASES.md`](docs/RELEASES.md).

## Stack

- CLI: Rust + Clap; bibliotecas compartilhadas no Cargo Workspace.
- Desktop (planejado): Tauri + Angular standalone + TypeScript + Tailwind v4.
- i18n desktop: Transloco, `pt-BR` e `en-US`.
- Desenvolvimento: PostgreSQL no Docker Compose; MySQL/MongoDB em perfis opcionais; histórico interno em SQLite embutido.
- Testes: Rust `cargo test`, integração com Testcontainers, Vitest futuramente no Angular.
- GitHub Actions: CI em PRs/main, auditoria e releases **somente manuais** (`workflow_dispatch`).

## Instalação (binário da release)

Disponível para Linux x86_64 nos [GitHub Releases](https://github.com/aqu1no1/database-backup-utility/releases).

```bash
VERSION=v0.1.1
curl -LO "https://github.com/aqu1no1/database-backup-utility/releases/download/$VERSION/database-backup-linux-x86_64-$VERSION.tar.gz"
curl -LO "https://github.com/aqu1no1/database-backup-utility/releases/download/$VERSION/SHA256SUMS.txt"
sha256sum -c SHA256SUMS.txt                         # precisa mostrar OK
tar -xzf "database-backup-linux-x86_64-$VERSION.tar.gz"
install -m 755 database-backup ~/.local/bin/        # ou outro diretório do PATH
# o tarball traz LICENSE-MIT, LICENSE-APACHE e (a partir da v0.1.1) THIRD-PARTY-LICENSES.md
database-backup --version
```

Requisitos para usar, um dos dois:
- **Cliente PostgreSQL** (`pg_dump` e `pg_restore`) com versão **maior ou igual** à do servidor
  (`--runner native`);
- **Docker**: a CLI usa a imagem `postgres:<versão do servidor>` (`--runner docker`). Quem pode usar o
  Docker tem, na prática, acesso de root; veja [`docs/seguranca.md`](docs/seguranca.md).

O padrão `--runner auto` usa o cliente instalado se for compatível; senão, o Docker.

### Roteiro rápido: backup, verificação e restore em staging

```bash
mkdir -p ~/.config/dbu && install -m 600 /dev/null ~/.config/dbu/pg.pass && printf '%s\n' 'senha' > ~/.config/dbu/pg.pass
export DBU_PG_PASSWORD_FILE=~/.config/dbu/pg.pass

database-backup connection test --host db.interno --tls verify-full --database app --user backup
database-backup backup run --host db.interno --tls verify-full --database app --user backup
database-backup backup list --database app
database-backup backup verify --file ~/.local/share/database-backup-utility/backups/app-<data>.dump
database-backup restore run --host staging.interno --tls verify-full --user backup \
  --file ~/.local/share/database-backup-utility/backups/app-<data>.dump --target-database app_staging
```

O restore cria um banco **novo** por padrão. Substituir um banco com dados exige `--overwrite` e a
confirmação do nome, e o banco anterior é mantido. Detalhes nas seções abaixo e em
[`docs/comandos.md`](docs/comandos.md).

## Requisitos locais (desenvolvimento)

- Rust estável (`rustup`, Cargo), Docker Engine e Docker Compose v2.
- Python 3.11+ somente para scripts de release.
- PNPM e Node.js serão necessários quando a aplicação Angular for implementada.

## Executar a CLI

```bash
cargo run -p database-backup-cli -- --help
cargo run -p database-backup-cli -- backup --help
```

Comandos: `connection test`, `backup run`, `backup list`, `backup verify`, `restore run` e
`restore list`. Cada `--help` traz exemplos.

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
a CLI pede a senha em um prompt sem eco. Detalhes em [`docs/configuracao.md`](docs/configuracao.md#credenciais). Hosts fora de `localhost`
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
  limites em [`docs/manifesto.md`](docs/manifesto.md).
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

Consultar o histórico e verificar um backup sem restaurar:

```bash
cargo run -p database-backup-cli -- backup list [--database my_database] [--limit 20]
cargo run -p database-backup-cli -- backup verify --file <arquivo.dump> [--runner auto|native|docker]
cargo run -p database-backup-cli -- restore list
```

- `backup verify` recusa symlinks e arquivos graváveis por outros, confere o manifesto (versão, nome
  do arquivo, tamanho e SHA-256) ou, sem ele, o histórico, e valida a estrutura com
  `pg_restore --list` (PostgreSQL), pela leitura do dump SQL (MySQL, sem ferramentas externas) ou
  `PRAGMA integrity_check` (SQLite). O SGBD vem do manifesto
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

| Exit code | Significado |
| --- | --- |
| 0 | Sucesso |
| 1 | Erro não classificado (ex.: histórico ou pasta de destino inacessível) |
| 2 | Parâmetro inválido ou senha ausente |
| 3 | Autenticação recusada |
| 4 | Banco inexistente (ou arquivo SQLite de origem inexistente) |
| 5 | Servidor inacessível ou tempo esgotado |
| 6 | Falha de TLS |
| 7 | `pg_dump`/`pg_restore`/`mysqldump`/`mysql` ausente ou incompatível (e Docker indisponível); não se aplica ao SQLite |
| 8 | O backup falhou |
| 9 | Backup ou restore cancelado (Ctrl+C) |
| 10 | Arquivo de backup ausente, corrompido, sem checksum conhecido ou incompatível |
| 11 | O restore falhou (o destino não foi alterado) |
| 12 | Destino com dados sem `--overwrite`, ou confirmação ausente/divergente |

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
| [Configuração](docs/configuracao.md) | Variáveis, Docker e credenciais |
| [Segurança](docs/seguranca.md) | Controles do MVP com evidências, riscos aceitos e diferenças por SO |
| [Manifesto](docs/manifesto.md) | Formato versionado do `.meta.json` e limites do checksum |
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

Licenciado sob qualquer uma destas, à sua escolha:

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) ou <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](LICENSE-MIT) ou <https://opensource.org/licenses/MIT>)

### Contribuição

Salvo declaração explícita em contrário, qualquer contribuição enviada intencionalmente para inclusão
neste projeto, conforme definido na licença Apache-2.0, será licenciada nos termos acima, sem termos
ou condições adicionais.
