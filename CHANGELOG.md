# Changelog

Todas as mudanças relevantes deste projeto são registradas neste arquivo.

O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/) e o
versionamento segue [SemVer](https://semver.org/lang/pt-BR/).

> Somente mudanças implementadas e testadas devem aparecer como entregues.
> As próximas versões estão planejadas em `docs/RELEASES.md`.

## [Unreleased]

### Added
- CLI `database-backup` organizada em `connection test`, `backup run|list|verify` e `restore run|list`, com exemplos em cada `--help` e exit codes documentados.
- Comando `connection test` para validar parâmetros e testar a conexão PostgreSQL, com senha por prompt sem eco ou `DBU_PG_PASSWORD`, TLS verificado para hosts remotos e exit codes por tipo de falha.
- Comando `backup run`: backup full PostgreSQL com `pg_dump` (formato custom) em streaming, executado pelo cliente instalado ou via Docker, com arquivo finalizado de forma atômica, SHA-256, manifesto ao lado do dump e registro no histórico SQLite.
- Comando `restore run`: restaura um backup PostgreSQL após conferir SHA-256, estrutura e versão; cria um banco novo por padrão e só substitui um banco com dados com `--overwrite` e confirmação do nome, restaurando antes em um banco temporário e mantendo o anterior como `<destino>__previous_<data>`. O `pg_restore` roda em uma única transação e cada restore é registrado no histórico. Recusa symlinks e arquivos graváveis por grupo/outros como origem; `--allow-unverified` avisa e marca o restore como não verificado.
- `backup list` e `restore list` consultam o histórico; `backup verify` confere checksum e estrutura de um backup sem restaurar.
- Opção global `--json` com resultado (ou erro e exit code) em uma linha JSON no stdout.
- Manifesto versionado `<dump>.meta.json` (`manifest_version` 1): SGBD, formato, nome do arquivo, versão do utilitário, tamanho e SHA-256, sem credenciais. `backup verify` e `restore run` conferem versão, nome, tamanho e checksum, e funcionam sem o histórico SQLite. Manifestos sem versão ou de versão desconhecida são recusados.
- Senha por arquivo (`--password-file` ou `DBU_PG_PASSWORD_FILE`) para agendadores, aceito só com permissão `600`/`400`, sem symlink, até 4 KiB. Mais de uma fonte de senha definida passa a ser erro explícito, e o histórico registra só a origem (`prompt`, `env:...`, `file:...`).
- Logs estruturados com `tracing` no stderr (horário, operação, IDs, status e duração, sem credenciais), configuráveis por `--log-level` ou `RUST_LOG`.

### Fixed
- Cancelar um backup ou restore no runner Docker logo no início (Ctrl+C ou timeout) podia deixar um container `dbu-pg-*` criado e nunca removido. Agora o cliente `docker run` é encerrado antes e a remoção do container é repetida por até 2 segundos.

### Changed
- `backup run` avisa no stderr quando o diretório de saída já existe e é acessível por grupo ou outros usuários. Os backups continuam com permissão `0600`, e o modo do diretório não é alterado.

### Security
- GitHub Actions fixadas por commit SHA em todos os workflows.
- Clippy proíbe `unwrap()`/`expect()` fora de testes (`[workspace.lints.clippy]`).
- `pg_dump`/`pg_restore` passam a receber o nome do banco como valor entre aspas (`dbname='...'`). Antes, um nome de banco contendo `=` (por exemplo `dbname=postgres`) era interpretado pelo libpq como string de conexão e podia levar o backup ou o restore para outro banco.

<!-- Ao concluir uma funcionalidade relevante, registre-a aqui usando:
### Added
- Mensagem da mudança validada.

Para publicar, mova as entradas para uma nova seção com data real:
Seção versionada: `[0.1.0] - AAAA-MM-DD`
O workflow manual rejeita releases sem seção publicada e sem mudanças reais. -->
