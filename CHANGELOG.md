# Changelog

Todas as mudanças relevantes deste projeto são registradas neste arquivo.

O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/) e o
versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

> Somente mudanças implementadas e testadas devem aparecer como entregues.
> As próximas versões estão planejadas em `docs/RELEASES.md`.

## [Unreleased]

### Added
- Backup, verificação e restore de bancos **SQLite** como origem: `backup run --engine sqlite --source <arquivo>` e `restore run --engine sqlite --file <backup> --target <arquivo>`. A cópia usa a Online Backup API, é consistente com a aplicação gravando (inclusive em WAL) e passa por `PRAGMA integrity_check`. O restore nunca sobrescreve sem `--overwrite` e confirmação e mantém o arquivo anterior.
- `backup verify` identifica o SGBD pelo manifesto ou pelo cabeçalho do arquivo.
- `backup run` remove os arquivos `.partial` deixados por backups interrompidos à força (sem lock e parados há mais de 10 minutos), sem tocar em backups em andamento.
- `restore run --engine sqlite --overwrite` recusa um destino aberto por outro processo ou com transação aberta, informando o PID quando possível.
- Backup, verificação e restore de **MySQL** com `--engine mysql` em `connection test`, `backup run` e `restore run` (porta padrão 3306). O backup usa o `mysqldump` (`--single-transaction`, com rotinas, triggers e eventos), nativo ou via Docker (`mysql:<major.minor>`), e gera `<banco>-<data>.sql` com manifesto. `backup verify` confere o dump SQL sem ferramentas externas.
- O restore MySQL só cria bancos novos: recusa banco existente e `--overwrite`, e apaga o banco criado se o restore falhar ou for cancelado. O cliente `mysql` roda com `--binary-mode` e `--local-infile=0`, e dumps com `USE`/`CREATE DATABASE` são recusados.
- Senha do MySQL por `--password-file`, `DBU_MYSQL_PASSWORD_FILE` ou `DBU_MYSQL_PASSWORD`, entregue às ferramentas por um arquivo de opções temporário (`0600`), nunca por argumento.
- O backup MySQL lista as tabelas fora do InnoDB, que não entram no snapshot consistente, no log, no `--json` e no manifesto (`non_transactional_tables`).
- O backup MySQL falha (código 8) quando um trigger ou evento tem o corpo terminado em `;`, caso em que o próprio MySQL não conseguiria restaurar o dump; a mensagem diz qual objeto recriar.
- O restore MySQL testa, antes de carregar dados, se o usuário consegue recriar os objetos com os `DEFINER` do dump; se não, para com código 2 e apaga o banco criado.

### Changed
- Manifesto passa para a versão 2: campos de servidor (`host`, `port`, `pg_dump_version`, `runner`) ficam opcionais e `engine` passa de `postgresql` para `postgres`. Manifestos v1 continuam aceitos; a v0.1.x recusa manifestos v2.
- Histórico: migration `0004` permite restores sem host e porta e registra o SGBD de cada restore.
- Manifesto v2 ganha os campos opcionais `mysqldump_version` e `non_transactional_tables` (só em backups MySQL); a versão do formato continua 2.

### Fixed
- Um backup que coincide no nome com um `.partial` deixado no mesmo segundo agora falha com "já existe um arquivo em …", em vez de um erro de E/S sem explicação.

<!-- Ao concluir uma funcionalidade relevante, registre-a aqui usando:
### Added
- Mensagem da mudança validada.

Para publicar, mova as entradas para uma nova seção com data real:
Seção versionada: `[X.Y.Z] - AAAA-MM-DD`
O workflow manual rejeita releases sem seção publicada e sem mudanças reais. -->

## [0.1.1] - 2026-10-22

Release de correção: o código do projeto é o mesmo da v0.1.0, com o Tokio atualizado para corrigir
uma vulnerabilidade. O tarball passa a trazer os avisos de licença das dependências, que faltavam
na v0.1.0.

### Added
- `THIRD-PARTY-LICENSES.md` no tarball da release, gerado com `cargo-about`, com os avisos de licença das dependências embutidas no binário.

### Security
- Tokio atualizado de 1.54.0 para 1.54.1, corrigindo a race condition em `tokio::time::Interval` ([RUSTSEC-2026-0384](https://rustsec.org/advisories/RUSTSEC-2026-0384)).
- `unsafe` proibido em todo o workspace.
- Dependências verificadas por `cargo-deny` em cada PR (advisories RustSec, licenças, fontes e wildcards), com triagem documentada em `docs/seguranca-rust.md`.
- Toolchain do Rust fixada em `rust-toolchain.toml` e Cargo com `--locked` em toda a CI.

## [0.1.0] - 2026-10-21

Primeira versão: CLI de backup e restauração de PostgreSQL para Linux x86_64. Requer o cliente
PostgreSQL (`pg_dump`/`pg_restore`) com versão maior ou igual à do servidor, ou Docker.

### Added
- CLI `database-backup` organizada em `connection test`, `backup run|list|verify` e `restore run|list`, com exemplos em cada `--help` e exit codes documentados por tipo de falha.
- `connection test`: valida parâmetros e testa a conexão PostgreSQL, com TLS e certificado verificados para hosts remotos.
- `backup run`: backup full com `pg_dump` (formato custom) em streaming, pelo cliente instalado ou via Docker. O arquivo é finalizado de forma atômica (`.partial` + `rename`), com permissão `0600`, SHA-256, manifesto ao lado e registro no histórico SQLite. Ctrl+C ou timeout não deixam arquivo parcial nem container do runner. O comando avisa quando o diretório de saída é acessível por outros usuários.
- Manifesto versionado `<dump>.meta.json` (`manifest_version` 1): SGBD, formato, nome do arquivo, versão do utilitário, tamanho e SHA-256, sem credenciais.
- `backup verify`: confere manifesto, tamanho, SHA-256 e estrutura (`pg_restore --list`) sem restaurar e sem depender do histórico.
- `restore run`: restaura depois de conferir origem, manifesto, checksum, estrutura e versão. Cria um banco novo por padrão e só substitui um banco com dados com `--overwrite` e confirmação do nome. Nesse caso restaura antes em um banco temporário e mantém o anterior como `<destino>__previous_<data>`. O `pg_restore` roda em uma única transação. Arquivos sem checksum conhecido exigem `--allow-unverified`, e o restore fica marcado como não verificado.
- `backup list` e `restore list` consultam o histórico interno.
- Senha por prompt sem eco, `DBU_PG_PASSWORD` ou arquivo (`--password-file`/`DBU_PG_PASSWORD_FILE`, só com permissão `600`/`400`). Mais de uma fonte definida é erro. O histórico registra só a origem da senha, nunca o valor.
- Opção global `--json` com o resultado (ou erro e exit code) em uma linha JSON no stdout.
- Logs estruturados com `tracing` no stderr (horário, operação, IDs, status e duração, sem credenciais), configuráveis por `--log-level` ou `RUST_LOG`.
- Licença dupla **MIT OR Apache-2.0** (`LICENSE-MIT`, `LICENSE-APACHE`), incluída no tarball da release junto com o README.

### Security
- Senhas nunca vão em argumentos de processo: os utilitários recebem um `.pgpass` temporário e privado.
- O nome do banco é passado aos utilitários sempre como valor entre aspas (`dbname='...'`), para que nomes contendo `=` não sejam interpretados como string de conexão.
- Origens de restore recusadas: links simbólicos e arquivos (ou manifestos) graváveis por grupo ou outros.
- GitHub Actions fixadas por commit SHA; Clippy proíbe `unwrap()`/`expect()` fora de testes.
- Controles, evidências e riscos aceitos documentados em `docs/seguranca.md`.
